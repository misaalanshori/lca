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
