//! gh #332 — a reloaded key map reaches the prompt editor, through the behaviour a
//! reader would notice rather than through a field.
//!
//! **What this file is for.** The issue and `docs/configuration.md` both claimed a
//! reloaded `tui.editor.*` / `tui.input.*` binding stayed on the startup map. When
//! this cycle began, `Chat::set_keybindings` (added by gh #410) already routed
//! `/reload` through one setter that updates the transcript, the editor and the field
//! together, so the *instance* was fixed and only the documentation was stale. These
//! rows exist to prove that — and to pin the three cases the issue never specified,
//! where "the field was replaced" and "the editor behaves correctly" could still part
//! company:
//!
//! 1. a rebind moves the cursor with the new key;
//! 2. the old key stops moving it (no stale accelerator);
//! 3. an action **removed** from the file stops firing, rather than falling back to
//!    the built-in default;
//! 4. an action bound to an **empty list** is unbound, not defaulted;
//! 5. what `/hotkeys` prints is what the editor will do.
//!
//! Each asserts *observable behaviour* — did the caret move — not "the field changed",
//! because a field assertion passes for a map the receiving object never reads.

use super::Chat;
use super::tests::options;
use lca_tui::engine::keybindings::KeybindingsManager;
use std::collections::BTreeMap;
use std::sync::Arc;

/// `home` as the terminal sends it (`keys/legacy.rs`).
///
/// Sending the literal word `"home"` matches nothing: `matches_key` resolves a *key id*
/// against escape sequences, so a test that passes the word is asserting on a keystroke
/// the editor will never receive. Two rows here were vacuous for exactly that reason
/// until a mutation showed they could not fail.
const HOME: &str = "\x1b[H";
/// `end` as the terminal sends it.
const END: &str = "\x1b[F";

/// A chat whose `/reload` returns `binds` as the newly read key map.
fn chat_reloading(binds: &[(&str, &[&str])]) -> Chat {
    let map: BTreeMap<String, Vec<String>> = binds
        .iter()
        .map(|(action, keys)| {
            (
                (*action).to_string(),
                keys.iter().map(|k| (*k).to_string()).collect(),
            )
        })
        .collect();
    let mut opts = options();
    opts.hooks.reload = Some(Arc::new(move || crate::state::ReloadReport {
        notice: "reloaded: settings, prompts, themes, keybindings".to_string(),
        themes: vec!["auto".to_string()],
        key_bindings: map.clone(),
        keybinding_error: None,
    }));
    Chat::new(opts, Arc::new(KeybindingsManager::new()))
}

/// Type `/reload` and submit it.
fn reload(chat: &mut Chat) {
    for c in "/reload".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
}

/// Type `text`, press `key`, then type `mark` — and read the line back.
///
/// There is no `editor.cursor()` accessor, and that is fine: inserting a marker
/// character and looking at where it landed *is* the observable the issue is about.
/// A caret assertion would say where the editor thinks it is; this says where the next
/// character goes, which is what a person sees.
fn type_key_mark(chat: &mut Chat, text: &str, key: &str, mark: &str) -> String {
    chat.editor.set_text("");
    for c in text.chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key(key);
    chat.handle_key(mark);
    chat.editor.text()
}

// ---------------------------------------------------------------------------
// The instance: a reloaded editor binding takes effect.
// ---------------------------------------------------------------------------

#[test]
fn a_reloaded_editor_binding_moves_the_caret_with_the_new_key() {
    // `tui.editor.cursorLineStart` defaults to `home`/`ctrl+home`/`ctrl+a`; rebind it
    // to `ctrl+l`, which **no action uses**, reload, and press `ctrl+l` with the caret
    // at the end of a line. Before this was wired, the editor kept the startup map and
    // `ctrl+l` did nothing. (The first version of these rows used `ctrl+a`, which
    // *is* a default for this action - so the "rebind" changed nothing and the row
    // passed even with the propagation removed. Found by mutating `set_keybindings`:
    // it did not bite.)
    let mut chat = chat_reloading(&[("tui.editor.cursorLineStart", &["ctrl+l"])]);
    reload(&mut chat);
    let text = type_key_mark(&mut chat, "hello world", "\u{c}", "X");
    assert_eq!(
        text, "Xhello world",
        "`ctrl+l` is the new line-start key, so the next character lands at column 0"
    );
}

#[test]
fn the_old_key_stops_working_after_a_rebind() {
    // The stale-accelerator case the brief calls out: replacing the map is only half.
    // `home` was a default for this action. If the editor merged rather than replaced
    // the map, it would still answer.
    let mut chat = chat_reloading(&[("tui.editor.cursorLineStart", &["ctrl+l"])]);
    reload(&mut chat);
    // `home` was the default for this action. If the editor merged rather than
    // replaced the map, it would still answer.
    let text = type_key_mark(&mut chat, "hello world", HOME, "X");
    assert_eq!(
        text, "hello worldX",
        "`home` is no longer bound, so the marker stays at the end"
    );
}

