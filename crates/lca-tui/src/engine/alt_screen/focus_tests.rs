//! Gh #374 gap 2: `FOCUS_OUT` must not wipe a **finished** selection.
//!
//! ## The defect, and why it was invisible
//!
//! The classic flow — drag-select a command, release the mouse, `Alt+Tab` to a
//! browser to paste — sends `ESC [ O` **after** the release. The old arm was
//!
//! ```text
//! if data == "\x1b[O" { self.selection.clear(); return true; }
//! ```
//!
//! so the highlight the user was about to copy from was thrown away, and
//! `drag_edge` / `scrollbar_drag` went with it silently.
//!
//! It was invisible in practice for a second reason found while writing these
//! rows: the host only routed input starting with `ESC [ <` to the renderer, so
//! the arm was **unreachable** — and LCA never emitted `?1004h`, so a terminal
//! never sent the report either. A guarded `clear()` on its own would therefore
//! have been dead code that every unit row still passed, which is the
//! #377/#379 failure mode the brief calls out. Both halves are pinned here: the
//! guard, and the wiring that makes the guard reachable.
//!
//! ## The predicate
//!
//! `Selection::press_active` — "is a press still unreleased" — not "is there a
//! selection". A finished selection is exactly what the user is pasting from; an
//! interrupted one has no coherent highlight to keep. It is set by `start()` and
//! cleared by `end()` (the mouse release), so it is genuinely both-valued
//! without focus loss involved.
//!
//! Split out of `alt_screen.rs` for the workspace's 1,200-line ceiling (gate 11).

use super::*;
use crate::engine::terminal::FakeTerminal;

/// A renderer with `rows` painted and no gesture in flight.
fn renderer(rows: &[&str]) -> AltScreenRenderer {
    let mut r = AltScreenRenderer::new();
    r.previous = rows.iter().map(|s| s.to_string()).collect();
    r.width = 40;
    r.height = rows.len() as u16;
    r
}

/// Drag from `(col, row)` to `(col2, row2)` and release — a completed gesture.
fn drag_and_release(r: &mut AltScreenRenderer, from: (u16, u16), to: (u16, u16)) {
    r.handle_mouse(SgrMouse {
        bits: 0,
        x: from.0,
        y: from.1,
        press: true,
    });
    r.handle_mouse(SgrMouse {
        bits: 32, // motion
        x: to.0,
        y: to.1,
        press: true,
    });
    r.handle_mouse(SgrMouse {
        bits: 0,
        x: to.0,
        y: to.1,
        press: false,
    });
}

// Verifies: gh #374 gap 2 - a **finished** selection survives `FOCUS_OUT` and
// still yields the same copied text. This is the bug: the release happened, so
// the user is about to paste.
#[test]
fn a_finished_selection_survives_focus_loss_and_still_copies() {
    let mut r = renderer(&["hello world"]);
    drag_and_release(&mut r, (1, 1), (6, 1));
    let before = r.selected_text();
    assert_eq!(before, "hello", "the drag selected the word stem");

    assert!(r.handle_input(FOCUS_OUT), "the report is consumed");
    assert!(
        r.selection.is_active(),
        "a finished selection is still there - it is what the user is pasting"
    );
    assert_eq!(
        r.selected_text(),
        before,
        "and it still yields exactly the same bytes"
    );
}

// Verifies: gh #374 gap 2 (the mirror row) - a press with no release is an
// **interrupted** gesture, so `FOCUS_OUT` cancels it. Both directions matter: a
// fix that keeps every selection would keep a half-dragged one too.
#[test]
fn an_interrupted_drag_is_cancelled_by_focus_loss() {
    let mut r = renderer(&["hello world"]);
    // Press and move, but never release.
    r.handle_mouse(SgrMouse {
        bits: 0,
        x: 1,
        y: 1,
        press: true,
    });
    r.handle_mouse(SgrMouse {
        bits: 32,
        x: 6,
        y: 1,
        press: true,
    });
    assert!(r.selection.is_active(), "mid-drag there is a highlight");
    assert!(r.selection.press_active, "and the press is unreleased");

    assert!(r.handle_input(FOCUS_OUT));
    assert!(
        !r.selection.is_active(),
        "an interrupted drag keeps nothing: {r:?}",
        r = r.selected_text()
    );
    assert!(!r.selection.press_active, "and the press is over");
}

