//! Gh #374 gap 3 through the **real mouse path**: a double-click and a drag,
//! as `handle_mouse` sees them, rather than by calling `Selection` directly.
//!
//! The `selection`-level rows (`selection/backward_drag_tests.rs`) prove the
//! direction matrix. These prove the renderer wires it up: that the press
//! captures the initial range from the row it landed on, that motion hands
//! `update` the pointer's own row and line, and that the bytes a copy would put
//! on the clipboard are the ones the user sees. A fix that only worked when
//! `update` was called by hand would pass every row in the other file.
//!
//! Split out of `alt_screen.rs` for the workspace's 1,200-line ceiling (gate 11).

use super::*;

/// A renderer whose frame is three rows, the middle one carrying a word at a
/// known column. The rows are painted, not rendered, so `previous` is what a
/// selection reads.
fn renderer() -> AltScreenRenderer {
    let mut r = AltScreenRenderer::new();
    r.previous = vec![
        "second row here".to_string(),
        "aa bb interactive cc dd".to_string(),
        "third".to_string(),
    ];
    r.width = 40;
    r.height = 3;
    r
}

/// One click at 1-based `(x, y)`. Two in a row on the same cell is a
/// double-click within `handle_mouse`'s 500 ms window.
fn click(r: &mut AltScreenRenderer, x: u16, y: u16, press: bool) {
    r.handle_mouse(SgrMouse {
        bits: 0,
        x,
        y,
        press,
    });
}

/// A motion event (button held) at 1-based `(x, y)`.
fn motion(r: &mut AltScreenRenderer, x: u16, y: u16) {
    r.handle_mouse(SgrMouse {
        bits: 32,
        x,
        y,
        press: true,
    });
}

/// Double-click on `interactive` in row 2 (1-based y = 2), x = 9 → col 8.
fn double_click_word(r: &mut AltScreenRenderer) {
    click(r, 9, 2, true);
    click(r, 9, 2, false);
    click(r, 9, 2, true);
}

// Verifies: gh #374 gap 3 (the issue's row, through the mouse path) - a
// double-click then a drag **left** keeps the anchor word, and the copy is the
// span from the target word's start through the anchor word's end.
//
// Before the fix this row's copy was the empty string: the anchor collapsed onto
// the focus and there was nothing left to select.
#[test]
fn a_mouse_drag_left_keeps_the_double_clicked_word_in_the_copy() {
    let mut r = renderer();
    double_click_word(&mut r);
    assert_eq!(
        r.selected_text(),
        "interactive",
        "the double-click grabbed it"
    );

    // Drag to x = 2 (col 1), inside `aa`.
    motion(&mut r, 2, 2);
    click(&mut r, 2, 2, false);
    assert_eq!(
        r.selected_text(),
        "aa bb interactive",
        "the copy runs from the target word to the anchor word, which is still \
         in there"
    );
}

// Verifies: gh #374 gap 3 - the same through the mouse path, dragging **up** a
// row. The renderer has to hand `update` the pointer's row *and* that row's
// line; passing the anchor's line would re-derive the wrong boundaries.
#[test]
fn a_mouse_drag_up_keeps_the_double_clicked_word_in_the_copy() {
    let mut r = renderer();
    double_click_word(&mut r);

    // Up into row 1 (1-based y = 1), x = 3 → col 2, inside `second`.
    motion(&mut r, 3, 1);
    click(&mut r, 3, 1, false);
    assert_eq!(
        r.selected_text(),
        "second row here\naa bb interactive",
        "the whole span, the anchor row last"
    );
}

// Verifies: gh #374 gap 3 - dragging back past the anchor restores the forward
// path's bytes, through the mouse path. The row that proves the frame survives
// the round trip rather than being patched at each step.
#[test]
fn a_mouse_drag_that_reverses_matches_the_forward_drag() {
    let forward = {
        let mut r = renderer();
        double_click_word(&mut r);
        motion(&mut r, 23, 2); // x = 23 → col 22, inside `dd`
        click(&mut r, 23, 2, false);
        r.selected_text()
    };
    assert_eq!(forward, "interactive cc dd");

    let mut r = renderer();
    double_click_word(&mut r);
    motion(&mut r, 2, 2); // left into `aa`
    motion(&mut r, 23, 2); // back right past the anchor
    click(&mut r, 23, 2, false);
    assert_eq!(
        r.selected_text(),
        forward,
        "a reversed drag lands on the same bytes as going straight there"
    );
}

