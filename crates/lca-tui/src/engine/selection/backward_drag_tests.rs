//! Gh #374 gap 3: dragging **backwards** keeps the word you double-clicked.
//!
//! ## The defect, measured
//!
//! `Selection::update` adjusted the focus column against the *current* line's
//! copy of the initial range, and never flipped `anchor`. Double-clicking
//! `interactive` (columns `6..17`) and dragging left to column 1 therefore set
//! `focus.col = range.0`, leaving `anchor` and `focus` on the same cell - a
//! zero-width range. The word the user grabbed did not merely shrink, it
//! vanished, and the copy was the empty string:
//!
//! ```text
//! double-click      -> "interactive"
//! drag left to col1 -> ""          <-- the bug
//! ```
//!
//! ## The fix, and why the frame has to be stored
//!
//! The anchor word is the **reference frame**: both a forward and a backward
//! drag have to keep it inside the normalized range. Re-running word detection
//! on the drag's line cannot reproduce it - that is what makes the direction
//! matter - so the initial range (row *and* columns, pi's
//! `selectionInitialRange`) is remembered at the press and the endpoints are
//! derived from it:
//!
//! - target before the initial range: `anchor = initial.end`,
//!   `focus = target.start`;
//! - otherwise: `anchor = initial.start`, `focus = target.end`.
//!
//! The comparison is `(row, col)` **lexicographic**, so a drag up counts as
//! "before" whatever its column and a drag down counts as "after" - which is
//! why the diagonals matter: an axis-only comparison passes them by accident.
//!
//! Split out of `selection.rs` for the workspace's 1,200-line ceiling (gate 11)
//! and to keep the direction matrix in one readable place.

use super::*;

/// The rows the direction matrix runs on. Row 1 is the one the issue describes:
/// `interactive` occupies columns `6..17`.
///
/// ```text
/// col: 0123456789...
///      0  |aa|            cols 0..2
///         3 |bb|          cols 3..5
///            6|interactive|  cols 6..17
///                         18|cc|  cols 18..20
///                              21|dd|  cols 21..23
/// ```
fn rows() -> Vec<String> {
    vec![
        "second row here".to_string(),
        "aa bb interactive cc dd".to_string(),
        "third".to_string(),
    ]
}

/// A double-click on `interactive`.
fn anchor_word(sel: &mut Selection) {
    let r = rows();
    // Inside the word, not on its first cell: a click on a word's edge is the
    // case `line_word_range` treats specially, and the bug is not about edges.
    sel.start(
        SelectionPoint { row: 1, col: 8 },
        Granularity::Word,
        2,
        &r[1],
    );
}

/// A triple-click on row 1's line.
fn anchor_line(sel: &mut Selection) {
    let r = rows();
    sel.start(
        SelectionPoint { row: 1, col: 8 },
        Granularity::Line,
        3,
        &r[1],
    );
}

/// Drag to `point` on the row that point names, which is what the renderer does
/// (it hands `update` the pointer's own row).
fn drag_to(sel: &mut Selection, point: SelectionPoint) {
    let r = rows();
    let line = r.get(point.row as usize).cloned().unwrap_or_default();
    sel.update(point, &line);
}

/// The bytes a copy would put on the clipboard.
fn copied(sel: &Selection) -> String {
    sel.active_text(&rows())
}

// ===========================================================================
// Word granularity - the issue's own case.
// ===========================================================================

// Verifies: gh #374 gap 3 (the reported row) - double-click a word, drag
// **left**: the anchor word stays selected and the range grows from its far
// boundary. Before the fix the copy was empty, because anchor and focus
// collapsed onto the same cell.
#[test]
fn dragging_left_keeps_the_word_that_was_double_clicked() {
    let mut sel = Selection::new();
    anchor_word(&mut sel);
    assert_eq!(copied(&sel), "interactive", "the fixture grabbed the word");

    // Into `aa` (cols 0..2): the target word's start is column 0.
    drag_to(&mut sel, SelectionPoint { row: 1, col: 1 });
    sel.end();
    assert_eq!(
        copied(&sel),
        "aa bb interactive",
        "from the target word's start through the anchor word's end - the \
         anchor word is still in there"
    );
    // The specific failure the issue reports: the anchor word must be present.
    assert!(
        copied(&sel).contains("interactive"),
        "the double-clicked word never deselects itself: {:?}",
        copied(&sel)
    );
}

// Verifies: gh #374 gap 3 - drag **up** to a previous row. Same defect, other
// axis; a horizontal-only fix leaves this one.
#[test]
fn dragging_up_keeps_the_word_that_was_double_clicked() {
    let mut sel = Selection::new();
    anchor_word(&mut sel);

    // Into row 0's first word (`second`, cols 0..6).
    drag_to(&mut sel, SelectionPoint { row: 0, col: 2 });
    sel.end();
    assert_eq!(
        copied(&sel),
        "second row here\naa bb interactive",
        "the whole span from the target word up to the anchor word, which is on \
         the later row"
    );
    assert!(copied(&sel).contains("interactive"));
}

// Verifies: gh #374 gap 3 - **down-left**. The row dominates the comparison, so
// this is an "after" drag even though its column is smaller; the anchor stays
// where it started. An axis-only (column-only) comparison gets this wrong, which
// is why the diagonal is pinned.
#[test]
fn dragging_down_and_left_keeps_the_anchor_word() {
    let mut sel = Selection::new();
    anchor_word(&mut sel);

    // Row 2 is `third`, only five columns wide: the column is far left of the
    // anchor's, the row is after it.
    drag_to(&mut sel, SelectionPoint { row: 2, col: 1 });
    sel.end();
    let text = copied(&sel);
    assert_eq!(
        text, "interactive cc dd\nthird",
        "the anchor is the *start* of an after-drag, so the text begins with \
         the anchor word and runs forward to the target line - the row order \
         comes from the rows, not from the direction"
    );
}

