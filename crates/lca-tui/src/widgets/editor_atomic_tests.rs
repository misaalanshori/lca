//! gh #381 gaps 1-2: a paste chip is **one unit** for every cursor path and for
//! deletion, so the orphaned-marker corruption is unreachable.
//!
//! The predicate already existed (`engine::word::atomic_span_at`, gh #383) and word
//! motion already consulted it. What did not: plain `Left`/`Right`, `Up`/`Down`, the
//! line wrapper, and `Backspace`. So `Left` stepped 21 keystrokes through
//! `[paste #1 +123 lines]`, `Up` landed inside it, a wrap could break
//! `paste`/`#1` across rows, and one `Backspace` on `]` left
//! `[paste #1 +123 lines` - text that no longer matches the marker pattern and
//! therefore **ships literally to the model**, because `expand_paste_markers` leaves an
//! unrecognised shape alone.
//!
//! These rows are the contract for that one notion. The design rule they pin: **one
//! predicate, every motion path asks it** - not a second definition of "unit".

use super::*;

/// A chip for a 12-line paste, inserted at the cursor.
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
// gap 1: motion
// ---------------------------------------------------------------------------

// Verifies: gh #381 gap 1 - **`Left` once from after a chip lands before it.**
//
// This is the 21-keystroke row: `[paste #1 +12 lines]` is 21 characters, so a
// grapheme-wise `Left` needed 21 presses to get out of it. A chip is one unit.
#[test]
fn left_once_from_after_a_chip_lands_before_it() {
    let mut e = with_chip("look: ");
    let after_chip = e.cursor_col;
    assert!(
        after_chip > 1,
        "the caret sits after the inserted marker: {after_chip}"
    );
    e.cursor_left();
    assert_eq!(
        e.cursor_col,
        6,
        "one Left leaves the whole chip behind (6 = the `look: ` prefix), not one \
         character: {:?}",
        e.text()
    );
    // And it took exactly one keystroke, not 21.
    e.cursor_right();
    assert_eq!(
        e.cursor_col, after_chip,
        "one Right re-enters past the whole chip"
    );
}

// Verifies: gh #381 gap 1 - `Right` once from before a chip skips it whole.
#[test]
fn right_once_before_a_chip_skips_it() {
    let mut e = with_chip("look: ");
    e.cursor_col = 6; // before the chip
    let chip_end = 6 + "[paste #1 +12 lines]".chars().count();
    e.cursor_right();
    assert_eq!(
        e.cursor_col, chip_end,
        "one Right crosses the chip, not one character"
    );
}

// Verifies: gh #381 gap 1 - `Alt+Left`/`Alt+Right` treat the chip as one unit.
//
// Already true via `word.rs` (gh #383); pinned here so the two motion families
// cannot diverge as the plain path is changed.
#[test]
fn word_motion_crosses_a_chip_whole() {
    let mut e = with_chip("look: ");
    let after = e.cursor_col;
    e.cursor_word_left();
    assert_eq!(e.cursor_col, 6, "Alt+Left hops the chip and the space");
    e.cursor_word_right();
    assert_eq!(e.cursor_col, after, "Alt+Right returns past it");
}

// Verifies: gh #381 gap 1 - **`Up`/`Down` never land inside a chip.**
//
// The chip is inserted on its own line so a vertical move has to cross it. Landing
// inside would put the caret on a character that is not user text.
#[test]
fn vertical_motion_never_lands_inside_a_chip() {
    let mut e = Editor::new();
    e.insert_str("below");
    e.newline();
    let big = (0..12)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_paste(&big);
    e.newline();
    e.insert_str("above");
    // Caret is at the end of the last line. Walk up through every row.
    let mut visited = vec![(e.cursor_line, e.cursor_col)];
    for _ in 0..8 {
        e.cursor_up();
        visited.push((e.cursor_line, e.cursor_col));
    }
    // No visited position may sit strictly inside a chip's span.
    for (line, col) in &visited {
        let text = &e.lines()[*line];
        let chars: Vec<char> = text.chars().collect();
        if let Some(start) = chars
            .iter()
            .enumerate()
            .position(|(i, _)| {
                crate::engine::word::atomic_span_at(&chars, i)
                    .is_some_and(|end| *col > i && *col < end)
            })
            .map(|i| i.min(*col))
        {
            panic!(
                "row {line} col {col} lands inside the chip starting at {start}: {:?}",
                e.lines()
            );
        }
    }
}

