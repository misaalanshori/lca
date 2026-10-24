//! Rows for `/thinking`'s save-as-default and the two pickers' footer hints
//! (gh #396 gap 3).
//!
//! The model half is the reference: `/model` already had `app.models.save`, and
//! this cycle mirrors it. Each row below has a sibling on the model side (or is
//! the sibling), so the two cannot drift.

use super::*;
use std::sync::Arc;

/// A chat whose thinking picker saves through a cell the row can read.
fn chat_with_thinking_save() -> (Chat, Arc<std::sync::Mutex<Option<Option<String>>>>) {
    let mut chat = chat();
    let saved: Arc<std::sync::Mutex<Option<Option<String>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let cell = saved.clone();
    chat.world.options.hooks.save_default_thinking = Some(Arc::new(move |level: Option<&str>| {
        *cell.lock().unwrap_or_else(|p| p.into_inner()) = Some(level.map(str::to_string));
        match level {
            Some(level) => format!("default thinking saved: {level} - new sessions start here"),
            None => "default thinking cleared: new sessions use the provider default".to_string(),
        }
    }));
    (chat, saved)
}

/// Open the `/thinking` picker through the real command path.
fn open_thinking(chat: &mut Chat) {
    for c in "/thinking".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
}

// Verifies: gh #396 gap 3 - **`Ctrl+S` in the thinking picker persists a
// default**, which is what `/model` already did. Before this a thinking choice was
// session-only with no route to a default.
#[test]
fn ctrl_s_in_the_thinking_picker_saves_the_highlighted_level() {
    let (mut chat, saved) = chat_with_thinking_save();
    open_thinking(&mut chat);
    assert!(chat.thinking_picker.is_some(), "the picker opened");

    // Row 0 is `unset`; row 1 is the first level. Move to it.
    chat.handle_key("\x1b[B"); // down
    chat.handle_key("\x13"); // ctrl+s
    let got = saved.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(
        got,
        Some(Some(
            chat.thinking_picker.as_ref().expect("still open").offered[0].clone()
        )),
        "the highlighted level was saved, not the session's current one"
    );
    assert!(
        chat.thinking_picker.is_some(),
        "the picker stays open, pi's shape (the model picker's save does too)"
    );
}

// Verifies: gh #396 gap 3 - **`unset` is savable.** The picker's first row means
// "the provider's own default", and clearing a saved default is a real choice a
// user must be able to make.
#[test]
fn ctrl_s_on_unset_clears_the_saved_default() {
    let (mut chat, saved) = chat_with_thinking_save();
    open_thinking(&mut chat);
    // Row 0 is already selected.
    chat.handle_key("\x13");
    assert_eq!(
        *saved.lock().unwrap_or_else(|p| p.into_inner()),
        Some(None),
        "unset saves as an explicit `None`, which removes the config key"
    );
}

// Verifies: gh #396 rule 7 - **saving does not change the session's value.** The
// `/model` picker's save only writes the default for *new* sessions; a thinking
// save that also switched the current session would be a different (and
// surprising) gesture.
#[test]
fn saving_a_default_thinking_level_leaves_the_session_value_alone() {
    let (mut chat, saved) = chat_with_thinking_save();
    let before = chat.thinking_level();
    open_thinking(&mut chat);
    chat.handle_key("\x1b[B"); // down to the first level
    chat.handle_key("\x13"); // ctrl+s
    assert!(
        saved.lock().unwrap_or_else(|p| p.into_inner()).is_some(),
        "the save reached the host"
    );
    assert_eq!(
        chat.thinking_level(),
        before,
        "the session's level is untouched by saving a default"
    );
}

// Verifies: gh #396 rule 7 - **saving reports what landed.** A save whose notice
// is silent leaves the user unsure whether the config was written.
#[test]
fn saving_a_default_thinking_level_reports_what_happened() {
    let (mut chat, _saved) = chat_with_thinking_save();
    open_thinking(&mut chat);
    chat.handle_key("\x1b[B");
    chat.handle_key("\x13");
    let notice = chat.last_notice().unwrap_or_default();
    assert!(
        notice.contains("default thinking saved"),
        "the notice names the outcome: {notice:?}"
    );
    assert!(
        notice.contains("new sessions"),
        "and what it affects: {notice:?}"
    );
}

// Verifies: gh #396 rule 7 - **a host without the seam refuses rather than
// silently doing nothing.** The `/model` save has the same fallback copy.
#[test]
fn a_host_without_the_save_seam_says_so() {
    let mut chat = chat();
    chat.world.options.hooks.save_default_thinking = None;
    open_thinking(&mut chat);
    chat.handle_key("\x13");
    let notice = chat.last_notice().unwrap_or_default();
    assert!(
        notice.contains("not available"),
        "the refusal names the situation: {notice:?}"
    );
}

