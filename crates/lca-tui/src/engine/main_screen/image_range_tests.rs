//! gh #324 gap 4's changed-range rows: a differential repaint must never cut a
//! multi-row image in half, and a replaced texture must be deleted.
//!
//! The block walk itself is gap 1's (`a_kitty_image_paints_as_one_block_...`, in
//! `image_reserve_tests.rs`). These rows are the *range* half: the renderer picks an
//! interval of document rows to repaint, and an interval that starts or ends inside an
//! image is meaningless - the terminal cannot repaint half a texture. The ghosts that
//! produces (old drawing on the untouched half, new frame on the repainted half) are the
//! "Stale Text" defect.

use super::*;
use crate::engine::terminal::FakeTerminal;

/// A kitty transmission reserving `rows`.
fn header(id: u32, rows: u32) -> String {
    format!("\x1b_Ga=T,f=100,q=2,C=1,c=4,r={rows},i={id},m=0;AAAA\x1b\\")
}

/// Render `previous`, then `next`, and return the second frame's bytes.
///
/// The rows assert the **emitted stream**, never the internal range: a test accessor that
/// calls the expansion directly cannot notice the expansion being unwired from `render`,
/// which is exactly the failure mode it exists to catch. Measured - the first version of
/// these rows did that and stayed green with the wiring removed.
fn second_frame(previous: &[String], next: &[String], height: u16) -> String {
    let mut r = MainScreenRenderer::new();
    let mut term = FakeTerminal::new(40, height);
    r.render(&mut term, previous.to_vec(), 40, height);
    let _ = term.take_output();
    r.render(&mut term, next.to_vec(), 40, height);
    term.take_output()
}

/// The rows a frame repaints, counted from its **pre-clear pass**.
///
/// The clear pass runs before the content walk (gh #18) and walks down with `\x1b[1B`
/// between clears, so `1 + (number of \x1b[1B before the first content)` is the range's
/// width. Every `\x1b[2K` in the frame would also count the block walk's per-row
/// clears - a different number, which is why my first version of these rows asserted 3
/// against an emitted 4.
fn range_rows(out: &str) -> usize {
    // Everything up to the first walk-back (`\x1b[{n}A`) is the clear pass.
    let head = match out.find("\x1b[2A") {
        Some(at) => &out[..at],
        None => out,
    };
    1 + head.matches("\x1b[1B").count()
}

// Verifies: gh #324 gap 4 - **a changed range that intersects an image expands to the
// block's boundaries.** A change on the block's last reserved row must pull the repaint
// up to its header, or the header's placement is written over a row the range never
// cleared and the block is sliced. Asserted as **bytes**: the second frame must clear all
// three of the block's rows, not just the one that changed.
#[test]
fn a_changed_row_inside_an_image_expands_to_the_whole_block() {
    // A 3-row image at document row 1: rows 1..=3 are the block.
    let previous: Vec<String> = vec![
        "before".into(),
        header(7, 3),
        String::new(),
        String::new(),
        "after".into(),
    ];
    // Change only the block's last reserved row.
    let mut next = previous.clone();
    next[3] = "late".into();
    let out = second_frame(&previous, &next, 24);
    assert_eq!(
        range_rows(&out),
        3,
        "all three of the block's rows are cleared, not just the changed one: {out:?}"
    );
    // And the placement lands once, inside the block's own walk.
    assert_eq!(out.matches("\x1b_Ga=T").count(), 1, "{out:?}");
    assert!(
        out.contains("\x1b[2A"),
        "the walk returns to the header: {out:?}"
    );
}

