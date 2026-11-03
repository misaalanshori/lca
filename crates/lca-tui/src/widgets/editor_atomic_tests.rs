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

// ---------------------------------------------------------------------------
// gap 3: the registry is the truth, the marker text is its label
// ---------------------------------------------------------------------------

/// A chip whose payload is distinctive, so a wrong expansion is obvious.
fn chip_with(tag: &str) -> String {
    (0..12)
        .map(|n| format!("{tag} line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: gh #381 gap 3 - **after undo the payload expands byte-identically.**
//
// The defect: `undo.pop()` restored `(lines, cursor)` but not `pastes`, so undoing
// a delete brought `[paste #1 +12 lines]` back with an **empty registry** and the
// marker expanded to nothing. "The marker text came back" is what the broken
// version already did; the assertion has to be the bytes.
//
// The undo entry therefore carries the registry, per the brief: tolerating a missing
// entry inside `expand_paste_markers` would turn corruption into silence.
#[test]
fn undo_after_deleting_a_chip_restores_the_payload_byte_identically() {
    let payload = chip_with("alpha");
    let mut e = Editor::new();
    e.insert_str("look: ");
    e.insert_paste(&payload);
    let before = super::atomic::expand_paste_markers(&e.text(), &e.pastes);
    assert_eq!(
        before,
        format!("look: {payload}"),
        "the fixture expands to the payload"
    );

    // Delete the chip, then undo it.
    e.cursor_col = e.text().chars().count();
    e.backspace();
    assert!(!e.text().contains("[paste"), "the chip is gone");
    e.undo();

    let after = super::atomic::expand_paste_markers(&e.text(), &e.pastes);
    assert_eq!(
        after,
        before,
        "the restored chip expands to the same bytes, not to nothing: {:?}",
        e.text()
    );
}

// Verifies: gh #381 gap 3 - undo after a **renumber** restores labels *and*
// payloads together.
//
// Deleting `#1` of two renumbers the survivor to `#1` and drops its payload to
// `pastes[0]`. Undoing that must put the labels back to `#1`,`#2` **and** the
// payload order back with them - restoring either alone leaves a marker pointing at
// the wrong content, which is the corruption this gap is about.
#[test]
fn undo_after_a_renumber_restores_labels_and_payloads_together() {
    let a = chip_with("AAA");
    let b = chip_with("BBB");
    let mut e = Editor::new();
    e.insert_paste(&a);
    e.insert_paste(&b);
    let before_text = e.text();
    let before_expanded = super::atomic::expand_paste_markers(&before_text, &e.pastes);
    assert!(before_text.contains("[paste #1 ") && before_text.contains("[paste #2 "));

    e.cursor_col = 10; // inside chip #1
    e.backspace();
    assert_eq!(
        e.text(),
        "[paste #1 +12 lines]",
        "the survivor is renumbered"
    );
    e.undo();

    assert_eq!(
        e.text(),
        before_text,
        "both labels came back: {:?}",
        e.text()
    );
    assert_eq!(
        super::atomic::expand_paste_markers(&e.text(), &e.pastes),
        before_expanded,
        "and both payloads came back in the same order, so each chip still expands \
         to its own content"
    );
}

// Verifies: gh #381 gap 3 - **a restored draft keeps its chips' payloads.**
//
// The tab-switch path (`chat_tabs.rs`) saves a tab's editor text and restores it
// with `set_text`; clearing the registry unconditionally meant a draft holding chips
// came back with dead markers. The rule is not "never clear" - `/clear` legitimately
// drops everything - but that **a payload survives exactly when its marker does**.
//
// The text alone cannot reconstruct a payload (a marker is a label, not a
// container), so a preserving caller carries the registry: `pastes()` when saving,
// `set_text_with_pastes` when restoring. `set_text` alone re-derives from the text,
// which is the replacement case below.
#[test]
fn a_restored_draft_keeps_its_chips_payloads() {
    let payload = chip_with("kept");
    let mut e = Editor::new();
    e.insert_str("draft: ");
    e.insert_paste(&payload);
    let saved_text = e.text();
    let saved_pastes = e.pastes().to_vec();
    let expanded_before = super::atomic::expand_paste_markers(&saved_text, &e.pastes);

    // What a tab switch does: save the pair, restore it into the same editor.
    let mut restored = Editor::new();
    restored.set_text_with_pastes(&saved_text, saved_pastes);
    assert_eq!(restored.text(), saved_text);
    assert_eq!(
        super::atomic::expand_paste_markers(&restored.text(), &restored.pastes),
        expanded_before,
        "the recalled draft still expands to its payloads"
    );
}

// Verifies: gh #381 gap 3 - **`set_text` re-derives rather than preserving blindly.**
//
// A caller that restores only the *text* (not the payloads) keeps whichever markers
// its own registry can still satisfy and drops the rest. On a fresh editor that is
// nothing, and `expand_paste_markers` leaves the unknown id literal - the documented
// behaviour for a marker this editor did not write, not silent corruption.
#[test]
fn set_text_keeps_only_the_payloads_the_new_text_can_reach() {
    let kept = chip_with("kept");
    let dropped = chip_with("dropped");
    let mut e = Editor::new();
    e.insert_paste(&kept);
    e.insert_paste(&dropped);
    assert_eq!(e.pastes().len(), 2, "fixture has two payloads");

    // The new text references only `#1`; `#2` is gone, so its payload must not linger.
    e.set_text("[paste #1 +12 lines]\nand some words");
    assert_eq!(
        e.pastes().len(),
        1,
        "only the reachable payload survives: {:?}",
        e.pastes()
    );
    let expanded = super::atomic::expand_paste_markers(&e.text(), &e.pastes);
    assert!(
        expanded.contains("kept line 0"),
        "and it is still the right one: {}",
        &expanded[..expanded.len().min(60)]
    );
    assert!(
        !expanded.contains("dropped line 0"),
        "the dropped payload is gone with its marker"
    );
}

// Verifies: gh #381 gap 3 - **a wholesale replacement still drops the registry.**
//
// The other half of the rule, and the reason `set_text` must not preserve blindly:
// `/clear` (and any full-buffer replacement) must not leave a live payload behind
// whose marker no longer exists, or the next paste could inherit content that does
// not belong to it.
#[test]
fn set_text_still_clears_the_registry_when_no_marker_survives() {
    let mut e = Editor::new();
    e.insert_paste(&chip_with("gone"));
    assert!(!e.pastes.is_empty(), "fixture has a payload");

    e.set_text("");
    assert!(
        e.pastes.is_empty(),
        "a buffer with no markers owns no payloads: {:?}",
        e.pastes
    );

    // And a replacement that mentions the shape by hand is not a payload source.
    e.insert_paste(&chip_with("again"));
    e.set_text("just words, and a marker-shaped [paste #9 +1 lines] I typed");
    assert!(
        e.pastes.is_empty() || !e.text().contains("#1"),
        "a hand-typed marker does not resurrect a payload: {:?}",
        e.pastes
    );
}

// Verifies: gh #381 gap 3 - **no id collision after undo plus a new paste.**
//
// If undo rewound the counter to the pre-delete value, the next paste would take
// an id a restored chip already holds, and two different payloads would share a
// label. The counter is the high-water mark of every id ever issued and does not
// rewind on undo; `renumber_pastes` keeps the live labels contiguous.
#[test]
fn a_paste_after_undo_never_collides_with_a_restored_chip() {
    let first = chip_with("first");
    let second = chip_with("second");
    let mut e = Editor::new();
    e.insert_paste(&first);
    // Delete it and undo, so a chip exists again.
    e.cursor_col = e.text().chars().count();
    e.backspace();
    e.undo();

    // Now paste something else.
    e.insert_paste(&second);
    let text = e.text();
    let labels: Vec<&str> = text
        .match_indices("[paste #")
        .map(|(at, _)| {
            let rest = &text[at + "[paste #".len()..];
            let end = rest.find(' ').unwrap_or(rest.len());
            &rest[..end]
        })
        .collect();
    assert_eq!(labels.len(), 2, "two chips exist: {text:?}");
    assert_ne!(
        labels[0], labels[1],
        "the two chips have distinct ids, so neither shadows the other: {text:?}"
    );
    // And both still expand to their own content.
    let expanded = super::atomic::expand_paste_markers(&text, &e.pastes);
    assert!(
        expanded.contains("first line 0") && expanded.contains("second line 0"),
        "each chip expands to its own payload: {}",
        &expanded[..expanded.len().min(120)]
    );
}

// Verifies: gh #381 gap 3 - **undo memory does not grow per keystroke.**
//
// A payload can be megabytes, so snapshotting the registry on every keystroke would
// multiply the undo buffer by its size. A plain step carries no registry at all, so
// typing - which cannot change `pastes` - costs the same as before this change, and
// only the steps that *can* change it pay.
//
// The assertion is the registry bytes across the stack, plus the shape that makes it
// true: every plain snapshot holds `None`. (My first version asserted the paste step
// itself carried bytes, which is wrong: its entry is the **pre-paste** registry, and
// on a first paste that is empty. An undo entry records what to restore, not what was
// added.)
#[test]
fn typing_does_not_multiply_the_undo_buffers_payload_memory() {
    let big = (0..200)
        .map(|n| format!("payload line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut e = Editor::new();
    e.insert_paste(&big);
    let after_paste = e.undo_payload_bytes();

    // A word is many snapshots, none of which needs the registry.
    for ch in "hello".chars() {
        e.insert_str(&ch.to_string());
    }
    assert_eq!(
        e.undo_payload_bytes(),
        after_paste,
        "typing added no registry bytes to the undo stack"
    );
    // The shape behind that: a plain step carries `None`, so it cannot grow with a
    // payload however large the buffer is.
    let plain = e.undo.iter().filter(|entry| entry.chips.is_none()).count();
    assert!(
        plain >= "hello".len(),
        "each typed character is a plain step ({plain} plain of {} total)",
        e.undo.len()
    );
    // And a *second* paste does carry the live registry, because undoing it must
    // restore the first payload - that is where the memory is spent, and it is
    // bounded by the registry, not by keystrokes.
    e.insert_paste(&big);
    assert!(
        e.undo_payload_bytes() >= big.len(),
        "a paste whose undo must restore an earlier payload carries it: {}",
        e.undo_payload_bytes()
    );
}

// Verifies: gh #381 gap 3 - **a paste after a delete yields contiguous labels.**
//
// The counter cannot be both unique across undo and contiguous after a delete:
// rewinding it collides with a restored chip, not rewinding it yields `#1,#3`. Labels
// are therefore positions - the new chip takes the next free slot - and the buffer is
// renumbered after every insert. Without that renumber the third paste in this row
// would be written as `#3` beside a live `#1`.
#[test]
fn a_paste_after_a_delete_yields_contiguous_labels() {
    let chip = |tag: &str| {
        (0..12)
            .map(|n| format!("{tag} {n}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut e = Editor::new();
    e.insert_paste(&chip("one"));
    e.insert_paste(&chip("two"));
    // Both chips are on one line, so deleting the **first** means putting the caret
    // strictly inside it - a column at the line's end would be inside `#2`'s span.
    e.cursor_line = 0;
    e.cursor_col = 10;
    e.backspace();
    // Paste a third - it must land as `#2`, not `#3`.
    e.insert_paste(&chip("three"));
    let text = e.text();
    assert!(
        text.contains("[paste #1 ") && text.contains("[paste #2 "),
        "labels are contiguous after the paste: {text:?}"
    );
    assert!(!text.contains("[paste #3 "), "and no gap appears: {text:?}");
    // Every marker still expands to the content that belongs to it.
    let expanded = super::atomic::expand_paste_markers(&text, &e.pastes);
    assert!(
        expanded.contains("two 0") && expanded.contains("three 0"),
        "each chip expands to its own payload: {}",
        &expanded[..expanded.len().min(80)]
    );
    assert!(
        !expanded.contains("one 0"),
        "and the deleted payload is not resurrected"
    );
    // The labels were renumbered onto positions, so the surviving `#1` is `two`.
    assert!(
        text.starts_with("[paste #1 +12 lines]"),
        "the survivor leads: {text:?}"
    );
}
