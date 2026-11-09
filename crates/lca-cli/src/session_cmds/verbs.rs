//! The per-verb session commands whose body is a store operation:
//! `config`, `resume --list`, `fork`, `clone`, `rename`, `delete`, and the one
//! quiet-rename they share.
//!
//! Split out of `session_cmds/mod.rs` at gate 11's 1,200-line ceiling (gh #543).
//! The seam is **one command, one store call**: each function here resolves what
//! the verb asked for, calls one `lca_session` operation, and prints or reports.
//! The routing vocabulary and `resolve_session` stay in `mod.rs`; `export.rs`
//! writes the artifact and `session_maint.rs` reclaims the bytes.

use std::path::Path;

use lca_permissions::GrantStore;
use lca_session::Session;
use lca_session::SessionStore;

use super::{exit, open_store};
use crate::{CliFlags, data_dir, load_config};

pub(crate) fn config_command(cwd: &Path, approve: bool, no_approve: bool) -> i32 {
    let data = data_dir();
    let mut grants = match GrantStore::open(&data.join("grants.json")) {
        Ok(grants) => grants,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::INTERNAL;
        }
    };
    // gh #80: the dump shows what this process would load.
    let flags = CliFlags {
        approve,
        no_approve,
        ..Default::default()
    };
    let trust_default = crate::trust_default_fallback();
    crate::apply_startup_trust(&mut grants, cwd, &flags, &trust_default);
    let config = match load_config(cwd, &grants, false, false) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    };
    // gh #30 (PG-032): print-only by design - the interactive editor is
    // the `/settings` selector, and this is the same dump `/settings`
    // used to print: every key with its winning source, the live
    // overrides labelled, `shell.resolved`, and the screen renderer.
    let shell = crate::resolve_shell(&config).ok();
    let shell_error = crate::resolve_shell(&config).err();
    print!(
        "{}",
        crate::tui::settings_text(
            &config,
            config.thinking(),
            config.ui_theme(),
            shell.as_ref(),
            shell_error.as_deref(),
        )
    );
    0
}

pub(crate) fn resume_list(cwd: &Path) -> i32 {
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

pub(crate) fn fork_command(cwd: &Path, id: &str, message: &str) -> i32 {
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

pub(crate) fn clone_command(cwd: &Path, id: &str, title: Option<&str>) -> i32 {
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
    match store.clone_session(&parent, title) {
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

pub(crate) fn rename_command(cwd: &Path, id: &str, title: &str) -> i32 {
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

/// `lca delete <id>`: move a session to the trash directory (gh
/// #75). Prints where it went so recovery is one `mv` away.
pub(crate) fn delete_command(cwd: &Path, id: &str) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.session(cwd, id) {
        Ok(session) => match store.trash(&session) {
            Ok(target) => {
                println!(
                    "trashed {} (recover with `mv {} <sessions-dir>`)",
                    id,
                    target.display()
                );
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

/// Apply `--name` to an opened session (gh #69): a rename failure
/// warns and the interface still opens.
pub(crate) fn rename_quiet(store: &SessionStore, session: &Session, name: Option<&str>) {
    if let Some(name) = name
        && let Err(err) = store.rename(session, name)
    {
        eprintln!("warning: cannot name the session `{name}`: {err}");
    }
}
