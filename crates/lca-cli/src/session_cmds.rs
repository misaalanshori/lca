//! The session subcommands: `config`, `resume`, `fork`, `rename`,
//! `export`, and `session gc` (S3's ceiling split).

use super::*;

pub(super) fn config_command(cwd: &Path) -> i32 {
    let data = data_dir();
    let grants = match GrantStore::open(&data.join("grants.json")) {
        Ok(grants) => grants,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::INTERNAL;
        }
    };
    let config = match load_config(cwd, &grants, false, false) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    };
    let mut lines: Vec<(String, String, MergeSource)> = config
        .resolved()
        .map(|(key, value, source)| (key.to_string(), value, source))
        .collect();
    lines.sort();
    for (key, value, source) in lines {
        println!("{key} = {value}  [{source}]");
    }
    let _ = ColorMode::Auto; // documented in the table above; nothing extra to print
    0
}

pub(super) fn open_store() -> Result<(SessionStore, PathBuf), i32> {
    let data = data_dir();
    Ok((SessionStore::new(data.clone()), data))
}

/// Resolve a parsed command line to a route.
pub fn route(cli: &Cli) -> Route {
    if let Some(id) = &cli.resume_id {
        return Route::Interactive {
            resume: Some(id.clone()),
            model: cli.model.clone(),
        };
    }
    match &cli.command {
        None => match &cli.prompt {
            Some(prompt) => Route::Headless {
                prompt: prompt.clone(),
            },
            None => {
                if cli.r#continue {
                    let data = data_dir();
                    let store = SessionStore::new(data.clone());
                    let cwd = std::env::current_dir().unwrap_or_default();
                    let latest = store
                        .list_sessions(&cwd)
                        .ok()
                        .and_then(|list| list.into_iter().next())
                        .map(|s| s.id);
                    Route::Interactive {
                        resume: latest,
                        model: cli.model.clone(),
                    }
                } else {
                    Route::Interactive {
                        resume: None,
                        model: cli.model.clone(),
                    }
                }
            }
        },
        Some(Command::Config) => Route::Config,
        Some(Command::Resume { id }) => match id {
            None => Route::ResumeList,
            Some(id) => Route::Interactive {
                resume: Some(id.clone()),
                model: cli.model.clone(),
            },
        },
        Some(Command::Fork { session, message }) => Route::Fork {
            session: session.clone(),
            message: message.clone(),
        },
        Some(Command::Rename { session, title }) => Route::Rename {
            session: session.clone(),
            title: title.clone(),
        },
        Some(Command::Export { session, audit }) => Route::Export {
            session: session.clone(),
            audit: *audit,
        },
        Some(Command::Session { cmd }) => match cmd {
            SessionCmd::Gc { session } => Route::Gc {
                session: session.clone(),
            },
        },
        Some(Command::Ext { cmd }) => Route::Ext(cmd.clone()),
    }
}

pub(super) fn resume_list(cwd: &Path) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.list_sessions(cwd) {
        Ok(sessions) => {
            if sessions.is_empty() {
                println!("No sessions yet. Start one with `lca -p \"...\"`.");
                return exit::OK;
            }
            for session in sessions {
                println!(
                    "{}\t{}\t{} messages",
                    session.id,
                    session.display_title(),
                    session.message_count
                );
            }
            exit::OK
        }
        Err(err) => {
            eprintln!("error: cannot list sessions: {err}");
            exit::SESSION
        }
    }
}

pub(super) fn fork_command(cwd: &Path, id: &str, message: &str) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    let parent = match store.session(cwd, id) {
        Ok(parent) => parent,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::SESSION;
        }
    };
    match store.fork(&parent, message) {
        Ok(child) => {
            println!("{}", child.id());
            exit::OK
        }
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

pub(super) fn rename_command(cwd: &Path, id: &str, title: &str) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.session(cwd, id) {
        Ok(session) => match store.rename(&session, title) {
            Ok(()) => exit::OK,
            Err(err) => {
                eprintln!("error: {err}");
                exit::SESSION
            }
        },
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

pub(super) fn export_command(cwd: &Path, id: &str, audit: bool) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.session(cwd, id) {
        Ok(session) => match store.export(&session, ExportOptions { audit }) {
            Ok(path) => {
                println!("{}", path.display());
                exit::OK
            }
            Err(err) => {
                eprintln!("error: {err}");
                exit::SESSION
            }
        },
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

/// `lca session gc <id>`: mark-and-sweep over the session's fork tree (D5).
/// Prints one deleted hash per line, or a line saying nothing was collected.
pub(super) fn gc_command(cwd: &Path, id: &str) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    let session = match store.session(cwd, id) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::SESSION;
        }
    };
    match store.gc(&session) {
        Ok(deleted) => {
            if deleted.is_empty() {
                println!("no unreferenced attachments");
            } else {
                for hash in deleted {
                    println!("{hash}");
                }
            }
            exit::OK
        }
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn route_of(args: &[&str]) -> Route {
        let cli = Cli::parse_from(args);
        route(&cli)
    }

    // Verifies: issue #6 (pi's CLI surface) - `-r <id>` opens that session,
    // `resume <id>` is its long form, a bare `lca` opens fresh, `--model`
    // rides every one of them into the interface, and `-p` stays headless.
    #[test]
    fn resume_and_continue_flags_route_to_the_interface() {
        assert_eq!(
            route_of(&["lca", "-r", "abc", "--model", "m"]),
            Route::Interactive {
                resume: Some("abc".to_string()),
                model: Some("m".to_string()),
            }
        );
        assert_eq!(
            route_of(&["lca", "resume", "abc"]),
            Route::Interactive {
                resume: Some("abc".to_string()),
                model: None,
            }
        );
        assert_eq!(
            route_of(&["lca"]),
            Route::Interactive {
                resume: None,
                model: None,
            }
        );
        assert_eq!(
            route_of(&["lca", "-p", "hi"]),
            Route::Headless {
                prompt: "hi".to_string(),
            }
        );
    }

    // `-c` resolves this directory's latest session (whatever the machine
    // happens to hold), so the guarantee under test is the shape: it opens
    // the interface, with the model flag still attached.
    #[test]
    fn continue_opens_the_interface_with_the_model_flag() {
        match route_of(&["lca", "-c", "--model", "x"]) {
            Route::Interactive { model, .. } => {
                assert_eq!(model, Some("x".to_string()));
            }
            other => panic!("-c opens the interface, not {other:?}"),
        }
    }
}