// Verifies: gh #324 gap 4 - **a change outside the block does not pull it in.** The
// control in the other direction: expansion must be limited to intersected blocks, or
// every frame becomes a full repaint for no reason.
#[test]
fn a_changed_row_outside_an_image_does_not_expand_to_it() {
    let previous: Vec<String> = vec![
        "before".into(),
        header(7, 3),
        String::new(),
        String::new(),
        "after".into(),
        "tail".into(),
    ];
    let mut next = previous.clone();
    next[5] = "TAIL".into();
    let out = second_frame(&previous, &next, 24);
    assert_eq!(
        range_rows(&out),
        1,
        "only the changed row is cleared: {out:?}"
    );
    assert!(
        !out.contains("\x1b_Ga=T"),
        "and the untouched image is not re-placed: {out:?}"
    );
}

// Verifies: gh #324 gap 4 - **an image taller than the viewport does not loop or
// double-emit.** A block spanning most of the frame makes almost any range a full
// repaint; that is the accepted trade, but the expansion must terminate and must not
// emit the same row twice.
#[test]
fn a_full_viewport_image_neither_loops_nor_double_emits() {
    let rows = 20u32;
    let mut previous: Vec<String> = vec![header(9, rows)];
    previous.resize(rows as usize, String::new());
    let mut next = previous.clone();
    next[(rows - 1) as usize] = "bottom".into();
    let out = second_frame(&previous, &next, 24);
    assert_eq!(
        out.matches("\x1b_Ga=T").count(),
        1,
        "the sequence is placed once, not once per row: {out:?}"
    );
    assert!(
        range_rows(&out) <= rows as usize,
        "at most the block's own rows are cleared, got {}: {out:?}",
        range_rows(&out)
    );
}

// Verifies: gh #324 gap 4 - **a replaced image is deleted by its old id.** Without this
// the terminal accumulates a texture per replaced image for the session's life.
#[test]
fn a_replaced_image_emits_a_delete_for_its_old_id() {
    let previous: Vec<String> = vec![header(11, 2), String::new(), "after".into()];
    let mut next = previous.clone();
    next[0] = header(22, 2);
    let out = second_frame(&previous, &next, 24);
    assert!(
        out.contains("\x1b_Ga=d,d=I,i=11,q=2\x1b\\"),
        "the old id is deleted: {out:?}"
    );
    assert!(out.contains("i=22"), "and the new one is placed: {out:?}");
    assert!(
        !out.contains("d=I,i=22"),
        "the new id is not deleted: {out:?}"
    );
}

// Verifies: gh #324 gap 5 (the clears-first ordering) - **the delete for the old
// texture lands before the transmission that replaces it, inside one synchronized
// update.**
//
// pi emits the clear first so the replaced texture never shows for a frame; the
// same rule applies to any terminal, not only WezTerm, and this row is the proof
// rather than a source grep: it drives a `FakeTerminal` with **no** `WEZTERM_PANE`
// in the environment, so a WezTerm-only gate would leave the frame delete-free and
// fail here. Both offsets are asserted **inside** the single `?2026h`/`?2026l`
// pair, which is the "same synchronized update" half - a delete emitted before the
// block would clear the texture for a frame before the new drawing arrived, i.e.
// the flicker this ordering exists to remove.
#[test]
fn a_replaced_image_is_cleared_before_it_is_re_placed() {
    let previous: Vec<String> = vec![header(11, 2), String::new(), "after".into()];
    let mut next = previous.clone();
    next[0] = header(22, 2);
    let out = second_frame(&previous, &next, 24);

    let open = out
        .find("\x1b[?2026h")
        .unwrap_or_else(|| panic!("the frame opens a synchronized update: {out:?}"));
    let close = out
        .rfind("\x1b[?2026l")
        .unwrap_or_else(|| panic!("and closes it: {out:?}"));
    let delete = out
        .find("\x1b_Ga=d,d=I,i=11")
        .unwrap_or_else(|| panic!("the old texture is deleted: {out:?}"));
    let place = out
        .find("i=22")
        .unwrap_or_else(|| panic!("the new one is transmitted: {out:?}"));
    assert!(
        open < delete,
        "the delete is inside the synchronized block, not before it: {out:?}"
    );
    assert!(
        delete < place,
        "and it comes **before** the transmission that replaces the row, so the old          texture never shows for a frame (delete at {delete}, place at {place}): {out:?}"
    );
    assert!(
        place < close,
        "both are inside the one synchronized update: {out:?}"
    );
    // One update, not two: a second `?2026h` would put the pair in separate blocks.
    assert_eq!(
        out.matches("\x1b[?2026h").count(),
        1,
        "the frame is a single synchronized update: {out:?}"
    );
}

