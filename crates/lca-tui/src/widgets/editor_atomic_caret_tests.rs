//! gh #381 gap 4: **the caret on a paste chip is the whole badge**, not a column.
//!
//! Gaps 1-3 made a chip one unit for motion, deletion and restore; on screen it still
//! looked like any other column, so nothing told the user the thing under the caret was
//! atomic. pi merges a marker's graphemes into one atomic segment
//! (`segmentWithMarkers`, `editor.ts:44-60`) and its render paints the **first such
//! segment** reversed - so the badge is the caret.
//!
//! Split from `editor_atomic_tests.rs` at the 1,200-line ceiling.
//!
//! The design rules these rows pin, in the brief's order: one predicate (`chip_span`,
//! the one gaps 1-3 consult), close with `27m` never `0m` (#371 bug 2), `paint_caret`
//! gates *whether* rather than *what*, a split chip paints only its own row's cells,
//! and the boundary belongs to the chip.

use super::*;

/// A 12-line paste's chip, as the row draws it.
const CHIP: &str = "[paste #1 +12 lines]";

/// A chip for a 12-line paste, inserted at the cursor (`look: ` prefix).
fn with_chip(prefix: &str) -> Editor {
    let mut e = Editor::new();
    e.insert_str(prefix);
    let big = (0..12)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_paste(&big);
    e
}

// ---------------------------------------------------------------------------
// gap 4: the caret on a chip is the whole chip
// ---------------------------------------------------------------------------
//
// Gaps 1-3 made a chip one unit for motion, deletion and restore. On screen it
// still looked like any other column, so nothing told the user that the thing
// under the caret was atomic. pi merges a marker's graphemes into one atomic
// segment (`segmentWithMarkers`, editor.ts:44-60) and its render paints the
// **first such segment** reversed - so the badge, not one cell.

/// Every reverse-video run in a row, as `(opened_at, closed_at)` byte offsets.
///
/// A row can carry more than one (a themed row wraps its text); the rows here are
/// plain, so a second run is itself a finding.
fn reverse_runs(row: &str) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut open: Option<usize> = None;
    let mut at = 0usize;
    while let Some(found) = row[at..].find('\x1b') {
        let start = at + found;
        let rest = &row[start..];
        if rest.starts_with("\x1b[7m") {
            open = Some(start);
            at = start + 4;
        } else if rest.starts_with("\x1b[27m") {
            if let Some(opened) = open.take() {
                runs.push((opened, start));
            }
            at = start + 5;
        } else {
            at = start + 1;
        }
    }
    runs
}

/// The text a row paints between two byte offsets, escapes and markers removed.
///
/// `from` is where the row's `7m` sits, so the slice opens with it - strip handles
/// that. The marker is removed *before* stripping because `CURSOR_MARKER` is an APC
/// sequence, not an SGR one.
fn visible_between(row: &str, from: usize, to: usize) -> String {
    crate::engine::text::strip_terminal_sequences(&row[from..to].replace(CURSOR_MARKER, ""))
}

/// A chip whose caret sits at `col` (the row's own line, 0).
fn caret_at(col: usize) -> Editor {
    let mut e = with_chip("look: ");
    e.cursor_line = 0;
    e.cursor_col = col;
    e
}

// Verifies: gh #381 gap 4 - **the caret inside a chip paints the whole chip
// reversed**, and it is the chip, not a single cell.
//
// This is the row the gap exists for: the single-cell caret is what shipped, and a
// row asserting "some reverse video appears" passes against it. So the assertion is
// the *range* - exactly one run, from the chip's `[` to its `]`.
#[test]
fn the_caret_inside_a_chip_paints_the_whole_chip() {
    let e = caret_at(10); // inside `#1`
    let row = &e.render(60)[0];
    let runs = reverse_runs(row);
    assert_eq!(
        runs.len(),
        1,
        "the chip caret is one reversed run, not several: {row:?}"
    );
    let (open, close) = runs[0];
    assert_eq!(
        visible_between(row, open, close),
        CHIP,
        "the whole chip is reversed, not one column of it - this is the gap's \
         own defect: {row:?}"
    );
    // And the marker still lands at the caret, so the hardware cursor follows.
    let marker_at = row.find(CURSOR_MARKER).expect("the marker rides the row");
    assert!(
        row[..marker_at].replace(CURSOR_MARKER, "").len() <= open,
        "the marker sits at the chip's start, not after the badge: {row:?}"
    );
}

