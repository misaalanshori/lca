//! Gh #400 gaps 2 and 3, from the interface's side: the header entry expands on the
//! existing key, and it survives `/new`.
//!
//! The CLI-side rows (`crates/lca-cli/tests/startup_header.rs`) pin the header's
//! *content*: one compact line, the table behind it, empty sections at zero rows,
//! and the collision notice surviving `quietStartup`. These rows pin the *lifecycle*
//! - the part a content row cannot see:
//!
//! - the expand key is `app.tools.expand` (the key the interface already uses), not
//!   a second binding;
//! - the state toggles both ways;
//! - and the header comes back after `clear()`, which is the issue's "a header that
//!   resets on `/new` is worse than one that never expanded".
//!
//! Verifies: gh #400 gaps 2 and 3, FR-EXT-11.

use super::*;
use super::{options, strip};

/// A chat whose startup header has both states, as a launch would push it.
fn chat_with_header() -> Chat {
    let mut opts = options();
    opts.initial_header = (
        vec!["lca 0.6.0 (abi 0.6)\u{2003}4 extensions, 0 skills, 0 templates".to_string()],
        vec![
            "lca 0.6.0 (abi 0.6)".to_string(),
            "context: /w/.lca/AGENTS.md".to_string(),
            "4 extensions".to_string(),
        ],
    );
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.screen_mode = true;
    chat
}

fn frame(chat: &Chat) -> String {
    strip(&chat.viewport(100, 30, 0)).join("\n")
}

// Verifies: gh #400 gap 2 - **the default frame is the compact line**, and it does
// not contain the context path or the expanded table's rows. The launch is not a
// wall.
#[test]
fn the_header_starts_compact() {
    let chat = chat_with_header();
    let text = frame(&chat);
    assert!(
        text.contains("4 extensions"),
        "the compact counts show:\n{text}"
    );
    assert!(
        !text.contains("/w/.lca/AGENTS.md"),
        "and the context path does not, until expanded:\n{text}"
    );
}

// Verifies: gh #400 gap 2 - **the expand key is the interface's existing one.**
// `app.tools.expand` (`ctrl+o`) already expands a tool card; the header uses the
// same binding, so there is no second chord to learn and a rebind reaches both.
#[test]
fn the_existing_expand_key_reveals_the_table() {
    let mut chat = chat_with_header();
    // Resolve the key from the live manager rather than hardcoding `ctrl+o` - that
    // is the rule the whole codebase follows for a key a user presses (gh #410).
    let key = chat.keybindings.primary_key("app.tools.expand");
    assert_eq!(key, "ctrl+o", "the shipped binding is the tool-expand key");
    chat.handle_key("\u{0f}");
    let text = frame(&chat);
    assert!(
        text.contains("/w/.lca/AGENTS.md"),
        "the expanded table's context row shows:\n{text}"
    );
    // And back again: the toggle is a toggle, not a one-way reveal.
    chat.handle_key("\u{0f}");
    let text = frame(&chat);
    assert!(
        !text.contains("/w/.lca/AGENTS.md"),
        "and pressing it again collapses:\n{text}"
    );
}

// Verifies: gh #400 gap 2 - **the expanded state survives `/new`.** Opening a fresh
// session clears the transcript; the header must be re-pushed with the expand state
// carried across. A header that resets here makes the reader re-expand on every new
// session, which is worse than never having expanded at all.
#[test]
fn the_expanded_state_survives_a_new_session() {
    let mut chat = chat_with_header();
    chat.handle_key("\u{0f}");
    assert!(frame(&chat).contains("/w/.lca/AGENTS.md"), "expanded first");
    // `/new` is the tab path: it replays a fresh session's (empty) records, which
    // is what clears the transcript.
    let created = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = created.clone();
    chat.world.options.hooks.new_session = Some(std::sync::Arc::new(move || {
        let id = "fresh".to_string();
        sink.lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(id.clone());
        Some(id)
    }));
    chat.world.options.hooks.switch_session = Some(std::sync::Arc::new(|_id: &str| {
        Some(Vec::<lca_protocol::Record>::new())
    }));
    chat.open_tab();
    let text = frame(&chat);
    assert!(
        text.contains("/w/.lca/AGENTS.md"),
        "the header is still expanded after the switch:\n{text}"
    );
}

// Verifies: gh #400 gap 3 - **a collision notice reaches the pane on the compact
// line**, which is where a launch shows it. The CLI rows pin the string; this pins
// that the interface paints what it was handed.
#[test]
fn a_collision_notice_paints_on_the_compact_header() {
    let mut opts = options();
    opts.initial_header = (
        vec![
            "lca 0.6.0 (abi 0.6)\u{2003}4 extensions".to_string(),
            "extension \"formatter\" collision: \u{2713} project / \u{2717} user (skipped)"
                .to_string(),
        ],
        vec!["lca 0.6.0 (abi 0.6)".to_string()],
    );
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.screen_mode = true;
    let text = frame(&chat);
    assert!(
        text.contains("formatter") && text.contains("skipped"),
        "the notice is on screen without expanding anything:\n{text}"
    );
}

// Verifies: gh #400 gap 3 / NFR-28 - **the notice is a word, not colour alone.**
// The ✓/✗ marks are a glyph pair; `skipped` is what carries the meaning when the
// glyphs are missing (a dumb terminal, a font without them).
#[test]
fn the_collision_notice_says_skipped_in_words() {
    let mut opts = options();
    opts.initial_header = (
        vec!["extension \"x\" collision: \u{2713} a / \u{2717} b (skipped)".to_string()],
        vec![],
    );
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.screen_mode = true;
    let text = strip(&chat.render(100)).join("\n");
    assert!(
        text.contains("skipped"),
        "the word carries the meaning:\n{text}"
    );
}
