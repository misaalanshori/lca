//! `lca export`: one session to JSON, and optionally to HTML beside it
//! (FR-SESS-*'s export paragraph).
//!
//! Split out of `session_cmds/mod.rs` at gate 11's 1,200-line ceiling (gh #543).
//! The seam is the **artifact**: `gc` (in `session_maint.rs`) reclaims a session's
//! bytes and this writes them out; what stays in `mod.rs` is the verb vocabulary
//! and the routing every subcommand shares. `--output` relocates both files, and
//! the HTML lands next to whichever JSON path this run actually reports.

use std::path::Path;

use lca_session::ExportOptions;

use super::{exit, open_store};

pub(crate) fn export_command(
    cwd: &Path,
    id: &str,
    audit: bool,
    html: bool,
    output: Option<&Path>,
) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.session(cwd, id) {
        Ok(session) => {
            let json = match store.export(&session, ExportOptions { audit }) {
                Ok(path) => path,
                Err(err) => {
                    eprintln!("error: {err}");
                    return exit::SESSION;
                }
            };
            // `--output` relocates the JSON; the HTML lands next to
            // whichever JSON path this run reports.
            let json = match output {
                Some(dest) => match std::fs::copy(&json, dest) {
                    Ok(_) => dest.to_path_buf(),
                    Err(err) => {
                        eprintln!("error: {err}");
                        return exit::SESSION;
                    }
                },
                None => json,
            };
            println!("{}", json.display());
            if html {
                match store.export_html(&session, ExportOptions { audit }) {
                    Ok(path) => {
                        let shown = match output {
                            Some(dest) => {
                                let sibling = dest.with_extension("html");
                                if std::fs::copy(&path, &sibling).is_ok() {
                                    sibling
                                } else {
                                    path
                                }
                            }
                            None => path,
                        };
                        println!("{}", shown.display());
                    }
                    Err(err) => {
                        eprintln!("error: {err}");
                        return exit::SESSION;
                    }
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