// Verifies: gh #381 gap 4 - **a caret before the chip paints one cell**, and the
// chip is untouched.
//
// The boundary row: the caret on the last char before the chip (`col 5`, the space
// of `look: `) paints that space and nothing of the chip.
#[test]
fn the_caret_before_the_chip_paints_one_cell() {
    let e = caret_at(5);
    let row = &e.render(60)[0];
    let runs = reverse_runs(row);
    assert_eq!(runs.len(), 1, "one reversed run: {row:?}");
    let (open, close) = runs[0];
    assert_eq!(
        visible_between(row, open, close),
        " ",
        "the column before the chip is painted as itself, not as the chip: {row:?}"
    );
    assert!(
        !row.contains("\x1b[7m[") && !row.contains(&format!("\x1b[7m{CHIP}")),
        "the chip is not reversed when the caret is before it: {row:?}"
    );
}

// Verifies: gh #381 gap 4 - **the boundary is the chip's; `Left` from inside it
// lands before, and the two positions render differently.**
//
// Which side owns the boundary is the question the brief asks the report to state,
// so it is pinned from both directions rather than described: at `chip_start` the
// chip is painted (inside), at `chip_start - 1` it is not.
#[test]
fn the_chip_owns_its_opening_bracket() {
    let chip_start = 6; // `look: `.chars().count()

    let at_start = caret_at(chip_start).render(60).remove(0);
    let before = caret_at(chip_start - 1).render(60).remove(0);
    assert!(
        reverse_runs(&at_start)
            .iter()
            .any(|(o, c)| visible_between(&at_start, *o, *c) == CHIP),
        "at the chip's first char the chip is the caret's unit: {at_start:?}"
    );
    assert!(
        !reverse_runs(&before)
            .iter()
            .any(|(o, c)| visible_between(&before, *o, *c) == CHIP),
        "one column earlier it is not - the boundary belongs to the chip: {before:?}"
    );
}

// Verifies: gh #371 bug 2 (carried into gap 4) - **the chip caret closes with
// `27m`, never `0m`**, so text after the chip keeps its styling.
//
// A full reset here would strip the colours a caller applied around the row, which is
// the exact leak #371 bug 2 fixed one cycle earlier. The row asserts the *effect*
// (no `0m` on the row, and the closer is `27m`), because a row that only looked for
// `[27m` would pass against any string edit that never fixed the leak.
#[test]
fn the_chip_caret_closes_only_reverse_video() {
    let e = caret_at(10);
    let row = e.render(60).remove(0);
    assert!(
        !row.contains("\x1b[0m"),
        "the chip caret must not perform a full SGR reset: {row:?}"
    );
    // The closer immediately after the chip's last char is the reverse-video off.
    let chip_at = row.find(CHIP).expect("the chip is drawn");
    let closer = &row[chip_at + CHIP.len()..];
    assert!(
        closer.starts_with("\x1b[27m"),
        "the run closes with 27m, not a reset: {closer:?} in {row:?}"
    );
}

// Verifies: gh #381 gap 4 - **text after the chip keeps its own styling.**
//
// The `27m`-not-`0m` rule observed the way a reader sees it: a colour opened before
// the chip is still open after it, so the suffix is not repainted in the default
// colour.
#[test]
fn text_after_the_chip_keeps_its_own_styling() {
    let mut e = Editor::new();
    e.insert_str("\x1b[31m");
    let big = (0..12)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_paste(&big);
    e.insert_str("tail");
    e.cursor_line = 0;
    e.cursor_col = 10;
    let row = e.render(60).remove(0);
    let (_, close) = reverse_runs(&row)[0];
    let after = &row[close..];
    assert!(
        !after.contains("\x1b[0m"),
        "nothing after the chip's caret is fully reset - the `\\x1b[31m` opening \
         before it must survive: {row:?}"
    );
}

