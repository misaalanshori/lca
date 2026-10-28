//! Gh #374 gap 4, the Windows half: right-button paste in fullscreen.
//!
//! ## Two defects in one place
//!
//! `handle_mouse` had **no** right-button arm. Because the press arm tests only
//! `mouse.press`, a right press (`bits & 3 == 2`) fell straight into it and
//! **started a text selection** — a right-click began a left-drag. That is worse
//! than doing nothing: the next motion extends a highlight nobody asked for, and
//! on Windows the user's expectation is that the button pastes.
//!
//! ## The guard, and why it is a parameter rather than a `cfg!`
//!
//! pi gates on `process.platform !== "win32"` plus
//! `process.env.TERM_PROGRAM?.toLowerCase() === "vscode"`
//! (`tui-alt-screen.ts:1018`). A `cfg!(windows)` test cannot be exercised on this
//! box, so the decision is a pure function of `(is_windows, TERM_PROGRAM)`:
//! production passes `cfg!(windows)` and the environment, and the rows below drive
//! both branches on Linux. **That is a row-level receipt, not a live Windows
//! check** — this box has no Windows console.
//!
//! The VS Code exclusion is the one that prevents a regression: VS Code's
//! integrated terminal has its own right-click behaviour, and shadowing it would
//! break the editor it is running inside.
//!
//! Split out of `alt_screen.rs` for the workspace's 1,200-line ceiling (gate 11).

use super::*;

/// One right-button press (`SGR` button code 2, no modifiers) at 1-based `(x, y)`.
fn right_press(r: &mut AltScreenRenderer, x: u16, y: u16) -> bool {
    r.handle_mouse(SgrMouse {
        bits: 2,
        x,
        y,
        press: true,
    })
}

/// A frame with text on it, so a stray selection would be visible.
fn renderer() -> AltScreenRenderer {
    let mut r = AltScreenRenderer::new();
    r.previous = vec![
        "hello world here".to_string(),
        "second row here".to_string(),
    ];
    r.width = 40;
    r.height = 2;
    r
}

// Verifies: gh #374 gap 4 - the gating decision is right on every combination.
//
// This is the row that stands in for a live Windows check, so it covers all four
// cells rather than just the one production hits today.
#[test]
fn right_click_paste_is_allowed_only_on_windows_outside_vscode() {
    // Windows, no TERM_PROGRAM: the case the feature exists for.
    assert!(right_click_paste_allowed(true, None));
    // Windows Terminal sets TERM_PROGRAM=Windows_Terminal: still allowed.
    assert!(right_click_paste_allowed(true, Some("Windows_Terminal")));
    // conhost usually sets nothing, covered above; a random value is allowed too.
    assert!(right_click_paste_allowed(true, Some("iTerm.app")));
    // **The exclusion**: VS Code's integrated terminal keeps its own behaviour.
    assert!(
        !right_click_paste_allowed(true, Some("vscode")),
        "VS Code owns right-click in its integrated terminal"
    );
    // pi lowercases before comparing, so a differently-cased value must also be
    // excluded - otherwise the guard is bypassable by the environment's spelling.
    assert!(
        !right_click_paste_allowed(true, Some("VSCode")),
        "the comparison is case-insensitive, as pi's is"
    );
    // But only an exact (case-folded) match is VS Code: a different program that
    // merely mentions it is the user's own terminal and keeps the feature.
    assert!(
        right_click_paste_allowed(true, Some("VSCode.app")),
        "the guard is an exact match, not a prefix or substring test"
    );
    // Off Windows, never - macOS and Linux terminals have their own paste keys and
    // a right-click that pasted there would be a surprise, not a convention.
    assert!(!right_click_paste_allowed(false, None));
    assert!(!right_click_paste_allowed(false, Some("Windows_Terminal")));
    assert!(!right_click_paste_allowed(false, Some("vscode")));
}

