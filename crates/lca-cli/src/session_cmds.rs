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

/// `lca --list-models [search]`: every model the configured provider
/// offers, as `id  provider  context` lines, then exit 0 (gh #8,
/// EFG-003's CI building block).
///
/// `provider` is the row's profile when it has one (gh #31 - the service
/// that will answer), the provider extension's name otherwise; `context`
/// is the window in tokens, `0` when the provider publishes none. The
/// output is sorted by provider then id, so a diff of two listings says
/// what changed. An optional pattern filters it through the same matcher
/// `models.enabled` uses.
pub(super) fn list_models_command(
    cwd: &Path,
    search: &str,
    cli: &CliFlags,
    allow_host: &[String],
) -> i32 {
    let data = data_dir();
    let grants = match GrantStore::open(&data.join("grants.json")) {
        Ok(grants) => std::sync::Arc::new(std::sync::Mutex::new(grants)),
        Err(err) => {
            eprintln!("error: cannot open the grant store: {err}");
            return exit::INTERNAL;
        }
    };
    let config = match load_config_flags(cwd, &lock(&grants), true, false, cli) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    };
    // The gh #31 rule holds for a listing: a discovery `GET /models` is a
    // live request to a profile endpoint, and this command cannot prompt.
    // `--allow-host` grants the host for this run (the same session set;
    // a listing has no session to record a `permission` answer in, and
    // nothing has been requested yet). An ungranted host is refused with
    // headless mode's fix rather than reached for silently.
    for host in allow_host {
        if let Err(err) = lock(&grants).attach_session_net_pattern(cwd, host) {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    }
    if let Some(host) = crate::net_consent::env_configured_host(&data)
        && crate::ungranted_host(&grants, cwd, Some(host.clone())).is_some()
    {
        eprintln!("{}", crate::net_consent::denied_message(&host));
        return exit::PERMISSION;
    }
    let provider_name = config.provider().to_string();
    // No session exists to hang a stats source on: the native set's stats
    // source answers empty, because a listing is not a turn.
    let stats: lca_ext_native::StatsSource = Arc::new(String::new);
    let prompt = crate::HeadlessPrompt::default();
    let shared_prompt = lca_permissions::SharedPrompt::default();
    shared_prompt.set(std::sync::Arc::new(std::sync::Mutex::new(prompt.clone())));
    let registry = crate::registry::assemble(cwd, &config, shared_prompt, &grants, stats);
    let provider: Arc<dyn lca_provider::Provider> = match registry.provider(&provider_name) {
        Some(handle) => Arc::new(lca_core::ExtensionProvider::new(handle.clone())),
        None => {
            eprintln!("{}", crate::no_model_message(&provider_name));
            return exit::USAGE;
        }
    };
    let mut rows = crate::models::filter_enabled(provider.list_models(), config.models_enabled());
    let search = search.trim();
    if !search.is_empty() {
        rows.retain(|model| crate::models::in_scope(model, &[search.to_string()]));
    }
    if prompt.needed_approval() {
        // Something the listing touched wanted a human and there is no
        // human here: say so instead of printing a list built around the
        // refusal.
        eprintln!(
            "error: listing the models needed an approval this command cannot ask for; \
             run `lca` once interactively and approve it"
        );
        return exit::PERMISSION;
    }
    if rows.is_empty() && !search.is_empty() {
        println!("no models matching \"{search}\"");
        return exit::OK;
    }
    let service = |model: &lca_protocol::ModelInfo| {
        model
            .extras
            .get("profile")
            .cloned()
            .unwrap_or_else(|| provider_name.clone())
    };
    rows.sort_by(|a, b| service(a).cmp(&service(b)).then_with(|| a.id.cmp(&b.id)));
    for model in &rows {
        println!("{}  {}  {}", model.id, service(model), model.context_window);
    }
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;
    // `Cli::parse_from` needs the derive's trait in scope; it used to
    // arrive through the crate root, which the `cli_args` split took with it.
    use clap::Parser;

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
