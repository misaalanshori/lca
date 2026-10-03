//! GitHub issue #28 (open): Arrow Up/Down walked `Editor.lines` - the
//! logical lines - so on a prompt that *wrapped* without a hard newline the
//! buffer was one line and Up behaved like Home while Down behaved like End.
//!
//! pi's model (`editor.md` §3/§4): the buffer has a visual-row map at the
//! current width, Up/Down move between visual rows keeping the visual column
//! (the sticky column), a move past a logical line's first/last row enters
//! the neighbouring logical line's last/first row, and the buffer edges keep
//! pi's rules (Up past the top: start of the line, no history jump; Down
//! past the bottom: history while browsing, else end of line).
//!
//! The load-bearing part is the cycle's interaction constraint: the rows
//! navigation walks are the rows `Editor::render` draws at that width. This
//! guard asserts exactly that: the caret's rendered row after every arrow.
//! A geometry that drifts from the renderer fails here even when each half
//! looks right on its own.
//!
//! Verifies: NFR-24 (a released defect's guard), GitHub issue #28.

use lca_tui::widgets::editor::Editor;

/// The caret's row in `render(width)` - the row the renderer drew, which is
/// the row navigation must have moved it to.
fn caret_row(editor: &Editor, width: u16) -> usize {
    let (_, pos) = lca_tui::engine::core::extract_cursor_position(&editor.render(width));
    let (row, _, _) = pos.expect("every frame carries the caret marker");
    row as usize
}

// Verifies: gh #28 (the primary row) - on one wrapped logical line, Up and
// Down walk the rendered rows one at a time, in both directions, and the
// caret is drawn where the move put it.
#[test]
fn the_arrows_walk_the_rows_the_renderer_drew() {
    let mut editor = Editor::new();
    editor.set_text("aaaa bbbb cccc dddd eeee ffff gggg");
    let total = editor.render(10).len();
    assert!(total >= 3, "the line wraps into several rows: {total}");

    for want in (0..total - 1).rev() {
        editor.cursor_up();
        assert_eq!(
            caret_row(&editor, 10),
            want,
            "Up lands on the previous rendered row"
        );
    }
    for want in 1..total {
        editor.cursor_down();
        assert_eq!(
            caret_row(&editor, 10),
            want,
            "Down lands on the next rendered row"
        );
    }
}

// Verifies: gh #28 - the buffer's edges keep pi's behaviour once movement is
// visual: Up past the first rendered row starts the line (column 0) without
// jumping into history, and Down past the last row ends the line.
#[test]
fn the_buffer_edges_keep_their_behaviour_on_a_wrapped_line() {
    let mut editor = Editor::new();
    editor.set_text("aaaa bbbb cccc dddd eeee ffff gggg");
    assert_eq!(caret_row(&editor, 10), editor.render(10).len() - 1);

    editor.cursor_up();
    editor.cursor_up();
    editor.cursor_up();
    editor.cursor_up();
    assert_eq!(caret_row(&editor, 10), 0, "up the rows to the first one");
    assert_eq!(editor.lines().len(), 1, "still one logical line");

    editor.cursor_up();
    assert_eq!(
        caret_row(&editor, 10),
        0,
        "Up past the top row: start of the line, not history"
    );
    assert_eq!(
        editor.text(),
        "aaaa bbbb cccc dddd eeee ffff gggg",
        "and the buffer is untouched"
    );

    editor.cursor_line_end();
    editor.cursor_down();
    assert_eq!(
        caret_row(&editor, 10),
        editor.render(10).len() - 1,
        "Down past the last row: end of the line"
    );
}