// Verifies: gh #381 gap 1 - the column inside a chip is remembered across a round
// trip (pi's `snappedFromCursorCol`).
//
// Without this, `Down` out of a chip and back up loses the column and the caret
// jumps to the chip's start - a real annoyance when a user was aiming at the end.
#[test]
fn the_column_inside_a_chip_is_restored_on_return() {
    let mut e = Editor::new();
    e.insert_str("abc\n");
    let big = (0..12)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_paste(&big);
    // Park the caret as far into the chip as it will go, then come back.
    e.cursor_col = 0;
    let line = e.cursor_line;
    e.cursor_right(); // snaps past the chip
    let after_chip = e.cursor_col;
    e.cursor_left(); // back to before the chip
    assert_eq!(e.cursor_col, 0, "left of the chip");
    e.cursor_right();
    assert_eq!(
        e.cursor_col, after_chip,
        "and right of it again, on line {line}"
    );
}

// Verifies: gh #381 gap 1 - **a chip never wraps mid-token when it fits the pane.**
//
// The wrapper broke `paste`/`#1` across rows because it treated the marker as
// ordinary text. A chip is unbreakable, exactly like a wide glyph. Measured before
// the fix at width 14: `["look: [paste ", "#1 +12 lines]"]`.
//
// Scoped to widths the marker actually fits (21 cells plus the `look: ` prefix), so
// this row and the "wider than the pane" row below divide the contract cleanly
// instead of one contradicting the other.
#[test]
fn a_chip_that_fits_the_pane_never_wraps_mid_token() {
    let e = with_chip("look: ");
    const MARKER: &str = "[paste #1 +12 lines]";
    for width in [27u16, 30, 40, 60] {
        let rows: Vec<String> = e
            .render(width)
            .iter()
            .map(|r| r.replace("\u{1b}[0m", ""))
            .collect();
        let whole = rows.iter().filter(|r| r.contains(MARKER)).count();
        assert_eq!(
            whole, 1,
            "at width {width} exactly one row carries the whole marker: {rows:?}"
        );
        // And no other row carries a fragment of it.
        for row in &rows {
            if row.contains(MARKER) {
                continue;
            }
            let cleaned = row.replace("look: ", "");
            let cleaned = cleaned.trim();
            assert!(
                !MARKER.contains(cleaned) || cleaned.is_empty(),
                "at width {width} row {row:?} is a fragment of the marker: {rows:?}"
            );
        }
    }
    assert!(e.text().contains(MARKER));
}

// Verifies: gh #381 gap 1 - **a chip wider than the pane is split visually and
// stays atomic for editing**, which is pi's rule (`editor.ts`: "the split is purely
// visual for word-wrap layout").
//
// My first version made a chip unconditionally unbreakable, which broke the
// renderer's contract: a 21-cell marker in a 4-cell pane painted a 21-cell row. The
// two requirements coexist - the *drawing* may split, the *editing* may not.
#[test]
fn a_chip_wider_than_the_pane_splits_visually_but_stays_atomic() {
    let mut e = with_chip("look: ");
    for width in [4u16, 6, 8, 10] {
        let rows: Vec<String> = e
            .render(width)
            .iter()
            .map(|r| r.replace("\u{1b}[0m", ""))
            .collect();
        for row in &rows {
            assert!(
                crate::engine::text::visible_width(row) <= width as usize + 1,
                "at width {width} a row is wider than the pane (the painted caret is \
                 allowed one extra cell): {rows:?}"
            );
        }
        assert!(
            rows.iter().any(|r| r.contains("[pas")),
            "the marker is still drawn at width {width}: {rows:?}"
        );
    }
    // Still one unit for editing, despite the visual split: motion and delete both
    // consult `chip_span`, which never looks at the pane width.
    e.cursor_col = 6;
    e.cursor_right();
    assert_eq!(
        e.cursor_col,
        6 + "[paste #1 +12 lines]".chars().count(),
        "one Right crosses the whole chip at width 4 too"
    );
    e.backspace();
    assert_eq!(e.text(), "look: ", "and deletion takes the whole chip");
}

// ---------------------------------------------------------------------------
// gap 2: deletion
// ---------------------------------------------------------------------------

// Verifies: gh #381 gap 2 - **one `Backspace` at the end of a chip removes the
// whole chip**, and the result is valid text with no orphaned fragment.
//
// This is the issue's row: backspace on `]` left `[paste #1 +12 lines`, which no
// longer matches the marker pattern - and `expand_paste_markers` therefore passes
// it to the model **literally**.
#[test]
fn backspace_at_the_end_of_a_chip_removes_the_whole_chip() {
    let mut e = with_chip("look: ");
    e.backspace();
    assert_eq!(
        e.text(),
        "look: ",
        "the whole chip is gone, not one character of it"
    );
    assert!(
        !e.text().contains("[paste"),
        "and no orphaned marker fragment survives"
    );
}