// Verifies: gh #381 gap 4 - **a chip split across rows paints only the cells on
// each row**, not a phantom range past the edge.
//
// Gap 1 established that a chip wider than the pane splits *visually* only
// (`editor.ts:174`). So the reverse video has to cover what is drawn on the row the
// caret is on, and nothing beyond it.
#[test]
fn a_split_chip_paints_only_the_cells_on_its_row() {
    let mut e = Editor::new();
    // A multi-line payload: that is what makes `insert_paste` register a chip
    // (`[paste #1 +12 lines]`) rather than inserting plain text.
    let big = (0..12)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_paste(&big);
    let narrow = 8;
    e.cursor_col = 8; // inside the marker, past its first visual row
    let rows = e.render(narrow);
    assert!(rows.len() > 1, "the chip really is split: {rows:?}");
    // The caret's own row is the one carrying the marker - at col 8 that is the
    // chip's *second* visual row, which is the point: the badge is split.
    let index = rows
        .iter()
        .position(|r| r.contains(CURSOR_MARKER))
        .expect("the caret has a row");
    assert!(index > 0, "the caret is on a continuation row: {rows:?}");
    let first = rows[index].replace(CURSOR_MARKER, "");
    let runs = reverse_runs(&first);
    assert_eq!(runs.len(), 1, "one run on the caret's row: {first:?}");
    let (open, close) = runs[0];
    let painted = visible_between(&first, open, close);
    assert!(
        !painted.is_empty()
            && painted.chars().all(|c| c == '['
                || c == ']'
                || c == '+'
                || c == '#'
                || c == ' '
                || c.is_ascii_alphanumeric()),
        "the painted run is drawn text on this row: {painted:?} in {first:?}"
    );
    // **Clipped to this row's cells.** Without the clip the span runs to the chip's
    // true end, so the painted run would carry the whole marker - characters this row
    // does not draw - and the row would be wider than the pane. Both halves are
    // asserted because that is the visible harm.
    let full = visible_between(&first, open, close);
    assert!(
        full.chars().count() < CHIP.chars().count(),
        "the painted run is this row's part of the chip, not the whole marker: \
         {full:?} in {first:?}"
    );
    assert!(
        crate::engine::text::visible_width(&first) <= narrow as usize + 1,
        "and the row still fits the pane (the painted caret may cost one extra cell): \
         {first:?}"
    );
    // No other row is reversed: the caret is on exactly one.
    for (at, row) in rows.iter().enumerate() {
        if at != index {
            assert!(
                reverse_runs(row).is_empty(),
                "row {at} carries no reverse video: {row:?}"
            );
        }
    }
}

// Verifies: gh #381 gap 4 - **with `paint_caret` off nothing is painted and the
// marker is unchanged.**
//
// `paint_caret` decides *whether*, not *what*: the hardware cursor sits at the marker
// as usual, so the chip caret degrades the same way rather than inventing a second
// visibility rule.
#[test]
fn the_chip_caret_degrades_with_paint_caret_off() {
    let plain = {
        let mut e = caret_at(10);
        e.set_paint_caret(false);
        e.render(60).remove(0)
    };
    assert!(
        reverse_runs(&plain).is_empty(),
        "nothing is reversed when the hardware cursor is the caret: {plain:?}"
    );
    assert!(
        plain.contains(CURSOR_MARKER),
        "the marker still rides the row so the hardware cursor can be placed: {plain:?}"
    );
    // **And it stays at the caret's own column.** This is the half the first version
    // of this row missed, and the gate is load-bearing because of it: the chip arm's
    // `before` starts at the chip, so without the `paint_caret` gate the marker would
    // jump to the chip's first cell and an IME candidate window would open in the
    // wrong place. The painted caret may collapse to the badge; the unpainted one
    // must not.
    let at = plain.find(CURSOR_MARKER).expect("marker");
    let column = crate::engine::text::visible_width(
        &plain[..at].replace("\x1b[7m", "").replace("\x1b[27m", ""),
    );
    assert_eq!(
        column, 10,
        "the unpainted marker sits on the caret's own column, not the chip's first \
         cell: {plain:?}"
    );
    let painted = caret_at(10).render(60).remove(0);
    assert_eq!(
        plain.replace(CURSOR_MARKER, ""),
        painted
            .replace("\x1b[7m", "")
            .replace("\x1b[27m", "")
            .replace(CURSOR_MARKER, ""),
        "the painted caret replaces text rather than adding any - the row's visible \
         text is identical either way"
    );
}

