//! Session maintenance commands (`migrate`, `gc`): split from
//! `session_cmds.rs` for the line ceiling; behavior unchanged.

use std::path::Path;

use super::exit;
use super::session_cmds::open_store;

/// `lca import <file>`: JSONL lines become a new session (gh #57).
/// Prints the new session id; the transcript is resumable.
pub(super) fn import_command(cwd: &Path, file: &Path) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::SESSION;
        }
    };
    let lines: Vec<&str> = text.lines().collect();
    let title = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "imported".to_string());
    // The store keys sessions by working directory; the import lands
    // in this invocation's tree.
    match store.import_jsonl(cwd, &title, &lines) {
        Ok(session) => {
            println!("{}", session.id());
            exit::OK
        }
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

/// `lca session migrate <id>`: forward-migrate one log (gh #98).
/// Prints what was stamped and where the backup lives, or that the
/// log was already current; refusals name what to fix first.
pub(super) fn migrate_command(cwd: &Path, id: &str) -> i32 {
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
    match store.migrate(&session) {
        Ok(report) => {
            if report.already_current {
                println!("already current");
            } else {
                println!(
                    "migrated {} record{} (backup {})",
                    report.stamped,
                    if report.stamped == 1 { "" } else { "s" },
                    report
                        .backup
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                );
            }
            for warning in report.warnings {
                println!("note: {warning}");
            }
            exit::OK
        }
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
