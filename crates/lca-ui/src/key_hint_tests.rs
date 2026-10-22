//! gh #410: a key hint names the key the interface will actually accept.
//!
//! The defect: `key_text(action)`'s doc promised hints that "cannot lie when the
//! interface is rebound", and it built a **fresh default** `KeybindingsManager`
//! on every call. All fourteen UI sites used that free function, so a user who
//! rebound `app.tools.expand` saw `ctrl+o` in every truncation hint while the key
//! itself no longer worked: the label lied in exactly the situation the comment
//! promised it could not.
//!
//! These rows pin the contract from the outside - render through a `Transcript`
//! that owns a **live** manager and assert what a reader sees. They are
//! deliberately not unit tests of a formatting helper alone: the bug lived in the
//! wiring, so the rows that would have caught it have to cross it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeMap;
use std::sync::Arc;

use lca_tui::engine::keybindings::KeybindingsManager;

use crate::theme::{Role, Theme};
use crate::transcript::{ToolStatus, Transcript};

/// A manager with one action rebound, the way `/reload` builds one.
fn rebound(action: &str, keys: &[&str]) -> Arc<KeybindingsManager> {
    let mut user = BTreeMap::new();
    user.insert(
        action.to_string(),
        keys.iter().map(|key| (*key).to_string()).collect(),
    );
    Arc::new(KeybindingsManager::with_user_bindings(user))
}

