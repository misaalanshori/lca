//! Gh #374 gap 1: the document the fullscreen exit hands back.
//!
//! The renderer rows (in `lca-tui`) prove that *given* a document it dumps it
//! whole and styled. These rows prove the other half: that `lca-ui` produces the
//! whole one. Without them the fix would be a renderer that faithfully dumps
//! whatever it was handed, while the host kept handing it a window.
//!
//! Split out of `run_tests.rs` for the workspace's 1,200-line ceiling (gate 11).

use super::*;

// ============================================================================
// gh #374 gap 1 - the document the fullscreen exit hands back.
//
// The renderer rows (in `lca-tui`) prove that *given* a document it dumps it
// whole and styled. These rows prove the other half: that `lca-ui` produces the
// whole one. Without them the fix would be a renderer that faithfully dumps
// whatever it was handed, while the host kept handing it a window.
// ============================================================================

// Verifies: gh #374 gap 1 (loss (a)) - the document is the transcript's *every*
// row, not the frame's. The frame is 24 rows; the document must be taller, and
// must contain the first thing the user said, which scrolled away long ago.
#[test]
fn the_document_is_the_whole_transcript_not_the_frame() {
    let mut chat = chat();
    // The document is fullscreen's: the scrollback renderer appends as it goes
    // and has no exit dump to feed.
    chat.screen_mode = true;
    // Enough entries that the transcript cannot fit in any plausible frame.
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
    }
    let frame = chat.viewport(80, 24, 0);
    let document = chat.take_document();
    assert!(
        document.len() > frame.len(),
        "the document outgrows the frame, or nothing was cut: frame {} vs document {}",
        frame.len(),
        document.len()
    );
    assert!(
        document
            .iter()
            .any(|row| strip_row(row).contains("question 0")),
        "the first thing the user said is in the document"
    );
    assert!(
        !frame
            .iter()
            .any(|row| strip_row(row).contains("question 0")),
        "and is *not* in the frame - which is the bug: the frame alone lost it"
    );
    assert!(
        document
            .iter()
            .any(|row| strip_row(row).contains("question 39")),
        "the newest row is in the document too"
    );
}

// Verifies: gh #374 gap 1 - the document carries the dock (separator, editor,
// footer) exactly once and in transcript-then-dock order, and carries **no**
// screen-only chrome: the tab bar names a screen the scrollback does not have,
// and an app-painted scrollbar would make a column of the preserved transcript
// unselectable.
#[test]
fn the_document_is_transcript_then_dock_without_screen_chrome() {
    let mut chat = chat();
    // The document is fullscreen's: the scrollback renderer appends as it goes
    // and has no exit dump to feed.
    chat.screen_mode = true;
    for i in 0..6 {
        chat.transcript.push_user(format!("row {i}"));
    }
    let _ = chat.viewport(80, 24, 0);
    let document = chat.take_document();
    let plain: Vec<String> = document.iter().map(|row| strip_row(row)).collect();
    let last_row = plain
        .iter()
        .rposition(|row| row.contains("row 5"))
        .expect("the newest transcript row is there");
    // The footer is dock: it must come after the last transcript row.
    let footer = plain
        .iter()
        .rposition(|row| !row.trim().is_empty())
        .expect("the document has content");
    assert!(
        footer > last_row,
        "the dock follows the transcript, not the other way round: {plain:#?}"
    );
    assert!(
        !plain
            .iter()
            .any(|row| row.contains('┃') || row.contains('▲')),
        "no app-painted scrollbar in the document: {plain:#?}"
    );
}

// Verifies: gh #374 gap 1 - the frame and the document come from **one**
// composition, so a scrolled frame cannot show the user rows the exit then
// leaves out. The last row of the document is the same string the frame ends
// with when scrolled to the live bottom - one `sections` call, one transcript.
#[test]
fn the_frame_is_a_window_of_the_document_it_ships_with() {
    let mut chat = chat();
    // The document is fullscreen's: the scrollback renderer appends as it goes
    // and has no exit dump to feed.
    chat.screen_mode = true;
    for i in 0..12 {
        chat.transcript.push_user(format!("line {i}"));
    }
    let frame = chat.viewport(80, 24, 0);
    let document = chat.take_document();
    let frame_last = frame
        .iter()
        .rev()
        .find(|row| !strip_row(row).trim().is_empty())
        .map(|row| strip_row(row))
        .unwrap_or_default();
    let document_last = document
        .iter()
        .rev()
        .find(|row| !strip_row(row).trim().is_empty())
        .map(|row| strip_row(row))
        .unwrap_or_default();
    assert_eq!(
        frame_last, document_last,
        "the frame's live bottom and the document's last row agree"
    );
}

/// One row's text, for assertions that are about content rather than colour.
fn strip_row(row: &str) -> String {
    lca_tui::engine::text::strip_terminal_sequences(row)
}