// Verifies: gh #374 gap 3 - a triple-click's line drag **up** keeps the anchor
// line through the mouse path, which is the line case the issue names.
#[test]
fn a_mouse_line_drag_up_keeps_the_anchor_line_in_the_copy() {
    let mut r = renderer();
    // Three clicks on the same cell inside the 500 ms window → line granularity.
    click(&mut r, 9, 2, true);
    click(&mut r, 9, 2, false);
    click(&mut r, 9, 2, true);
    click(&mut r, 9, 2, false);
    click(&mut r, 9, 2, true);
    assert_eq!(
        r.selected_text(),
        "aa bb interactive cc dd",
        "the fixture grabbed the whole row"
    );

    motion(&mut r, 3, 1);
    click(&mut r, 3, 1, false);
    assert_eq!(
        r.selected_text(),
        "second row here\naa bb interactive cc dd",
        "both lines whole, the anchor line last"
    );
}

// Verifies: gh #374 gap 3 - the **highlight** moves with the copy. A fix that
// corrected the range but left the painted rows to the old endpoints would show
// one thing and copy another, which is worse than either bug alone.
#[test]
fn the_highlight_covers_the_anchor_word_after_a_backwards_drag() {
    let mut r = renderer();
    double_click_word(&mut r);
    motion(&mut r, 2, 2);
    click(&mut r, 2, 2, false);

    let mut frame = r.previous.clone();
    r.selection.highlight(&mut frame);
    let row = &frame[1];
    // The anchor word's first cell must be inside the inverse-video run. The
    // whole selected span is `aa bb interactive`, so columns 0..17 are painted.
    let painted = crate::engine::text::strip_terminal_sequences(row);
    assert!(
        painted.contains("interactive"),
        "the row still holds the word: {painted:?}"
    );
    assert!(
        row.contains("\x1b[7m"),
        "and something is highlighted: {row:?}"
    );
    // Count the cells the reverse video covers, so a one-cell highlight (the
    // pre-fix collapse) cannot pass.
    let after = row.split("\u{1b}[7m").nth(1).unwrap_or("");
    let highlighted = crate::engine::text::visible_width(after);
    assert!(
        highlighted >= 10,
        "the highlight spans the anchor word, not a single cell: {row:?}"
    );
}

// Verifies: gh #374 gap 3 - a plain double-click with no drag still copies just
// the word. The control at the renderer level: the fix changes backward drags,
// not clicks.
#[test]
fn a_double_click_with_no_drag_still_copies_only_the_word() {
    let mut r = renderer();
    double_click_word(&mut r);
    click(&mut r, 9, 2, false);
    assert_eq!(r.selected_text(), "interactive");
}

// Verifies: gh #374 gap 3 - the edge auto-scroll path (R6) keeps the anchor for a
// **backward** extension too.
//
// It is a separate row because `tick_auto_scroll` calls `update` from its own
// place, with the edge row and that row's line - so a fix that only reasoned
// about `handle_mouse`'s motion arm can miss it. Anchoring the word on the
// *bottom* row and scrolling up makes the extension a "before" drag: the anchor
// has to become the range's end, or the last row is dropped from the span.
#[test]
fn auto_scroll_upwards_keeps_the_anchor_word() {
    let mut r = AltScreenRenderer::new();
    r.previous = vec![
        "second row here".to_string(),
        "middle row".to_string(),
        "aa bb interactive cc dd".to_string(),
    ];
    r.height = 3;
    r.width = 40;
    // The anchor word is on the last row (1-based y = 3), x = 9 -> col 8.
    click(&mut r, 9, 3, true);
    click(&mut r, 9, 3, false);
    click(&mut r, 9, 3, true);
    assert_eq!(r.selected_text(), "interactive", "the fixture grabbed it");

    // A drag up onto the top row arms the *up* edge; it only ticks when there is
    // scrollback left, so give the view some.
    r.scroll = 4;
    motion(&mut r, 5, 1);
    assert!(
        r.tick_auto_scroll(),
        "the top edge is armed and it scrolled"
    );
    // The extension targets the word under the edge cell - `tick_auto_scroll`
    // points at the row's last column, which lands inside `here` - so the span
    // starts at *that word's* start, not at column 0. The property under test is
    // the one that matters: the anchor word is still the end of the range.
    assert_eq!(
        r.selected_text(),
        "here\nmiddle row\naa bb interactive",
        "the anchor word survived as the end of the span, which the pre-fix \
         logic dropped"
    );
}

// Verifies: gh #374 gap 3 - a scrollbar column clamp (gh #35) still holds on a
// backwards drag. The two rules meet here: the range grows backwards *and* the
// adornment column stays out of the text.
#[test]
fn a_backwards_drag_still_respects_the_scrollbar_column() {
    let mut r = renderer();
    // A scrollbar at column 24 over 3 rows: `cc dd` is at 18..23, so the clamp
    // would bite if the selection reached past it.
    r.set_scrollbar(Some((24, 3)));
    double_click_word(&mut r);
    motion(&mut r, 2, 2);
    click(&mut r, 2, 2, false);
    let text = r.selected_text();
    assert_eq!(
        text, "aa bb interactive",
        "the adornment column is not in the text, and the anchor word is"
    );
}
