//! `Chat`'s overlay visibility predicates (gh #373 gap 6): the side panel's
//! responsive breakpoint, and the focus trap a hidden overlay must not spring.
//!
//! Split from `chat_overlay_tests.rs` under the workspace's 1,200-line file
//! ceiling. The fixtures (`options`, `chat`, `strip`) stay in `chat_tests.rs`
//! and are imported from it.
//!
//! The predicate rows live together because they are **one contract**: the same
//! answer decides the paint and the keyboard. Splitting the paint row from the
//! focus row is how the two halves drift, which is the failure mode this cycle
//! exists to close.

use super::*;
use super::{chat, strip};

// ===========================================================================
// gh #373 gap 6: the side panel's visibility predicate, and the focus trap.
//
// The option exists so a floating HUD can hide itself below a width breakpoint
// **without** its owner rebuilding its options on every resize. The safe half is
// not the paint: a panel hidden in the painter but still owning the keyboard makes
// the app feel possessed. These rows are that half.
// ===========================================================================

/// The width gate the panel uses in these rows.
fn panel_wide_enough(min_cols: u16) -> Box<dyn Fn(u16, u16) -> bool> {
    Box::new(move |cols, _rows| cols >= min_cols)
}

// Verifies: gh #373 gap 6 - **a panel hidden by its own breakpoint does not eat the
// keystroke.**
//
// The predicate is set to "at least 90 columns" and the frame is painted at 60. The
// key then has to reach the editor. Before the focus half existed, `handle_panel`
// answered first and the key vanished into a surface that was not on screen - which
// is precisely the bug the option would introduce if it were implemented in the
// painter alone.
#[test]
fn a_panel_hidden_by_its_breakpoint_does_not_own_the_keyboard() {
    let mut chat = chat();
    // An interactor that swallows everything: without the gate this is what eats the
    // keystroke, and with it the editor gets the key. A panel with no interactor
    // registered answers `None` either way, which is why the first version of this row
    // passed against the defect - the harness, not the fix, was doing the work.
    chat.world.options.ui_events =
        Some(Arc::new(|region: &str, _input: &lca_protocol::UiInput| {
            (region == "panel").then_some((String::new(), lca_protocol::UiEffect::None))
        }));
    chat.panel_visible = Some(panel_wide_enough(90));
    chat.world.panel_open = true;
    // Paint at 60 columns: below the breakpoint, so the panel is not on screen.
    let _ = chat.viewport(60, 24, 0);
    assert!(
        !chat.panel_shown(60, 24),
        "the frame is below the panel's breakpoint"
    );

    chat.handle_key("x");
    assert_eq!(
        chat.editor.text(),
        "x",
        "the key reached the editor instead of a hidden panel"
    );
    assert!(
        chat.world.panel_open,
        "and the panel is still open, merely hidden"
    );
}

// Verifies: gh #373 gap 6 - the same options value shows the panel **above** the
// breakpoint, with no re-construction: two frames, two answers.
//
// This is the property a static `bool` cannot have, and it is why the option is a
// predicate. It also proves the answer follows the *frame's* size rather than a
// stale one.
#[test]
fn the_panel_shows_again_above_its_breakpoint() {
    let mut chat = chat();
    chat.panel_visible = Some(panel_wide_enough(90));
    chat.world.panel_open = true;
    assert!(!chat.panel_shown(60, 24), "hidden when narrow");
    assert!(chat.panel_shown(120, 24), "shown when wide");
    assert!(chat.panel_shown(90, 24), "and the breakpoint is inclusive");
}

// Verifies: gh #373 gap 6 - **the key gate reads the size the last frame painted at,
// not the terminal's current size.**
//
// Input dispatch has no size argument, so a second guess here would be a second
// answer. The frame at 60 columns is what the reader is looking at, and it says the
// panel is hidden; a later resize that has not been painted yet must not flip the
// gate underneath the reader.
#[test]
fn the_key_gate_reads_the_size_of_the_frame_on_screen() {
    let mut chat = chat();
    chat.panel_visible = Some(panel_wide_enough(90));
    chat.world.panel_open = true;
    let _ = chat.viewport(60, 24, 0);
    assert!(!chat.panel_shown_now(), "hidden, per the painted frame");

    // The terminal grew, but nothing has painted at the new size yet. `world.resize` is
    // what the host calls on SIGWINCH, and it is deliberately *not* trusted here: the
    // reader is still looking at the 60-column frame, so a key must still be judged
    // against it. This is the half that distinguishes "the painted frame" from "the
    // terminal's current size" - without it the row cannot tell the two apart, which is
    // how the first version of this row passed against the wrong implementation.
    chat.world.resize(120, 24);
    assert_eq!(chat.world.size, (120, 24), "the terminal did grow");
    assert!(
        !chat.panel_shown_now(),
        "the gate still answers for the frame on screen"
    );

    // One frame at the new size, and the gate follows.
    let _ = chat.viewport(120, 24, 0);
    assert!(chat.panel_shown_now(), "now the frame agrees it is visible");
}

// Verifies: gh #373 gap 6 (control) - **no predicate means always shown**, so every
// existing host keeps exactly the behaviour it had.
#[test]
fn a_chat_with_no_panel_predicate_shows_the_panel_while_open() {
    let mut chat = chat();
    chat.world.panel_open = true;
    assert!(chat.panel_shown(40, 24), "no predicate: shown at any size");
    assert!(chat.panel_shown_now(), "and the key gate agrees");
    chat.world.panel_open = false;
    assert!(!chat.panel_shown(200, 60), "and closed is closed");
}

// Verifies: gh #373 gap 6 - a hidden panel is not painted either.
//
// The painter half. A panel below its breakpoint must contribute no rows, or the
// predicate would only move the defect: the box would still take the right-hand
// columns while its keys went to the editor.
#[test]
fn a_panel_below_its_breakpoint_paints_nothing() {
    let mut chat = chat();
    chat.world.options.render_regions = Some(Arc::new(|region: &str| {
        if region == "panel" {
            vec![(
                "panel".to_string(),
                lca_protocol::WidgetTree {
                    nodes: vec![lca_protocol::Widget::Text {
                        content: "PANEL-MARKER".into(),
                        role: "default".into(),
                    }],
                },
            )]
        } else {
            Vec::new()
        }
    }));
    chat.panel_visible = Some(panel_wide_enough(90));
    chat.world.panel_open = true;
    // The panel is composited over the viewport, so the fullscreen path is the one
    // that paints it - the same path `compose_overlays` serves.
    chat.screen_mode = true;

    let narrow = strip(&chat.viewport(60, 24, 0)).join("\n");
    assert!(
        !narrow.contains("PANEL-MARKER"),
        "no panel rows below the breakpoint:\n{narrow}"
    );
    let wide = strip(&chat.viewport(120, 24, 0)).join("\n");
    assert!(
        wide.contains("PANEL-MARKER"),
        "and the panel paints above it:\n{wide}"
    );
}
