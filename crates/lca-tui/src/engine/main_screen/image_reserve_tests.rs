//! gh #324 gap 1's renderer rows for the main screen: an image's reserved rows are
//! one block.
//!
//! Split out of `main_screen.rs` for gate 11's 1,200-line ceiling. These drive the
//! real renderer through a `FakeTerminal` and assert the emitted byte stream, so
//! they belong beside the code they exercise rather than in a sibling crate.

use super::*;
use crate::engine::terminal::FakeTerminal;

/// A kitty header reserving `rows`, with an id and scale of its own.
fn header(id: u32, cols: u32, rows: u32) -> String {
    format!("\x1b_Ga=T,f=100,q=2,C=1,c={cols},r={rows},i={id},m=0;AAAA\x1b\\")
}

// Verifies: gh #324 gap 1 - **an image's reserved rows are one block, and the
// paint loop must not walk into them.** The old loop emitted `\r\n` for every row
// including the blanks below the header, so the cursor left the block's top and
// the rest of the reserve was never claimed; the entry after the image then
// painted over it. This asserts the block shape: exactly one row carrying the
// sequence, the reserve cleared before it is placed, and the next entry's text
// arriving after the reserve is complete.
#[test]
fn a_kitty_image_paints_as_one_block_and_the_next_row_starts_below_it() {
    let (mut r, mut term) = (MainScreenRenderer::new(), FakeTerminal::new(20, 10));
    // A first frame, so the image arrives as an **append**: the full-render path
    // writes the document verbatim and has no block logic to test.
    r.render(&mut term, vec!["before".into()], 20, 10);
    let _ = term.take_output();
    r.render(
        &mut term,
        vec![
            "before".into(),
            header(7, 4, 3),
            String::new(),
            String::new(),
            "after".into(),
        ],
        20,
        10,
    );
    let out = term.take_output();
    let header_at = out.find("\x1b_Ga=T").expect("the sequence is painted");
    let after_at = out.find("after").expect("the next entry paints");
    assert!(
        after_at > header_at,
        "the next entry starts after the image sequence: {out:?}"
    );

    // Order is the whole point: **clear the reserve first, then place the
    // image.** A clear after the placement would erase the pixels it just drew; a
    // clear that never happens lets whatever was on those rows show through a
    // transparent PNG.
    let before = &out[..header_at];
    assert_eq!(
        before.matches("\x1b[2K").count(),
        3,
        "one clear per reserved row, all before the sequence: {out:?}"
    );

    // Then walk back up to the block's top, place once, and come back down.
    assert!(
        before.ends_with("\x1b[2A"),
        "the cursor walks up to the header row: {out:?}"
    );
    assert!(
        out[header_at..after_at].contains("\x1b[2B"),
        "and back down to the row below the block: {out:?}"
    );
    assert_eq!(
        out.matches("\x1b_Ga=T").count(),
        1,
        "the sequence is placed exactly once: {out:?}"
    );
}

// Verifies: gh #324 gap 1 - the block walk is **pi's exact byte shape.** pi
// (`tui-main-screen.ts`, the `imageReservedRows > 1` branch) emits
// `2K`, then `\r\n\x1b[2K` per remaining row, then `\x1b[{n}A`, the line,
// then `\x1b[{n}B`. Prose can drift from code; this pins the sequence, so a
// hand-rolled variant that "looks equivalent" fails here.
#[test]
fn the_block_walk_matches_pis_byte_shape() {
    let (mut r, mut term) = (MainScreenRenderer::new(), FakeTerminal::new(20, 10));
    r.render(&mut term, vec!["before".into()], 20, 10);
    let _ = term.take_output();
    r.render(
        &mut term,
        vec![
            "before".into(),
            header(7, 4, 3),
            String::new(),
            String::new(),
            "after".into(),
        ],
        20,
        10,
    );
    let out = term.take_output();
    let header_at = out.find("\x1b_Ga=T").expect("the sequence");
    // The walk back up is immediately before the sequence, and the down is
    // immediately after its terminator.
    let before = &out[..header_at];
    assert!(
        before.ends_with("\x1b[2K\r\n\x1b[2K\r\n\x1b[2K\x1b[2A"),
        "clear, clear, clear, then up two: {before:?}"
    );
    let after_seq = &out[header_at..];
    let terminator = after_seq.find("\x1b\\").expect("the APC terminator") + 2;
    assert!(
        after_seq[terminator..].starts_with("\x1b[2B"),
        "then back down two: {:?}",
        &after_seq[terminator..(terminator + 8).min(after_seq.len())]
    );
}

// Verifies: gh #324 gap 1 - two images in one frame both keep their own block. The
// filed bug painted the second over the first; here the second must start below
// the first's reserve and carry a different id.
#[test]
fn two_images_in_one_frame_keep_their_own_blocks() {
    let (mut r, mut term) = (MainScreenRenderer::new(), FakeTerminal::new(20, 10));
    r.render(
        &mut term,
        vec![
            header(11, 4, 2),
            String::new(),
            header(22, 4, 2),
            String::new(),
        ],
        20,
        10,
    );
    let out = term.take_output();
    let first = out.find("i=11").expect("the first image");
    let second = out.find("i=22").expect("the second image");
    assert!(second > first, "the second image paints after the first");
    assert_eq!(
        out.matches("\x1b_Ga=T").count(),
        2,
        "both images are transmitted exactly once: {out:?}"
    );
}

// Verifies: R5 - a frame of ordinary text is untouched by the image path: no
// cursor-up/down block walk appears when no row states a reserve.
#[test]
fn a_text_only_frame_walks_no_blocks() {
    let (mut r, mut term) = (MainScreenRenderer::new(), FakeTerminal::new(20, 10));
    r.render(
        &mut term,
        vec!["one".into(), String::new(), "three".into()],
        20,
        10,
    );
    let out = term.take_output();
    assert!(
        !out.contains("\x1b[2K\r\n\x1b[2K"),
        "no block walk in a text frame: {out:?}"
    );
}