// Verifies: gh #374 gap 4 - **a right-button press does not start a selection**.
//
// The old behaviour: `bits & 3 == 2` reached the press arm, which begins a
// char-granularity selection, so a right-click on Windows began a highlight that
// the next motion extended. The row asserts both halves - the flag is set *and*
// nothing got selected.
#[test]
fn a_right_press_requests_a_paste_and_does_not_start_a_selection() {
    let mut r = renderer();
    // Allow the feature for this renderer the way production would on Windows.
    r.set_right_click_paste_allowed(true);
    assert!(right_press(&mut r, 3, 1), "the press is consumed");
    assert!(
        r.take_right_click_paste(),
        "the paste intent is what a right press records"
    );
    assert!(
        r.selected_text().is_empty(),
        "and no selection started: {:?}",
        r.selected_text()
    );
    assert!(
        !r.selection.press_active,
        "nor a press that a later motion would extend"
    );
    // A second take is empty: the intent is consumed once, like the click cell.
    assert!(!r.take_right_click_paste());
}

// Verifies: gh #374 gap 4 - a right-button **release** does not also request a
// paste, and does not end a gesture it never started.
//
// A terminal sends press then release; only the press is the paste. Without this
// row a release-triggered implementation would paste twice or paste on the way up
// from an unrelated drag.
#[test]
fn a_right_release_requests_nothing() {
    let mut r = renderer();
    r.set_right_click_paste_allowed(true);
    r.handle_mouse(SgrMouse {
        bits: 2,
        x: 3,
        y: 1,
        press: false,
    });
    assert!(
        !r.take_right_click_paste(),
        "the release is not a second paste"
    );
}

// Verifies: gh #374 gap 4 - **the VS Code exclusion, end to end through
// `handle_mouse`.** The guard is in the pure function above; this row proves the
// renderer actually consults it rather than always setting the flag.
#[test]
fn a_right_press_is_ignored_where_paste_is_not_allowed() {
    let mut r = renderer();
    r.set_right_click_paste_allowed(false);
    // Not consumed: with the feature disabled the gesture falls through, so an
    // embedder that wants its own right-click handling still sees the event.
    assert!(
        !right_press(&mut r, 3, 1),
        "the renderer does not claim a gesture it will not act on"
    );
    assert!(!r.take_right_click_paste(), "and records no paste intent");
}

// Verifies: gh #374 gap 4 - a left press is untouched by the new arm.
//
// The right-button arm has to sit before the press arm, so the cheap way to get
// the fix wrong is to catch every press. The left-click rows elsewhere cover the
// full gesture; this is the adjacent guard.
#[test]
fn a_left_press_still_starts_a_selection() {
    let mut r = renderer();
    r.set_right_click_paste_allowed(true);
    r.handle_mouse(SgrMouse {
        bits: 0,
        x: 3,
        y: 1,
        press: true,
    });
    assert!(!r.take_right_click_paste(), "a left press is not a paste");
    assert!(
        r.selection.press_active,
        "it is a selection press, as before"
    );
}

// Verifies: gh #374 gap 4 - a right press **while a selection drag is in flight**
// is not a paste either, because the drag owns the pointer.
//
// The scrollbar-drag rule already establishes this shape (`a scrollbar drag
// suppresses selection`); a right press mid-drag pasting would interrupt a gesture
// the user is still performing.
#[test]
fn a_right_press_during_a_drag_does_not_paste() {
    let mut r = renderer();
    r.set_right_click_paste_allowed(true);
    r.handle_mouse(SgrMouse {
        bits: 0,
        x: 3,
        y: 1,
        press: true,
    });
    r.handle_mouse(SgrMouse {
        bits: 32,
        x: 8,
        y: 1,
        press: true,
    });
    assert!(r.selection.press_active, "a drag is in flight");
    assert!(
        !right_press(&mut r, 10, 1),
        "the drag still owns the pointer"
    );
    assert!(!r.take_right_click_paste(), "so no paste was requested");
}
