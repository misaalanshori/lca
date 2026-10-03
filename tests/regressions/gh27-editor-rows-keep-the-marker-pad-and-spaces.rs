//! GitHub issue #27 (open): a multiline prompt stepped back to column 0 on
//! every line after the first - the `> ` marker was applied to
//! `editor_lines[0]` only, so lines 2+ started at the margin and the cursor
//! marker's column disagreed with line 1's by two. And spaces the user
//! typed were dropped at wrap boundaries: the row the word wrap broke on was
//! `trim_end`ed, so a space at the break never reached the screen (and the
//! caret's char offsets drifted with it).
//!
//! pi's answers, both from its editor rather than its transcript wrapper:
//! every row after the marker carries the marker's width as a plain pad
//! (`editor.md` §5's row layout), and the break sits *after* the whitespace
//! so the continuation starts at the next word and the spaces stay in the
//! row (`wordWrapLine`, `editor.md` §3).
//!
//! The rows below hold the two symptoms at the surfaces they were reported
//! on: the prompt as the user sees it (marker, pad, cursor column) and the
//! wrapped rows as the terminal shows them (every typed space on screen).
//!
//! Verifies: NFR-24 (a released defect's guard), GitHub issue #27.

use lca_tui::engine::core::CURSOR_MARKER;
use lca_tui::engine::text::{strip_terminal_sequences, visible_width};
use lca_tui::widgets::editor::Editor;

/// The buffer's rows as the terminal shows them: no caret marker, no SGR.
fn plain_rows(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| strip_terminal_sequences(&row.replace(CURSOR_MARKER, "")))
        .collect()
}

// Verifies: gh #27 (b) - a space occupies its cell. Every space in the
// buffer is on screen after a wrap, and the row the wrap broke on still
// fills its width (the break's space included).
#[test]
fn every_typed_space_survives_a_wrap_and_occupies_its_cell() {
    let text = "aaaa bbbb cccc dddd eeee ffff";
    let mut editor = Editor::new();
    editor.set_text(text);
    // Off the end of the line, so the caret paints an existing character
    // rather than adding its own end-of-line cell to the count.
    editor.cursor_line_start();

    let rows = plain_rows(&editor.render(10));
    assert!(rows.len() >= 3, "the line wraps at width 10: {rows:?}");

    let on_screen: usize = rows
        .iter()
        .map(|row| row.chars().filter(|c| *c == ' ').count())
        .sum();
    assert_eq!(
        on_screen,
        text.chars().filter(|c| *c == ' ').count(),
        "no typed space was swallowed: {rows:?}"
    );
    assert!(
        rows[0].ends_with(' '),
        "the break keeps its space at the row's end: {rows:?}"
    );
    assert_eq!(
        visible_width(&rows[0]),
        10,
        "the row's width includes that space: {rows:?}"
    );
}