// Verifies: gh #324 gap 5 - **the ordering is generic, not a WezTerm special
// case.** `docs/platform-notes.md` states the rule as universal ("the orphaned ids
// are deleted inside the synchronized block, before the repaint"), and the code
// carries no `WEZTERM_PANE` gate: this row runs with a terminal that has no
// WezTerm identity at all and still finds the delete first. If someone later
// scopes the ordering behind a terminal check, this frame loses its delete and the
// row fails.
#[test]
fn the_clears_first_ordering_does_not_depend_on_the_terminal_identity() {
    let previous: Vec<String> = vec![header(31, 2), String::new()];
    let mut next = previous.clone();
    next[0] = header(32, 2);
    let plain = second_frame(&previous, &next, 24);
    // Same document, different terminal **height**: the frame must carry the same
    // ordering, which is what "generic" means for a code path that takes no
    // terminal parameter at all.
    let shorter = second_frame(&previous, &next, 12);
    for (label, out) in [("24-row", &plain), ("12-row", &shorter)] {
        let delete = out
            .find("a=d,d=I,i=31")
            .unwrap_or_else(|| panic!("{label}: the old texture is deleted: {out:?}"));
        let place = out
            .find("i=32")
            .unwrap_or_else(|| panic!("{label}: the new one is placed: {out:?}"));
        assert!(
            delete < place,
            "{label}: delete before place, whatever the terminal: {out:?}"
        );
    }
}

// Verifies: gh #324 gap 4 + gap 3 - **a *cropped* image is not deleted.** Gap 3 made a
// crop the same id with a new placement, so a blunt "delete anything that changed" would
// delete the texture it is about to re-place. This is the row that fix fails.
#[test]
fn a_cropped_image_is_re_placed_and_never_deleted() {
    // A real cropped block: the header's `r=` is the band it occupies *now*, and the
    // reserve below it is exactly that many rows (gap 1). The uncropped previous frame
    // was 23 rows tall; the cropped one is 12, so the document shrinks with it.
    let mut previous: Vec<String> = vec!["\x1b_Ga=p,q=2,C=1,c=23,r=23,i=42\x1b\\".into()];
    previous.resize(24, String::new());
    let mut next: Vec<String> = vec!["\x1b_Ga=p,q=2,C=1,c=23,r=12,y=191,h=209,i=42\x1b\\".into()];
    next.resize(13, String::new());
    let out = second_frame(&previous, &next, 24);
    assert!(
        !out.contains("d=I"),
        "a moved texture keeps its id, so nothing is deleted: {out:?}"
    );
    assert!(
        out.contains("i=42"),
        "and the cropped placement is emitted: {out:?}"
    );
}

// Verifies: gh #324 gap 4 - **a frame with no image is unchanged.** The control: the
// expansion and the delete are image-only operations, so a text frame emits exactly what
// it did - one clear for one changed row, and no graphics traffic at all.
#[test]
fn a_text_only_frame_is_unchanged() {
    let previous: Vec<String> = vec!["one".into(), "two".into(), "three".into()];
    let mut next = previous.clone();
    next[1] = "TWO".into();
    let out = second_frame(&previous, &next, 24);
    assert!(
        !out.contains("\x1b_G"),
        "no graphics in a text frame: {out:?}"
    );
    assert!(!out.contains("a=d"), "and nothing is deleted: {out:?}");
    assert!(out.contains("TWO"), "and the change paints: {out:?}");
    assert_eq!(range_rows(&out), 1, "exactly the changed row: {out:?}");
}