// Verifies: gh #381 gap 4 (control) - **a caret in ordinary text is unchanged.**
#[test]
fn a_caret_outside_any_chip_still_paints_one_cell() {
    let e = caret_at(2); // inside `look`
    let row = e.render(60).remove(0);
    let runs = reverse_runs(&row);
    assert_eq!(runs.len(), 1, "one reversed run: {row:?}");
    let (open, close) = runs[0];
    assert_eq!(
        visible_between(&row, open, close),
        "o",
        "the grapheme under the caret, as before: {row:?}"
    );
}

// Verifies: gh #381 gap 4 - **the end-of-line caret is unchanged** by the chip arm.
#[test]
fn the_end_of_line_caret_is_unaffected_by_the_chip_arm() {
    let mut e = Editor::new();
    e.insert_str("ab");
    let row = e.render(40).remove(0);
    assert!(
        row.contains("\x1b[7m \x1b[27m"),
        "the parked caret is still a reverse-video space: {row:?}"
    );
}

// Verifies: gh #381 gap 1 (a defect found by *live* verification, not by the rows) -
// **the caret can leave a chip in both directions.**
//
// The gap-1 rows tested entry, not exit: `Left` from after a chip and `Right` from
// before it. Sitting *on* a chip's boundary, both movements were no-ops -- `Left` at
// the start returned the start, `Right` at the end returned the end -- so a caret that
// entered a chip could only leave it by crossing the whole thing the other way. A tmux
// pane showed `Left` doing nothing, which is what sent me looking; no row covered it.
//
// These two are that missing half, one per direction.
#[test]
fn left_from_a_chips_start_leaves_it() {
    let mut e = with_chip("look: ");
    e.cursor_col = 6; // the chip's first char - the boundary it owns
    e.cursor_left();
    assert_eq!(
        e.cursor_col,
        5,
        "one Left from the chip's start steps before it: {:?}",
        e.text()
    );
    // And it does not stall there either.
    e.cursor_left();
    assert_eq!(e.cursor_col, 4, "and keeps walking");
}

#[test]
fn right_from_a_chips_end_leaves_it() {
    // A suffix after the chip, so `Right` at the chip's end has somewhere to go: in
    // `with_chip` the chip ends the buffer, where staying put is correct.
    let mut e = with_chip("look: ");
    e.insert_str(" tail");
    let end = 6 + CHIP.chars().count();
    e.cursor_col = end; // one past the chip's last char
    e.cursor_right();
    assert_eq!(
        e.cursor_col,
        end + 1,
        "one Right from the chip's end steps past it: {:?}",
        e.text()
    );
}

// And the direction the gap-1 rows *did* cover still holds, so the fix above did not
// trade exit for entry: from inside, one press still crosses the whole badge.
#[test]
fn a_caret_inside_a_chip_still_crosses_it_whole_in_both_directions() {
    let mut e = with_chip("look: ");
    let start = 6;
    let end = start + CHIP.chars().count();
    e.cursor_col = start + 5; // inside the marker
    e.cursor_left();
    assert_eq!(e.cursor_col, start, "Left lands on the chip's start");
    e.cursor_col = start + 5;
    e.cursor_right();
    assert_eq!(e.cursor_col, end, "Right lands past its end");
}