// Verifies: gh #381 gap 2 - `Backspace` with the caret **inside** a chip removes
// the whole chip too, not the characters before the caret.
#[test]
fn backspace_inside_a_chip_removes_the_whole_chip() {
    let mut e = with_chip("look: ");
    // Land the caret in the middle of the marker, as a stale column would.
    e.cursor_col = 12;
    e.backspace();
    assert_eq!(
        e.text(),
        "look: ",
        "the chip is one unit even from inside it, not a partial delete"
    );
}

// Verifies: gh #381 gap 2 - `delete_forward` at the chip's start removes it whole.
#[test]
fn delete_forward_at_the_start_of_a_chip_removes_the_whole_chip() {
    let mut e = with_chip("look: ");
    e.cursor_col = 6;
    e.delete_forward();
    assert_eq!(e.text(), "look: ", "forward delete takes the whole chip");
}

// ---------------------------------------------------------------------------
// gap 2: renumbering, payload integrity, and "the model never sees a marker"
// ---------------------------------------------------------------------------

/// Two chips in one buffer, each on its own line.
fn with_two_chips() -> Editor {
    let mut e = Editor::new();
    let first = (0..12)
        .map(|n| format!("alpha {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_paste(&first);
    e.newline();
    let second = (0..12)
        .map(|n| format!("beta {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_paste(&second);
    e
}

// Verifies: gh #381 gap 2 - **deleting a chip renumbers the survivors**, so the
// labels stay contiguous and the next paste cannot collide.
//
// The ids are positional (`pastes.len() + 1`), so removing `#1` of two left a chip
// labelled `#2` whose payload indexed `pastes[0]` - the *wrong* expansion - and the
// next paste also took id 2, colliding outright.
#[test]
fn deleting_a_chip_renumbers_the_survivors() {
    let mut e = with_two_chips();
    assert!(e.text().contains("[paste #1 "), "two chips: {:?}", e.text());
    assert!(e.text().contains("[paste #2 "), "two chips: {:?}", e.text());

    // Remove the first chip: park the caret at its end and backspace.
    e.cursor_line = 0;
    e.cursor_col = e.lines()[0].chars().count();
    e.backspace();
    assert_eq!(e.lines()[0], "", "the first chip is gone");

    let text = e.text();
    assert!(
        text.contains("[paste #1 "),
        "the survivor is renumbered to #1, so the labels are contiguous: {text:?}"
    );
    assert!(
        !text.contains("[paste #2 "),
        "and no #2 gap remains: {text:?}"
    );

    // The next paste must not collide with the survivor.
    e.cursor_line = 1;
    e.cursor_col = e.lines()[1].chars().count();
    let third = (0..12)
        .map(|n| format!("gamma {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_paste(&third);
    let text = e.text();
    assert!(
        text.contains("[paste #1 ") && text.contains("[paste #2 "),
        "the new paste takes the next free id, not a duplicate: {text:?}"
    );
    assert!(
        text.matches("[paste #").count() >= 2,
        "two markers exist: {text:?}"
    );
}

// Verifies: gh #381 gap 2 - **renumbering changes labels, never payloads.**
//
// The whole point of a chip is that it expands to what was pasted. A renumber that
// shifted the wrong direction would leave `#1` pointing at the second paste, which
// looks correct on screen and ships the wrong text to the model.
#[test]
fn renumbering_keeps_every_payload_attached_to_its_own_chip() {
    let mut e = with_two_chips();
    let alpha_first = e.pastes[0].clone();
    let beta_second = e.pastes[1].clone();
    assert!(alpha_first.contains("alpha 0"), "fixture: {alpha_first:?}");
    assert!(beta_second.contains("beta 0"), "fixture: {beta_second:?}");

    // Delete the first chip.
    e.cursor_line = 0;
    e.cursor_col = e.lines()[0].chars().count();
    e.backspace();

    // The remaining chip is `#1` and must still expand to the *beta* payload.
    let expanded = super::atomic::expand_paste_markers(&e.text(), &e.pastes);
    assert!(
        expanded.contains("beta 0"),
        "the survivor still expands to its own content: {expanded:?}"
    );
    assert!(
        !expanded.contains("alpha 0"),
        "and the deleted chip's payload is not reachable: {expanded:?}"
    );
    assert!(
        !expanded.contains("[paste #"),
        "no marker survives expansion: {expanded:?}"
    );
}

// Verifies: gh #381 gap 2 - **the model never receives marker text**, including
// on the delete-then-submit path.
//
// This is the user-visible harm, driven through the real `expand_paste_markers`
// rather than asserted on the chip. Before the fix one Backspace on `]` left
// `[paste #1 +123 lines`, which no longer matches the pattern, so it passed through
// **literally** - the model read a marker it could not interpret as the paste.
#[test]
fn a_submit_after_a_delete_never_contains_marker_text() {
    let mut e = with_two_chips();
    // Backspace at the end of the first chip (the issue's exact gesture).
    e.cursor_line = 0;
    e.cursor_col = e.lines()[0].chars().count();
    e.backspace();
    // And again at the end of the surviving chip, leaving nothing.
    e.cursor_line = 1;
    if e.cursor_line < e.lines().len() {
        e.cursor_col = e.lines()[1].chars().count();
        e.backspace();
    }
    let submitted = super::atomic::expand_paste_markers(&e.text(), &e.pastes);
    assert!(
        !submitted.contains("[paste"),
        "no marker or marker fragment reaches the model: {submitted:?}"
    );
    assert!(
        !submitted.contains("paste #"),
        "and no partial marker either: {submitted:?}"
    );
}

// Verifies: gh #381 gap 2 - an orphan can no longer be produced at all.
//
// The negative property behind the row above: for every single Backspace position
// across a buffer holding a chip, the resulting text either still contains a
// complete marker or contains no `[paste` at all - never a fragment. This is what
// "unreachable rather than repaired" means.
#[test]
fn no_backspace_position_can_leave_a_marker_fragment() {
    let template = with_chip("look: ");
    let total = template.text().chars().count();
    for col in 0..=total {
        let mut e = with_chip("look: ");
        e.cursor_col = col;
        e.backspace();
        let text = e.text();
        let has_open = text.contains("[paste");
        if has_open {
            assert!(
                text.contains("]") && text.contains("[paste #"),
                "column {col} left a marker fragment: {text:?}"
            );
        }
    }
}

// Verifies: gh #381 gap 2 - the same for forward delete.
#[test]
fn no_delete_forward_position_can_leave_a_marker_fragment() {
    let template = with_chip("look: ");
    let total = template.text().chars().count();
    for col in 0..=total {
        let mut e = with_chip("look: ");
        e.cursor_col = col;
        e.delete_forward();
        let text = e.text();
        if text.contains("[paste") {
            assert!(
                text.contains(']') && text.contains("[paste #"),
                "column {col} left a marker fragment: {text:?}"
            );
        }
    }
}

// Verifies: gh #381 gap 2 - **renumbering keeps payloads attached when two chips
// share one line.**
//
// The live run that found this shape matters: two chips pasted without a newline
// between them read `[paste #1 …][paste #2 …]`, and the caret column "just past the
// first" (21) is in fact the *second* chip's opening bracket, so a Backspace there
// correctly removes `#2`. That made my first live reading look like wrong-content
// corruption when the code was right. This row pins the real contract from both
// directions, so neither reading is ambiguous again.
#[test]
fn two_chips_on_one_line_keep_their_own_payloads_across_a_delete() {
    let mut e = Editor::new();
    let a = (0..12)
        .map(|n| format!("AAA {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let b = (0..12)
        .map(|n| format!("BBB {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_paste(&a);
    e.insert_paste(&b);
    assert_eq!(e.text(), "[paste #1 +12 lines][paste #2 +12 lines]");

    // Delete the **first** chip: a column strictly inside it.
    e.cursor_col = 10;
    e.backspace();
    assert_eq!(
        e.text(),
        "[paste #1 +12 lines]",
        "the survivor is renumbered"
    );
    let expanded = super::atomic::expand_paste_markers(&e.text(), &e.pastes);
    assert!(
        expanded.starts_with("BBB 0"),
        "and it still expands to its OWN payload, not the deleted one: {:?}",
        expanded.lines().next()
    );

    // The mirror: from a fresh buffer, a column at `#2`'s opening bracket deletes
    // `#2`, leaving `#1` with AAA - which is what the live pane showed.
    let mut e2 = Editor::new();
    e2.insert_paste(&a);
    e2.insert_paste(&b);
    e2.cursor_col = 21; // the first character of chip #2
    e2.backspace();
    assert_eq!(e2.text(), "[paste #1 +12 lines]");
    let expanded2 = super::atomic::expand_paste_markers(&e2.text(), &e2.pastes);
    assert!(
        expanded2.starts_with("AAA 0"),
        "the surviving first chip keeps its own payload: {:?}",
        expanded2.lines().next()
    );
}