// Verifies: gh #374 gap 2 - each non-selection gesture state is aborted
// individually, because one can be forgotten. Leaving any armed trades one stale
// state for another.
#[test]
fn every_other_gesture_state_still_aborts_on_focus_loss() {
    // drag_edge: the auto-scroll tick must not keep extending a selection nobody
    // is dragging.
    let mut r = renderer(&["a", "b", "c", "d"]);
    r.handle_mouse(SgrMouse {
        bits: 0,
        x: 1,
        y: 1,
        press: true,
    });
    r.handle_mouse(SgrMouse {
        bits: 32,
        x: 1,
        y: 4, // the last row: arms the bottom edge
        press: true,
    });
    assert!(r.tick_auto_scroll(), "the edge is armed mid-drag");
    r.handle_input(FOCUS_OUT);
    assert!(
        !r.tick_auto_scroll(),
        "focus loss disarms the edge auto-scroll"
    );

    // scrollbar_drag: it would swallow the next real motion and release.
    let mut r = renderer(&["a"]);
    r.begin_scrollbar_drag();
    assert!(r.scrollbar_dragging());
    r.handle_input(FOCUS_OUT);
    assert!(
        !r.scrollbar_dragging(),
        "focus loss ends a scrollbar drag - a later motion is not the drag's"
    );

    // scrollbar_hover: it is a pointer position, and the pointer is gone.
    let mut r = renderer(&["a"]);
    r.set_scrollbar_hover(true);
    r.handle_input(FOCUS_OUT);
    assert!(
        !r.scrollbar_hovered(),
        "the pointer is not over this window"
    );

    // press_cell: a release after focus returns must not look like a click on a
    // cell from before the focus change.
    let mut r = renderer(&["a link \x1b]8;;https://example.test\x07here\x1b]8;;\x07"]);
    r.handle_mouse(SgrMouse {
        bits: 0,
        x: 3,
        y: 1,
        press: true,
    });
    assert!(r.press_cell.is_some(), "a press records its cell");
    r.handle_input(FOCUS_OUT);
    assert!(r.press_cell.is_none(), "focus loss forgets the press cell");

    // the click run: the next single click must not continue a word/line grab.
    let mut r = renderer(&["a"]);
    r.set_click_run(2);
    r.handle_input(FOCUS_OUT);
    assert_eq!(
        r.click_run(),
        0,
        "the multi-click run does not survive: a later click is a fresh one"
    );
}

// Verifies: gh #374 gap 2 - `FOCUS_IN` resumes nothing. On return the selection
// is either still there (finished) or absent (interrupted), and no gesture
// state comes back.
#[test]
fn focus_in_resurrects_nothing() {
    // A finished selection stays exactly as it was.
    let mut r = renderer(&["hello world"]);
    drag_and_release(&mut r, (1, 1), (6, 1));
    let before = r.selected_text();
    assert!(r.handle_input(FOCUS_IN), "the report is consumed");
    assert_eq!(r.selected_text(), before, "nothing changed on the way back");
    assert!(!r.selection.press_active, "and no press came back");

    // An interrupted one does not come back either.
    let mut r = renderer(&["hello world"]);
    r.handle_mouse(SgrMouse {
        bits: 0,
        x: 1,
        y: 1,
        press: true,
    });
    r.handle_input(FOCUS_OUT);
    r.handle_input(FOCUS_IN);
    assert!(
        !r.selection.is_active(),
        "a cancelled drag is not resurrected by returning"
    );
    assert!(!r.selection.press_active);
}

// Verifies: gh #374 gap 2 - a press after focus returns starts a **fresh**
// selection rather than extending the old one. This is what makes the surviving
// highlight usable: you can see it, copy it, and then select again.
#[test]
fn a_press_after_focus_returns_starts_a_fresh_selection() {
    let mut r = renderer(&["alpha beta", "gamma delta"]);
    drag_and_release(&mut r, (1, 1), (6, 1));
    assert_eq!(r.selected_text(), "alpha");
    r.handle_input(FOCUS_OUT);
    r.handle_input(FOCUS_IN);
    assert_eq!(r.selected_text(), "alpha", "the old one survived");

    // Click elsewhere: the anchor moves, so the new drag does not span from the
    // old one's start.
    drag_and_release(&mut r, (7, 2), (12, 2));
    assert_eq!(
        r.selected_text(),
        "delta",
        "the new gesture is anchored where it started, not extended from before"
    );
}

// Verifies: gh #374 gap 2 (the brief's point 5) - **the predicate can be both
// ways** without focus loss involved. If focus loss were the only thing that
// cleared it, the guard would never be false and the fix would be inert - the
// #377/#379 failure mode. This row exists so the next reader can see the proof.
#[test]
fn the_predicate_is_both_valued_without_focus_loss() {
    let mut r = renderer(&["hello world"]);
    r.handle_mouse(SgrMouse {
        bits: 0,
        x: 1,
        y: 1,
        press: true,
    });
    assert!(r.selection.press_active, "a press sets it");
    r.handle_mouse(SgrMouse {
        bits: 0,
        x: 4,
        y: 1,
        press: false,
    });
    assert!(
        !r.selection.press_active,
        "the mouse release clears it, with no focus event in sight"
    );
    assert!(
        r.selection.is_active(),
        "and the selection it left behind is still a selection"
    );
}

// Verifies: gh #374 gap 2 (the wiring half) - the alt screen asks the terminal
// for focus reports and turns them off again. Without `?1004h` the terminal
// never sends `ESC [ O`, so the guard above could never run: this is the row
// that stops the fix from being dead code.
#[test]
fn the_alt_screen_asks_for_focus_reports_and_stops_them_on_exit() {
    let mut term = FakeTerminal::new(80, 24);
    let mut r = AltScreenRenderer::new();
    r.enter(&mut term);
    let entered = term.take_output();
    assert!(
        entered.contains("\x1b[?1004h"),
        "the terminal is asked to report focus changes: {entered:?}"
    );

    let mut term = FakeTerminal::new(80, 24);
    r.leave(&mut term, true);
    let left = term.take_output();
    assert!(
        left.contains("\x1b[?1004l"),
        "and focus reporting stops with the interface, or the shell prompt \
         reads the reports as typing: {left:?}"
    );
    assert!(
        !left.contains("\x1b[?1004h"),
        "the exit never re-enables it: {left:?}"
    );
}
