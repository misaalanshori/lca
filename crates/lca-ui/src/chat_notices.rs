//! The text behind `/hotkeys`, `/help`, and `/changelog`.
//!
//! Split out of `chat_commands.rs` when that file crossed gate 11's 1,200-line
//! ceiling. These three build strings from a keybinding manager, the command list,
//! and the embedded changelog - none of them reads `Chat` state, which is why they
//! move cleanly.
//!
//! Gh #410 reaches `/hotkeys` too: its rows come from the **live** manager, so a
//! rebind is what the table prints.

use lca_tui::engine::keybindings::KeybindingsManager;

use super::chat_commands::CHANGELOG_TEXT;

/// The `/hotkeys` table: every action that is bound, its keys, and what it does.
pub(crate) fn hotkeys_notice(kb: &KeybindingsManager) -> String {
    let mut lines = vec!["keys:".to_string()];
    for (action, keys) in kb.resolved_bindings() {
        if keys.is_empty() {
            continue;
        }
        let description = kb.description(&action).unwrap_or("");
        lines.push(format!("  {} - {description}", keys.join(", ")));
    }
    // R2: the main screen never captures the mouse; `/fullscreen` does, so the
    // terminal's own bypass is worth naming for the fullscreen case.
    lines.push(
        "  Shift+click (or your terminal's bypass key) selects text in fullscreen".to_string(),
    );
    lines.join("\n")
}

/// The `[Unreleased]` section of the embedded changelog, for `/changelog`.
pub(crate) fn latest_changelog_section() -> String {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in CHANGELOG_TEXT.lines() {
        if line.starts_with("## [") {
            if in_section {
                break;
            }
            in_section = !line.contains("[Unreleased]");
            if in_section {
                out.push(line);
            }
            continue;
        }
        if in_section {
            out.push(line);
        }
    }
    out.join("\n").trim().to_string()
}
