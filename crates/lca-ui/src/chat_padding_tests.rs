//! gh #372 bug 2: the viewport padding, and the overlay predicate behind its fix.
//!
//! **Measured reach.** The padding was unconditional at `viewport()`, so a frame shorter
//! than the terminal was resized up to `height`. This row proves it at that seam (45 rows
//! for a 45-row viewport before the fix, 21 after). It does **not** reproduce in a live
//! pane: measured through the real loop, a fresh session paints 19 rows in a 60-row
//! terminal both pre- and post-fix, because the live dock (tabs, separator, editor
//! frame, footer) makes the frame taller than the padding target in the ordinary case.
//! So the fix closes the seam rather than a symptom users were hitting on every start;
//! it bites when the frame *is* shorter than the pane, which these rows construct
//! directly.
//!
//! Split from `chat_overlay_tests.rs` when that file crossed gate 11's 1,200-line
//! ceiling with these rows in it — the same reason `chat_bug_tests.rs`,
//! `chat_drawer_tests.rs` and `chat_picker_scroll_tests.rs` were split before it.

use super::chat;

// ============================================================================
// gh #372 bug 2 - no overlay open means no padding to viewport height
// ============================================================================

// Verifies: gh #372 bug 2 - a **short session with no overlay is not padded** to the
// viewport height. `compose_overlays_bottom_anchored` padded every frame
// unconditionally, so a fresh 8-line session in a 45-row terminal appended 37 blank
// rows and shoved the prompt and header to the top of the scrollback. Padding exists
// so overlays land at the visible bottom, so it applies when an overlay is open.
#[test]
fn a_short_session_with_no_overlay_is_not_padded() {
    let mut chat = chat();
    // Scrollback mode: this is where a padded frame appends blank rows to the real
    // scrollback. Fullscreen pads its own window by design (the dock pins to the
    // bottom), so the bug is specific to the other renderer.
    chat.screen_mode = false;
    for i in 0..3 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (width, height) = (80u16, 45u16);
    let frame = chat.viewport(width, height, 0);
    assert!(
        frame.len() < height as usize,
        "no overlay is open, so nothing needs the padding: {} rows for a {height}-row \
         viewport means {} blank rows were appended",
        frame.len(),
        height as usize - frame.len()
    );
    // ...and the frame ends where the session does, not at a run of padding. The
    // transcript's own trailing blank row is its content; a *padded* frame would
    // end with far more.
    let strip = lca_tui::engine::text::strip_terminal_sequences;
    let trailing_blanks = frame
        .iter()
        .rev()
        .take_while(|row| strip(row).trim().is_empty())
        .count();
    assert!(
        trailing_blanks <= 2,
        "the frame must not end in a run of padding: {trailing_blanks} blank rows"
    );
}

// Verifies: gh #372 bug 2 (the **no-regression** row) - an **open** overlay still pads
// and composites. Padding is not wrong; applying it when nothing needs it is.
#[test]
fn an_open_overlay_still_composites_over_the_padded_viewport() {
    let mut chat = chat();
    chat.screen_mode = false;
    chat.transcript.push_user("hello".to_string());
    chat.transcript.append_text("hi");
    chat.transcript.finish_assistant();
    chat.world.show_permission("rm -rf /tmp/x".into());
    let (width, height) = (80u16, 45u16);
    let frame = chat.viewport(width, height, 0);
    let strip = lca_tui::engine::text::strip_terminal_sequences;
    let text = frame
        .iter()
        .map(|r| strip(r))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("rm -rf /tmp/x"),
        "the modal is composited when it is open"
    );
    assert_eq!(
        frame.len(),
        height as usize,
        "an open overlay anchors to the visible bottom, so the frame fills the viewport"
    );
}

// Verifies: gh #372 bug 2 - a **panel** counts as an overlay. `panel_open` is a
// separate flag from the modal/picker slots, so a fix keyed only on
// `modal_active()` would silently stop padding the panel's drawer tab into place.
#[test]
fn an_open_panel_counts_as_an_overlay_for_padding() {
    let mut chat = chat();
    chat.screen_mode = false;
    for i in 0..3 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    chat.world.panel_open = true;
    let (width, height) = (80u16, 45u16);
    let frame = chat.viewport(width, height, 0);
    assert_eq!(
        frame.len(),
        height as usize,
        "the panel needs the padded bottom, so the frame fills the viewport"
    );
}
