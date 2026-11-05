//! Gh #403 gap 4: `Ctrl+Z` reaches the loop as a suspend request, in all three
//! spellings a terminal can send it.
//!
//! Three spellings, one action - the same rule `is_quit` follows for `Ctrl+C`
//! during startup (`run_early_input`): the raw C0 byte for a terminal without the
//! keyboard protocol, Kitty's CSI-u, and xterm's `modifyOtherKeys`. Missing one is
//! a live-only bug, because which one arrives depends on what the terminal
//! negotiated.
//!
//! The key is answered by the interface and *carried out* by the loop - handing the
//! tty back is `leave` + `stop`, and only the loop holds the screen. So the row's
//! subject is the action, not the suspend.

use super::chat;

// Verifies: gh #403 gap 4 - the key is registered, and every dialect the terminal
// may use maps to the same action.
#[test]
fn every_spelling_of_ctrl_z_asks_to_suspend() {
    for data in [
        "\u{1a}",         // the raw C0 byte
        "\x1b[122;5u",    // Kitty CSI-u
        "\x1b[27;5;122~", // xterm modifyOtherKeys
    ] {
        let mut chat = chat();
        assert_eq!(
            chat.handle_key(data),
            crate::state::Action::Suspend,
            "{data:?} must ask to suspend"
        );
    }
}

// Verifies: gh #403 gap 4 - **the prompt does not stand in the way.** A suspend is
// about the process, not the buffer, so a composer full of text suspends rather
// than being swallowed as a printable character.
//
// The negative half: the text is unchanged, so the key never doubles as an edit.
#[test]
fn ctrl_z_suspends_with_text_in_the_prompt() {
    let mut chat = chat();
    for ch in "draft".chars() {
        chat.handle_key(&ch.to_string());
    }
    assert_eq!(chat.handle_key("\u{1a}"), crate::state::Action::Suspend);
    assert_eq!(
        chat.editor.text(),
        "draft",
        "and the draft survives - the key is not an edit"
    );
}

// Verifies: gh #403 gap 4 - the binding is in the registry, so `/hotkeys` lists it
// and an embedder can rebind it.
//
// A hard-coded byte test in `global_key` would pass the two rows above and leave
// the key invisible in `/hotkeys` and un-rebindable, which is the `key_text` lie gh
// #410 removed in the other direction.
#[test]
fn the_suspend_binding_is_in_the_registry_and_shows_in_hotkeys() {
    let chat = chat();
    let kb = chat.keybindings_manager();
    assert_eq!(
        kb.primary_key("app.suspend"),
        "ctrl+z",
        "the documented key"
    );
    let notice = crate::chat_notices::hotkeys_notice(kb);
    assert!(
        notice.contains("ctrl+z") && notice.contains("Suspend"),
        "/hotkeys lists it with its description: {notice}"
    );
}

// Verifies: gh #403 gap 4 - a rebind is honoured, so the registry is the one answer
// rather than a second hard-coded byte test.
#[test]
fn a_rebound_suspend_key_is_honoured() {
    let mut chat = chat();
    let mut overrides = std::collections::BTreeMap::new();
    overrides.insert("app.suspend".to_string(), vec!["ctrl+y".to_string()]);
    chat.set_keybindings(std::sync::Arc::new(
        lca_tui::engine::keybindings::KeybindingsManager::with_user_bindings(overrides),
    ));
    assert_eq!(
        chat.handle_key("\u{19}"),
        crate::state::Action::Suspend,
        "ctrl+y is the new binding"
    );
    assert_ne!(
        chat.handle_key("\u{1a}"),
        crate::state::Action::Suspend,
        "and the old key no longer suspends"
    );
}
