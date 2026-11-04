//! gh #324 gap 1's alt-screen rows: an image block repaints whole.
//!
//! Split out of `alt_screen.rs` for gate 11's 1,200-line ceiling.

use super::*;
use crate::engine::terminal::FakeTerminal;

/// A kitty header reserving `rows`.
fn header(id: u32, rows: u32) -> String {
    format!("\x1b_Ga=T,f=100,q=2,C=1,c=4,r={rows},i={id},m=0;AAAA\x1b\\")
}

// Verifies: gh #324 gap 1 - **a repaint of any row of an image block repaints the
// whole block.** The alt screen diffs rows, and only the header carries the
// sequence: repainting the blanks alone paints nothing, and repainting the header
// alone erases the block's own reserved rows. Measured against a mutation that
// removes the expansion - every other row stayed green, which is why this exists.
#[test]
fn repainting_one_row_of_an_image_block_repaints_the_block() {
    let mut term = FakeTerminal::new(20, 10);
    let mut r = AltScreenRenderer::new();
    let frame = vec![header(7, 3), String::new(), String::new(), "after".into()];
    r.render_lines(&mut term, frame.clone(), 20, 10);
    let _ = term.take_output();

    // Change only the block's **last reserved row**: the header must come along,
    // or the repaint clears a row of a block whose sequence was never re-placed -
    // the image is sliced and nothing puts it back. (Changing the row *after* the
    // block correctly expands nothing, which the sibling row pins; that is why
    // the touched index here is inside the reserve.)
    let mut next = frame.clone();
    next[2] = "late".into();
    r.render_lines(&mut term, next, 20, 10);
    let out = term.take_output();
    assert!(
        out.contains("\x1b_Ga=T"),
        "the block's sequence is repainted with it: {out:?}"
    );
    assert!(
        out.contains("late"),
        "and the changed row still paints: {out:?}"
    );
}

// Verifies: gh #324 gap 1 - the expansion is a no-op for a frame with no images, so
// a text-only frame keeps its exact row set and its pinned byte shapes.
#[test]
fn a_text_only_frame_is_not_expanded() {
    let lines: Vec<String> = vec!["one".into(), "two".into(), "three".into()];
    assert_eq!(expand_for_image_blocks(&lines, &[1]), None);
    let with_image = vec![header(7, 2), String::new(), "after".into()];
    // Touching the *reserve* row (index 1) must drag in the header (index 0): the
    // reserve is blank, so a content diff never names it, and the header is the
    // only row that paints anything.
    let expanded = expand_for_image_blocks(&with_image, &[1]).expect("an expansion");
    assert_eq!(expanded, vec![0, 1], "the reserve and the header above it");
    // And touching the row after the block expands nothing: the block is already
    // painted and unchanged.
    assert_eq!(expand_for_image_blocks(&with_image, &[2]), None);
}