// Verifies: gh #396 rule 8 - **both pickers show the save clause.** A picker that
// supports saving must say so; the one that already did must keep saying so. The
// key comes from the live bindings (gh #410), so a rebind reaches the row.
#[test]
fn both_pickers_show_the_save_clause_in_their_hint() {
    let mut chat = chat();
    for command in ["/model", "/thinking"] {
        for c in command.chars() {
            chat.handle_key(&c.to_string());
        }
        chat.handle_key("\r");
        let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
        assert!(
            viewport.contains("set as default"),
            "{command} names the save gesture:\n{viewport}"
        );
        chat.handle_key("\x1b"); // close
    }
}

// Verifies: gh #396 rule 8 + gh #410 - **the hint names the live key.** A user who
// rebinds `app.thinking.save` sees the new key; the row is not a hard-coded
// `ctrl+s` string that a rebind would make a lie.
#[test]
fn the_save_clause_names_the_live_keybinding() {
    let mut bindings = std::collections::BTreeMap::new();
    bindings.insert("app.thinking.save".to_string(), vec!["ctrl+r".to_string()]);
    let manager = KeybindingsManager::with_user_bindings(bindings);
    let hint = crate::chat_overlays::save_hint(&manager, "app.thinking.save", false);
    assert!(
        hint.contains("ctrl+r"),
        "the rebound key reaches the hint: {hint}"
    );
    assert!(!hint.contains("ctrl+s"), "and the old key is gone: {hint}");
}

// Verifies: gh #396 rule 8 - **an unbound action drops the clause** rather than
// naming a key that does nothing.
#[test]
fn an_unbound_save_action_drops_the_save_clause() {
    let mut bindings = std::collections::BTreeMap::new();
    bindings.insert("app.thinking.save".to_string(), Vec::new());
    let manager = KeybindingsManager::with_user_bindings(bindings);
    let hint = crate::chat_overlays::save_hint(&manager, "app.thinking.save", false);
    assert!(
        !hint.contains("set as default"),
        "no dead key is advertised: {hint}"
    );
    assert!(
        hint.contains("enter select"),
        "the rest of the hint survives: {hint}"
    );
}

// Verifies: gh #396 - **the two pickers use the same gesture.** One key for one
// idea; a second binding for the same save would be the drift this repo keeps
// closing.
#[test]
fn the_model_and_thinking_saves_share_one_key() {
    let manager = KeybindingsManager::new();
    assert_eq!(
        manager.display_keys("app.models.save"),
        manager.display_keys("app.thinking.save"),
        "the same key saves a default in both pickers"
    );
    assert_eq!(
        manager.display_keys("app.thinking.save"),
        "ctrl+s",
        "and it is pi's `app.thinking.save` key"
    );
}

// Verifies: gh #396 - **the two `ctrl+s` saves are never live at the same time.**
//
// Both pickers claim one key, which is only safe if they cannot both be open. They
// cannot, and the mechanism is worth naming because it is not the obvious one: an
// open picker **owns the keyboard**, so `/thinking` typed while `/model` is up never
// reaches the command parser - the characters go into the picker's search box, and
// Enter then commits that search and closes the model picker. So the transcript is:
// one picker open, the other's command swallowed, and enter leaves *neither* open
// rather than both.
//
// A future change that let a slash command run through an open picker would put two
// handlers on `ctrl+s` in one frame; this row is what would fail.
#[test]
fn the_two_save_actions_belong_to_mutually_exclusive_pickers() {
    let mut first = chat();
    for c in "/model".chars() {
        first.handle_key(&c.to_string());
    }
    first.handle_key("\r");
    assert!(first.model_picker.is_some(), "the model picker opened");

    // `/thinking` typed into the open model picker does not open a second picker:
    // the keys belong to the one that is up.
    for c in "/thinking".chars() {
        first.handle_key(&c.to_string());
    }
    assert!(
        first.thinking_picker.is_none(),
        "a command typed into an open picker does not open a second one"
    );
    assert!(
        first.model_picker.is_some(),
        "the model picker is still the one owning the keyboard"
    );
    // The two are never both live, which is what makes one key safe.
    assert!(
        !(first.model_picker.is_some() && first.thinking_picker.is_some()),
        "two save pickers must never be open at once"
    );
    first.handle_key("\r");

    // From a fresh chat the same holds with the roles swapped.
    let mut fresh = chat();
    for c in "/thinking".chars() {
        fresh.handle_key(&c.to_string());
    }
    fresh.handle_key("\r");
    assert!(
        fresh.thinking_picker.is_some(),
        "the thinking picker opened"
    );
    for c in "/model".chars() {
        fresh.handle_key(&c.to_string());
    }
    assert!(
        fresh.model_picker.is_none(),
        "and `/model` typed into it does not open a second picker"
    );
    assert!(
        !(fresh.model_picker.is_some() && fresh.thinking_picker.is_some()),
        "two save pickers must never be open at once"
    );
}