/// A tool card whose result is long enough to collapse, so it renders a hint.
fn collapsed_card(manager: Option<Arc<KeybindingsManager>>) -> Transcript {
    let mut transcript = Transcript::new();
    if let Some(manager) = manager {
        transcript.set_keybindings(manager);
    }
    transcript.start_tool("shell", r#"{"command":"seq 1 200"}"#);
    // Long enough that the card collapses and renders its expand hint: the preview
    // counts visual rows, so a three-line result has nothing to omit.
    let body: String = (1..=200).map(|n| format!("{n}\n")).collect();
    transcript.finish_tool(ToolStatus::Ok, Some(body));
    transcript
}

// ------------------------------------------------------------------ rebound

// Verifies: gh #410 - the truncation hint names the **rebound** key.
//
// This is the defect's own row: rebind `app.tools.expand` to `ctrl+e` and the
// tool card's "… to expand" hint must say `ctrl+e`, not the default `ctrl+o`.
#[test]
fn a_rebound_expand_key_reaches_the_tool_card_hint() {
    let theme = Theme::colored();

    // Baseline: the default manager renders the default key.
    let default = collapsed_card(None).render(80, &theme).join("\n");
    assert!(
        default.contains("ctrl+o") && default.contains("to expand"),
        "the default binding is the baseline:\n{default}"
    );

    // Now the live manager rebinds it, and the render must follow.
    let live = collapsed_card(Some(rebound("app.tools.expand", &["ctrl+e"])))
        .render(80, &theme)
        .join("\n");
    assert!(
        live.contains("ctrl+e"),
        "the hint names the rebound key:\n{live}"
    );
    assert!(
        !live.contains("ctrl+o"),
        "and never the dead default:\n{live}"
    );
}

// Verifies: gh #410 - the thinking-run hint follows the live manager too.
//
// The fourteen sites span several render paths, so the defect needs a row per
// surface rather than one row for whichever call site was easiest to reach.
#[test]
fn a_rebound_thinking_key_reaches_the_reasoning_hint() {
    let theme = Theme::plain();
    let hint = crate::transcript::render_key_hint(
        &theme,
        &rebound("app.thinking.toggle", &["ctrl+t"]),
        "app.thinking.toggle",
        Some("to expand"),
    );
    assert!(
        hint.contains("ctrl+t"),
        "the reasoning hint names the rebound key: {hint:?}"
    );
    assert!(!hint.contains("ctrl+o"), "and never the default: {hint:?}");
}

// ---------------------------------------------------------------- formatting

// Verifies: gh #410 design point 4 - the key is dim and the description muted,
// in the theme's own roles (never literal SGR).
#[test]
fn the_hint_paints_the_key_dim_and_the_description_muted() {
    let theme = Theme::colored();
    let default = KeybindingsManager::new();
    let hint =
        crate::transcript::render_key_hint(&theme, &default, "app.tools.expand", Some("to expand"));
    // The description is painted with its leading space, matching pi's
    // `theme.fg("muted", ` ${description}`)` - so the assertion carries it too.
    for (role, text) in [(Role::Dim, "ctrl+o"), (Role::Muted, " to expand")] {
        let painted = theme.role(role)(text);
        assert!(
            hint.contains(&painted),
            "the hint carries {role:?} for {text:?}: {hint:?}"
        );
    }
    assert_ne!(
        theme.role(Role::Dim)("x"),
        theme.role(Role::Muted)("x"),
        "dim and muted are different roles, so the two halves read differently"
    );

    // NFR-28: the plain theme emits no SGR and the words are still there.
    let hint = crate::transcript::render_key_hint(
        &Theme::plain(),
        &default,
        "app.tools.expand",
        Some("to expand"),
    );
    assert!(
        hint.contains("ctrl+o") && hint.contains("to expand"),
        "{hint:?}"
    );
    assert!(
        !hint.contains('\x1b'),
        "the plain theme emits no escape: {hint:?}"
    );
}

// Verifies: gh #410 design point 6 - multiple bindings and a chord render in full.
//
// pi's `formatKeys` joins a multi-binding list with `/` and keeps a chord's parts
// with `+`; the manager already had the vocabulary, and the old free function
// never asked it.
#[test]
fn multiple_bindings_and_chords_render_in_full_form() {
    let theme = Theme::plain();
    // Two keys for one action: joined with `/`.
    let hint = crate::transcript::render_key_hint(
        &theme,
        &rebound("app.prompt.previous", &["shift+up", "ctrl+p"]),
        "app.prompt.previous",
        None,
    );
    assert!(
        hint.contains("shift+up/ctrl+p"),
        "multiple bindings join with `/`: {hint:?}"
    );
    // A chord keeps its `+` parts rather than being truncated to its first key.
    let chord = crate::transcript::render_key_hint(
        &theme,
        &rebound("app.tools.expand", &["ctrl+x+ctrl+e"]),
        "app.tools.expand",
        None,
    );
    assert!(
        chord.contains("ctrl+x+ctrl+e"),
        "a chord keeps its form: {chord:?}"
    );
}

// Verifies: gh #410 design point 5 - an unbound action renders **no** hint.
//
// `[]` is an explicit unbind in the manager's vocabulary. A hint reading
// " to expand" with a leading space, or "· " with nothing after it, is the
// failure this row exists to prevent.
#[test]
fn an_unbound_action_renders_no_hint_at_all() {
    let theme = Theme::plain();
    let hint = crate::transcript::render_key_hint(
        &theme,
        &rebound("app.tools.expand", &[]),
        "app.tools.expand",
        Some("to expand"),
    );
    assert!(hint.is_empty(), "an unbind renders nothing: {hint:?}");

    // And through a real transcript: the card's marker must not leave a dangling
    // key slot behind.
    let joined = collapsed_card(Some(rebound("app.tools.expand", &[])))
        .render(80, &theme)
        .join("\n");
    assert!(
        !joined.contains("to expand"),
        "with no key bound the hint is omitted, not left dangling:\n{joined}"
    );
    assert!(
        !joined.contains("ctrl+o"),
        "and never falls back to the default:\n{joined}"
    );
}

// Verifies: gh #410 design point 3 - a `/reload` rebind repaints the hint.
//
// This is the acceptance criterion: the **next** frame shows the new key, with no
// restart and no re-construction of the transcript.
#[test]
fn a_reload_rebind_repaints_the_hint_without_a_restart() {
    let theme = Theme::plain();
    let mut transcript = collapsed_card(None);

    let before = transcript.render(80, &theme).join("\n");
    assert!(before.contains("ctrl+o"), "before the reload: {before}");

    // What `/reload` installs.
    transcript.set_keybindings(rebound("app.tools.expand", &["ctrl+e"]));
    let after = transcript.render(80, &theme).join("\n");
    assert!(
        after.contains("ctrl+e") && !after.contains("ctrl+o"),
        "the repainted frame follows the rebind: {after}"
    );

    // A second rebind changes again: the render cache must not serve stale rows.
    transcript.set_keybindings(rebound("app.tools.expand", &["ctrl+g"]));
    let again = transcript.render(80, &theme).join("\n");
    assert!(
        again.contains("ctrl+g") && !again.contains("ctrl+e"),
        "a cached frame must not outlive the binding change: {again}"
    );
}