// Verifies: gh #374 gap 3 - **up-right**. The mirror of the row above: a smaller
// row with a larger column is a "before" drag, so the anchor flips to its far
// boundary and the text *ends* with the anchor row.
#[test]
fn dragging_up_and_right_keeps_the_anchor_word() {
    let mut sel = Selection::new();
    anchor_word(&mut sel);

    // Row 0, far right of it - a column larger than the anchor's start.
    drag_to(&mut sel, SelectionPoint { row: 0, col: 12 });
    sel.end();
    let text = copied(&sel);
    assert!(
        text.ends_with("aa bb interactive"),
        "the anchor is the *end* of a before-drag, so the text ends with the \
         anchor row: {text:?}"
    );
    assert!(text.contains("interactive"), "and the anchor word survives");
}

// Verifies: gh #374 gap 3 (the frame-preserved row) - drag left, then drag back
// right past the anchor: the result is **byte-identical** to having dragged
// straight there. This is the row a re-derivation approach fails: if the
// endpoints came from the current line's word boundaries rather than from the
// remembered initial range, the round trip would not restore the forward path.
#[test]
fn reversing_the_drag_restores_exactly_what_the_forward_path_gives() {
    let forward = {
        let mut sel = Selection::new();
        anchor_word(&mut sel);
        drag_to(&mut sel, SelectionPoint { row: 1, col: 22 });
        sel.end();
        copied(&sel)
    };
    assert_eq!(forward, "interactive cc dd", "the forward path's answer");

    let mut sel = Selection::new();
    anchor_word(&mut sel);
    // Left first, then right past the anchor.
    drag_to(&mut sel, SelectionPoint { row: 1, col: 1 });
    drag_to(&mut sel, SelectionPoint { row: 1, col: 22 });
    sel.end();
    assert_eq!(
        copied(&sel),
        forward,
        "reversing restores the forward path exactly, so the frame was kept \
         rather than patched"
    );
}

// Verifies: gh #374 gap 3 - a drag that never crosses the anchor still grows the
// way it always did. The simple case is the control: the fix changes the
// backwards directions, not the forwards ones.
#[test]
fn a_forward_drag_is_unchanged() {
    let mut sel = Selection::new();
    anchor_word(&mut sel);
    drag_to(&mut sel, SelectionPoint { row: 1, col: 19 });
    sel.end();
    assert_eq!(
        copied(&sel),
        "interactive cc",
        "from the anchor word to the target word, as before"
    );
}

// ===========================================================================
// Line granularity - the same defect, the same treatment.
// ===========================================================================

// Verifies: gh #374 gap 3 - a triple-clicked line dragged **up** keeps that
// line. `update` compared columns on the current row instead of comparing the
// target line against the initial one, so the anchor line could fall out.
#[test]
fn a_line_dragged_up_keeps_the_anchor_line() {
    let mut sel = Selection::new();
    anchor_line(&mut sel);
    assert_eq!(
        copied(&sel),
        "aa bb interactive cc dd",
        "the fixture grabbed row 1"
    );

    drag_to(&mut sel, SelectionPoint { row: 0, col: 3 });
    sel.end();
    assert_eq!(
        copied(&sel),
        "second row here\naa bb interactive cc dd",
        "both whole lines, the anchor line last"
    );
}

// Verifies: gh #374 gap 3 - the line case **down**: the anchor line is the
// start, and the target line is included whole.
#[test]
fn a_line_dragged_down_keeps_the_anchor_line() {
    let mut sel = Selection::new();
    anchor_line(&mut sel);

    drag_to(&mut sel, SelectionPoint { row: 2, col: 1 });
    sel.end();
    assert_eq!(
        copied(&sel),
        "aa bb interactive cc dd\nthird",
        "anchor line first, then the target line whole"
    );
}

// Verifies: gh #374 gap 3 - and **left within the same line**, which is what the
// line case's column comparison got wrong. A line selection dragged left stays
// the whole line: there is nothing further left to grow into.
#[test]
fn a_line_dragged_left_within_itself_stays_the_whole_line() {
    let mut sel = Selection::new();
    anchor_line(&mut sel);
    drag_to(&mut sel, SelectionPoint { row: 1, col: 1 });
    sel.end();
    assert_eq!(
        copied(&sel),
        "aa bb interactive cc dd",
        "a line drag inside its own line is still that line"
    );
}

// ===========================================================================
// Character granularity - the control.
// ===========================================================================

// Verifies: gh #374 gap 3 (the control) - character granularity has no anchor
// range, so it must be untouched by this fix. A backward drag still grows from
// the press point to the pointer, which is the documented behaviour.
#[test]
fn a_character_drag_is_unchanged_in_both_directions() {
    let mut sel = Selection::new();
    let r = rows();
    sel.start(
        SelectionPoint { row: 1, col: 8 },
        Granularity::Char,
        1,
        &r[1],
    );
    drag_to(&mut sel, SelectionPoint { row: 1, col: 3 });
    sel.end();
    // The half-open cells between the two columns - the behaviour character
    // granularity has always had, and the control for this fix. `bb in` is
    // columns 3..8, which excludes both endpoints' cells.
    assert_eq!(copied(&sel), "bb in", "backwards, cell by cell");

    let mut sel = Selection::new();
    sel.start(
        SelectionPoint { row: 1, col: 8 },
        Granularity::Char,
        1,
        &r[1],
    );
    drag_to(&mut sel, SelectionPoint { row: 1, col: 19 });
    sel.end();
    assert_eq!(copied(&sel), "teractive c", "forwards, likewise");
}
