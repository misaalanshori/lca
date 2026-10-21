//! Guards for gh #443: `/help`'s description table and the router's
//! allowlist must agree with what actually dispatches.
//!
//! Ceiling split from `chat_commands.rs` (gate 11). Behaviour unchanged;
//! these rows read the router's source and the real registration shape, so
//! they live in their own module rather than growing the file they guard.

use crate::state::UiOptions;
use lca_tui::engine::keybindings::KeybindingsManager;
use std::sync::{Arc, Mutex};

use crate::Chat;
use crate::chat_commands::command_help;

/// Every arm the UI router matches, read from the source so the guard
/// cannot be satisfied by editing the list it checks.
fn router_arms() -> Vec<String> {
    let source = include_str!("chat_commands.rs");
    let start = source
        .find("pub(crate) fn dispatch_command")
        .expect("the router exists");
    let body = &source[start..];
    let end = body.find("\n    }\n").expect("the router has a body");
    let mut arms = Vec::new();
    for line in body[..end].lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('"') || !trimmed.contains("=>") {
            continue;
        }
        for (index, part) in trimmed.split('"').enumerate() {
            // Odd indices are inside quotes.
            if index % 2 == 1 && !part.contains(' ') {
                arms.push(part.to_string());
            }
        }
    }
    arms.sort();
    arms.dedup();
    arms
}

// Verifies: gh #443 - every command the router answers carries a `/help`
// line, so `/help` can never under-advertise a working command.
#[test]
fn every_routed_command_has_a_help_line() {
    // `/quit` is `/exit`'s spelling and `/marketplace` is `/extensions`'s;
    // one description serves each pair, and the table names the canonical
    // spelling.
    let aliases = ["quit", "marketplace"];
    for arm in router_arms() {
        if aliases.contains(&arm.as_str()) {
            continue;
        }
        assert!(
            !command_help(&format!("/{arm}")).is_empty(),
            "/{arm} routes but /help says nothing about it; a bare name is \
             how /label and /jump stayed invisible"
        );
    }
}

// Verifies: gh #443 - the guard's reader works, so the row above cannot
// pass vacuously on an empty arm list.
#[test]
fn the_router_arm_reader_finds_arms() {
    let arms = router_arms();
    // `login` is answered by an `if name == "login"` guard above the match,
    // so it is deliberately not an arm; the reader only claims arms.
    for expected in ["help", "tree", "extensions", "label", "reload"] {
        assert!(
            arms.iter().any(|arm| arm == expected),
            "the arm reader missed /{expected}; arms seen: {arms:?}"
        );
    }
}

// Verifies: gh #443 - the three commands that were unreachable have their
// descriptions, so `/help` advertising them is now true.
#[test]
fn the_reachable_now_advertise_accurately() {
    for (name, needle) in [
        ("/bug", "diagnostic bundle"),
        ("/export", "session out"),
        ("/import", "JSONL"),
    ] {
        let text = command_help(name);
        assert!(
            text.contains(needle),
            "/help for {name} should describe it ({needle}): {text:?}"
        );
    }
}

/// A chat whose `slash_commands` list is the shape the host hands over, so a
/// name missing from it is dropped exactly as it was in the bug. `reached`
/// records what got past the router's gate.
fn chat_with(registered: Vec<String>, reached: Arc<Mutex<Vec<(String, String)>>>) -> Chat {
    let options = UiOptions {
        slash_commands: registered,
        invoke_command: Arc::new(move |name: &str, argument: &str| {
            reached
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((name.to_string(), argument.to_string()));
            lca_protocol::CommandEffect::ShowWidget(format!("ran {name}"))
        }),
        ..crate::chat::tests::options()
    };
    Chat::new(options, Arc::new(KeybindingsManager::new()))
}

fn submit(chat: &mut Chat, line: &str) {
    for c in line.chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
}

// Verifies: gh #443 - `/bug` reaches the host once registered, so the
// handler's own notice is what the user sees (not "unknown command").
#[test]
fn bug_reaches_the_host_once_registered() {
    let reached = Arc::new(Mutex::new(Vec::new()));
    let mut chat = chat_with(vec!["/bug".into()], reached.clone());
    submit(&mut chat, "/bug");
    assert_eq!(
        reached.lock().unwrap_or_else(|p| p.into_inner()).as_slice(),
        &[("bug".to_string(), String::new())],
        "/bug must reach the host: {:?}",
        chat.last_notice()
    );
}

// Verifies: gh #443 - `/export html` keeps its argument all the way to the
// handler, so the transcript-page variant is reachable too.
#[test]
fn export_passes_its_argument_through() {
    let reached = Arc::new(Mutex::new(Vec::new()));
    let mut chat = chat_with(vec!["/export".into()], reached.clone());
    submit(&mut chat, "/export html");
    assert_eq!(
        reached.lock().unwrap_or_else(|p| p.into_inner()).as_slice(),
        &[("export".to_string(), "html".to_string())]
    );
}

// Verifies: gh #443 - `/import <path>` reaches the handler with its path.
#[test]
fn import_passes_its_path_through() {
    let reached = Arc::new(Mutex::new(Vec::new()));
    let mut chat = chat_with(vec!["/import".into()], reached.clone());
    submit(&mut chat, "/import /tmp/x.jsonl");
    assert_eq!(
        reached.lock().unwrap_or_else(|p| p.into_inner()).as_slice(),
        &[("import".to_string(), "/tmp/x.jsonl".to_string())]
    );
}

// Verifies: gh #443 - and the shape that WAS the bug: unregistered, a name is
// refused rather than silently falling through. This row would have caught it.
#[test]
fn an_unregistered_name_is_still_refused() {
    let reached = Arc::new(Mutex::new(Vec::new()));
    let mut chat = chat_with(vec!["/help".into()], reached.clone());
    submit(&mut chat, "/bug");
    assert!(
        reached.lock().unwrap_or_else(|p| p.into_inner()).is_empty(),
        "an unregistered name must not reach the host"
    );
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("unknown command /bug"),
        "and it says so"
    );
}
