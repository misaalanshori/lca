//! GitHub #443: `/bug`, `/export`, and `/import` were implemented and
//! described in `/help` but absent from `slash_commands`, so the interface
//! router refused them and all three answered `unknown command`.
//!
//! The three registrations are the instance; the guard in
//! `crates/lca-cli/src/tui/commands.rs` (`command_registration_guards`) and
//! `crates/lca-ui/src/chat_commands.rs` (`help_table_guards`) are the class.
//! This file is the NFR-24 named guard: it pins the *observable* contract
//! those unit rows assume - that the source still has the shape the guards
//! scan - so a future refactor that moves the dispatcher out of reach of
//! `include_str!` cannot leave the guards silently checking an empty list.
//!
//! Source-scanning, not behaviour: the behavioural rows live beside the code
//! they exercise. Keeping the two apart is what lets this one assert the
//! mechanism (a reader that finds arms) without duplicating the rows.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The host's dispatch arms, read the same way the in-crate guard reads
/// them. Kept in the test rather than exported so the guard file stays the
/// single definition of "handled".
fn dispatch_arms(source: &str) -> Vec<String> {
    let start = source
        .find("fn dispatch_command(self: &Arc<Self>")
        .expect("the host dispatcher exists");
    let body = &source[start..];
    let end = body.find("\n    }\n").expect("the dispatcher has a body");
    let mut arms = Vec::new();
    for line in body[..end].lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("name if ") && trimmed.contains("=>") {
            for (index, part) in trimmed.split('"').enumerate() {
                if index % 2 == 1 && !part.contains(' ') && !part.ends_with(':') {
                    arms.push(part.to_string());
                }
            }
            continue;
        }
        if !trimmed.starts_with('"') || !trimmed.contains("=>") {
            continue;
        }
        for (index, part) in trimmed.split('"').enumerate() {
            if index % 2 == 1 && !part.contains(' ') {
                arms.push(part.to_string());
            }
        }
    }
    arms.sort();
    arms.dedup();
    arms
}

/// The names `HANDLED_NAMES` declares, read from source.
fn handled_names(source: &str) -> Vec<String> {
    let start = source
        .find("pub(crate) const HANDLED_NAMES")
        .expect("HANDLED_NAMES exists: gh #443's single registration list");
    let body = &source[start..];
    let end = body.find("];").expect("the list is terminated");
    let mut names = Vec::new();
    for line in body[..end].lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('"')
            && let Some(name) = rest.split('"').next()
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// The three commands this defect is about, and the artifact each produces.
/// Kept here so the guard names them even if the list is reorganised.
const CASUALTIES: [(&str, &str); 3] = [
    ("bug", "a diagnostic bundle"),
    ("export", "the session written out"),
    ("import", "JSONL imported as a session"),
];

#[test]
fn the_three_commands_are_registered_where_the_router_looks() {
    let source = read("crates/lca-cli/src/tui/commands.rs");
    let handled = handled_names(&source);
    for (name, artifact) in CASUALTIES {
        assert!(
            handled.iter().any(|entry| entry == name),
            "/{name} ({artifact}) must be in HANDLED_NAMES, which is what \
             `slash_commands()` registers; found {handled:?}"
        );
    }
}

#[test]
fn the_registration_list_and_the_dispatch_arms_agree() {
    let source = read("crates/lca-cli/src/tui/commands.rs");
    let handled = handled_names(&source);
    let arms = dispatch_arms(&source);
    // The reader must find arms, or this whole file would pass vacuously.
    assert!(
        arms.len() >= 10,
        "the arm reader found {} arms, which is too few to be the real \
         dispatcher - the source shape changed and this guard needs updating",
        arms.len()
    );
    for name in &handled {
        assert!(
            arms.iter().any(|arm| arm == name),
            "/{name} is registered but has no dispatch arm; a listed-but-\
             unhandled name falls through to the extension dispatcher and \
             silently does nothing. Arms: {arms:?}"
        );
    }
}

#[test]
fn every_help_line_belongs_to_a_command_that_runs() {
    // The reverse direction of the bug: a `/help` line for a name nothing
    // answers is an advertisement for a command that cannot run.
    let ui = read("crates/lca-ui/src/chat_commands.rs");
    let cli = read("crates/lca-cli/src/tui/commands.rs");
    let handled = handled_names(&cli);
    let router_arms = {
        let start = ui
            .find("pub(crate) fn dispatch_command")
            .expect("the router exists");
        let body = &ui[start..];
        let end = body.find("\n    }\n").expect("the router has a body");
        let mut names = Vec::new();
        for line in body[..end].lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('"') && trimmed.contains("=>") {
                for (index, part) in trimmed.split('"').enumerate() {
                    if index % 2 == 1 && !part.contains(' ') {
                        names.push(part.to_string());
                    }
                }
            }
        }
        names.sort();
        names.dedup();
        names
    };
    for (name, artifact) in CASUALTIES {
        assert!(
            handled.iter().any(|h| h == name) || router_arms.iter().any(|a| a == name),
            "/{name} ({artifact}) is advertised but nothing dispatches it"
        );
    }
}