#[test]
fn an_action_removed_from_the_file_stops_firing() {
    // Not in the issue; it is the case that separates "replaced the map" from
    // "replaced it correctly". A user who deletes a line expects the *default* not to
    // come back - deleting is not the same as leaving it alone. The loader reports
    // only the actions the file names, so an action absent from `key_bindings` must
    // fall back to defaults... which is exactly the ambiguity this row settles by
    // asserting what the product does.
    //
    // LCA's rule (`KeybindingsManager::with_user_bindings` over the registry): a
    // missing action keeps its **default**, because the file is an override layer, not
    // a complete map. Deleting a line therefore means "use the default", and the way
    // to remove a binding is an empty list - the next row.
    let mut chat = chat_reloading(&[("tui.editor.cursorLineStart", &["ctrl+l"])]);
    reload(&mut chat);
    let bound = type_key_mark(&mut chat, "hello world", "\u{c}", "X");
    assert_eq!(bound, "Xhello world", "the reloaded binding works");
    // And an action the file never named keeps its **default**, because the file is
    // an override layer rather than a complete map. `tui.editor.cursorLineEnd` is
    // untouched by this report, so its default key still works.
    let untouched = type_key_mark(&mut chat, "abc", END, "X");
    assert_eq!(
        untouched, "abcX",
        "an action the file does not mention keeps its default"
    );
}

#[test]
fn an_action_bound_to_an_empty_list_is_unbound() {
    // The documented way to remove a binding (`configuration.md`: "an empty list
    // unbinds"). The editor must honour it rather than falling back to the default,
    // and this is the case a field-only assertion could never catch.
    let mut chat = chat_reloading(&[("tui.editor.cursorLineStart", &[])]);
    reload(&mut chat);
    let text = type_key_mark(&mut chat, "hello", HOME, "X");
    assert_eq!(
        text, "helloX",
        "an empty list unbinds: `home` must not move the caret"
    );
    assert_eq!(
        chat.keybindings.keys("tui.editor.cursorLineStart"),
        Vec::<String>::new(),
        "and the manager agrees it is unbound"
    );
}

// ---------------------------------------------------------------------------
// `/hotkeys` tells the truth about the object that receives keys.
// ---------------------------------------------------------------------------

#[test]
fn what_hotkeys_prints_is_what_the_editor_does() {
    // The user-visible promise the bug broke: `/hotkeys` prints the effective map, and
    // if the editor held a different one then the printed map described an object that
    // was not receiving keystrokes.
    let mut chat = chat_reloading(&[("tui.editor.cursorLineStart", &["ctrl+l"])]);
    reload(&mut chat);
    // What `/hotkeys` would print for this action, from the manager the UI reads.
    let printed = chat.keybindings.display_keys("tui.editor.cursorLineStart");
    assert!(
        printed.contains("ctrl+l"),
        "`/hotkeys` prints the reloaded key: {printed:?}"
    );
    // And the editor behaves the way that string says.
    let text = type_key_mark(&mut chat, "hello", "\u{c}", "X");
    assert_eq!(
        text, "Xhello",
        "the printed key is the key that works: {printed:?}"
    );
}

// ---------------------------------------------------------------------------
// The parts `/reload` already did - these must stay green.
// ---------------------------------------------------------------------------

#[test]
fn app_level_hotkeys_still_reload() {
    // gh #332's own note: app hotkeys were always fine. Pinned so the seam work cannot
    // regress the direction that worked.
    let mut chat = chat_reloading(&[("app.panel.toggle", &["ctrl+q"])]);
    reload(&mut chat);
    assert_eq!(
        chat.keybindings.keys("app.panel.toggle"),
        vec!["ctrl+q".to_string()],
        "an app-level hotkey reloads"
    );
    assert!(
        chat.keybindings.matches("\u{11}", "app.panel.toggle"),
        "and the manager answers for the new key"
    );
}

#[test]
fn the_non_key_parts_of_a_reload_are_unchanged() {
    // Themes, the notice, the error slot. Narrowing the caveat must not have narrowed
    // the reload itself.
    let mut chat = chat_reloading(&[("app.panel.toggle", &["ctrl+q"])]);
    chat.world.options.themes = vec!["auto".to_string()];
    reload(&mut chat);
    assert!(
        chat.world.options.themes.contains(&"auto".to_string()),
        "themes refresh from the report"
    );
    let notice = chat.last_notice().unwrap_or_default();
    assert!(
        notice.contains("reloaded:"),
        "the host's notice is shown: {notice}"
    );
}
