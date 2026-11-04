//! `MainScreenRenderer`'s unit rows: differential rendering, the append contract, the
//! caret row, and the deleted-lines arms.
//!
//! Split out of `main_screen.rs` for gate 11's 1,200-line ceiling.

use super::*;
use crate::engine::terminal::FakeTerminal;

fn renderer() -> (MainScreenRenderer, FakeTerminal) {
    (MainScreenRenderer::new(), FakeTerminal::new(20, 10))
}

#[test]
fn first_render_writes_all_lines() {
    let (mut r, mut term) = renderer();
    r.render(&mut term, vec!["one".into(), "two".into()], 20, 10);
    let out = term.take_output();
    assert!(out.contains("one"));
    assert!(out.contains("two"));
}

#[test]
fn append_uses_real_newlines_for_scrollback() {
    let (mut r, mut term) = renderer();
    r.render(&mut term, vec!["one".into()], 20, 10);
    let _ = term.take_output();
    r.render(&mut term, vec!["one".into(), "two".into()], 20, 10);
    let out = term.take_output();
    assert!(
        out.contains("\r\n\x1b[2Ktwo"),
        "appends with newline: {out:?}"
    );
}

#[test]
fn in_place_change_moves_up_and_rewrites() {
    let (mut r, mut term) = renderer();
    r.render(
        &mut term,
        vec!["one".into(), "two".into(), "three".into()],
        20,
        10,
    );
    let _ = term.take_output();
    r.render(
        &mut term,
        vec!["ONE".into(), "two".into(), "three".into()],
        20,
        10,
    );
    let out = term.take_output();
    assert!(out.contains("\x1b[2A"), "moves up 2 lines: {out:?}");
    assert!(out.contains("ONE"));
}

// Verifies: FR-UI-24 (R5) - a frame whose only change is the caret
// column still repaints the caret's row, so a typed space is visible
// on the next frame rather than on the next letter.
#[test]
fn a_caret_only_move_repaints_its_row() {
    use crate::engine::core::CURSOR_MARKER;
    let (mut r, mut term) = renderer();
    r.render(
        &mut term,
        vec![format!("ab{CURSOR_MARKER} "), "footer".into()],
        20,
        10,
    );
    let _ = term.take_output();
    r.render(
        &mut term,
        vec![format!("ab {CURSOR_MARKER}"), "footer".into()],
        20,
        10,
    );
    let out = term.take_output();
    assert!(
        out.contains("\r\x1b[2Kab "),
        "the caret row repaints: {out:?}"
    );
    assert!(out.contains("\x1b[4G"), "and the column moves: {out:?}");
    assert!(
        !out.contains("footer"),
        "the other rows are untouched: {out:?}"
    );
}

#[test]
fn overlong_lines_are_truncated_not_fatal() {
    let (mut r, mut term) = renderer();
    r.render(&mut term, vec!["x".repeat(100)], 20, 10);
    let out = term.take_output();
    assert!(out.contains("x"));
}

// Verifies: S3 (issue #7) - mouse capture policy follows the renderer mode:
// the default main-screen mode never enables mouse capture (?1000h, ?1002h, ?1003h, ?1006h),
// guaranteeing native selection, right-click paste, and Ctrl+V work by construction.
#[test]
fn main_screen_never_enables_mouse_tracking() {
    let (mut r, mut term) = renderer();
    r.render(&mut term, vec!["one".into(), "two".into()], 20, 10);
    r.render(&mut term, vec!["one".into(), "three".into()], 20, 10);
    let out = term.take_output();
    for seq in ["\x1b[?1000h", "\x1b[?1002h", "\x1b[?1003h", "\x1b[?1006h"] {
        assert!(
            !out.contains(seq),
            "the main screen must not emit {seq:?}:\n{out:?}"
        );
    }
}

// ---------------------------------------------------------------------
// gh #371 bug 1: a caret above the viewport must not corrupt the frame.
// ---------------------------------------------------------------------

/// A document taller than the pane, with the caret on `caret_row`.
fn tall_prompt(caret_row: usize, total: usize) -> Vec<String> {
    use crate::engine::core::CURSOR_MARKER;
    (0..total)
        .map(|row| {
            if row == caret_row {
                format!("line {row}{CURSOR_MARKER}")
            } else {
                format!("line {row}")
            }
        })
        .collect()
}

/// Every row the renderer believes the hardware cursor may be on.
///
/// The invariant gh #371 is about: after any frame, the recorded row must be a
/// row the terminal can physically be on - i.e. inside the visible viewport.
fn assert_recorded_row_is_on_screen(r: &MainScreenRenderer, height: u16, frame: &str) {
    let top = r.previous_viewport_top;
    let bottom = top + height as usize;
    assert!(
        r.hardware_cursor_row >= top && r.hardware_cursor_row < bottom,
        "after {frame}, hardware_cursor_row={} is outside the viewport \
         [{top}, {bottom}) - the terminal cannot be there, so the next frame's diff \
         is computed from a screen row that does not exist",
        r.hardware_cursor_row
    );
}

// Verifies: gh #371 bug 1 - the recorded cursor row is always addressable.
//
// The caret sits on a line that has scrolled above the viewport (a prompt taller
// than the pane, caret moved to its top). The old code emitted a cursor-up past row
// 0 (the terminal clamps at the viewport top) and then recorded the *requested* row,
// so the field described a place the terminal never went.
#[test]
fn a_caret_above_the_viewport_is_never_recorded_as_a_cursor_row() {
    let (mut r, mut term) = renderer();
    term.resize(20, 4);
    // 40 lines in a 4-row pane: rows 0..35 have scrolled out.
    r.render(&mut term, tall_prompt(38, 40), 20, 4);
    let _ = term.take_output();
    assert_recorded_row_is_on_screen(&r, 4, "the first frame");

    // The caret moves to row 2 - far above the 4-row viewport at 36..40.
    r.render(&mut term, tall_prompt(2, 40), 20, 4);
    let out = term.take_output();
    assert_recorded_row_is_on_screen(&r, 4, "a caret above the viewport");
    // And no cursor-up sequence beyond the tallest possible viewport can appear:
    // a 4-row pane can address at most 3 rows up from its bottom.
    for up in 4..40 {
        assert!(
            !out.contains(&format!("\x1b[{up}A")),
            "the frame moved the cursor {up} rows up in a 4-row pane: {out:?}"
        );
    }
}

// Verifies: gh #371 bug 1 - the *next* frame's diff is correct, which is the
// damage the issue describes rather than the clamp itself.
//
// A single-frame assertion cannot see it: frame 1 records a bogus row, and frame 2
// is the one that computes a negative screen row and renders into the wrong place.
#[test]
fn the_frame_after_a_scrolled_out_caret_still_renders_the_right_rows() {
    let (mut r, mut term) = renderer();
    term.resize(20, 4);
    r.render(&mut term, tall_prompt(38, 40), 20, 4);
    let _ = term.take_output();
    // Frame 2: the caret scrolled out above the viewport.
    r.render(&mut term, tall_prompt(2, 40), 20, 4);
    assert_recorded_row_is_on_screen(&r, 4, "the caret left the viewport");
    let _ = term.take_output();
    // Frame 3: ordinary typing at the bottom - the diff must land on the rows it
    // names, which it cannot do if the recorded row was never reachable.
    let mut lines = tall_prompt(2, 40);
    lines[39] = "line 39 typed".to_string();
    r.render(&mut term, lines, 20, 4);
    let out = term.take_output();
    assert!(
        out.contains("line 39 typed"),
        "the changed row is written: {out:?}"
    );
    assert_recorded_row_is_on_screen(&r, 4, "the frame after");
}

// Verifies: gh #371 - the corruption is cumulative, so a journey of frames is what
// catches it. Each frame is checked, not just the last.
#[test]
fn a_tall_prompt_journey_keeps_every_frame_addressable() {
    let (mut r, mut term) = renderer();
    term.resize(20, 4);
    r.render(&mut term, tall_prompt(39, 40), 20, 4);
    let _ = term.take_output();
    assert_recorded_row_is_on_screen(&r, 4, "frame 0");
    // Walk the caret up out of the viewport and back down, one frame each.
    for caret in [30usize, 20, 10, 2, 1, 0, 5, 12, 25, 36, 39] {
        r.render(&mut term, tall_prompt(caret, 40), 20, 4);
        let _ = term.take_output();
        assert_recorded_row_is_on_screen(&r, 4, &format!("caret at {caret}"));
    }
}

// Verifies: gh #371 - the invariant over *every* frame shape, not just the ones
// above.
//
// The three rows before this one drive the shapes I thought of. This one drives a
// mixed journey - appends, in-place edits, deletions, caret jumps above and below
// the viewport, and resizes - because gh #371's desync is cumulative and the write
// sites are spread across four render paths. A row per path would be a row per
// guess; this asserts the property each of them owes.
#[test]
fn every_frame_shape_keeps_the_cursor_row_addressable() {
    let (mut r, mut term) = renderer();
    term.resize(20, 4);
    let mut lines = tall_prompt(39, 40);
    r.render(&mut term, lines.clone(), 20, 4);
    let _ = term.take_output();
    assert_recorded_row_is_on_screen(&r, 4, "initial");

    // A scripted journey: each step is a frame the real interface can produce.
    let steps: Vec<Vec<String>> = vec![
        // In-place edit at the bottom.
        {
            lines[39] = "line 39 edited".to_string();
            lines.clone()
        },
        // Caret leaps to the top of the document (the bug's trigger).
        tall_prompt(0, 40),
        // Caret mid-document, still above the viewport.
        tall_prompt(20, 40),
        // Back to the bottom.
        tall_prompt(39, 40),
        // A deletion: two rows vanish, everything shifts.
        tall_prompt(39, 38),
        // Growth past the pane again.
        tall_prompt(39, 60),
        // Caret parked above a grown document.
        tall_prompt(3, 60),
        // An ordinary append at the bottom.
        tall_prompt(60, 61),
    ];
    for (step, step_lines) in steps.iter().enumerate() {
        r.render(&mut term, step_lines.clone(), 20, 4);
        let _ = term.take_output();
        assert_recorded_row_is_on_screen(&r, 4, &format!("step {step}"));
    }
}

// Verifies: gh #371 bug 1's second half - a saturated row cannot be trusted
// either.
//
// `extract_cursor_position` stores the document row in a `u16` and saturates at
// `u16::MAX`. A document longer than that puts the marker on a row the reported
// position cannot express, so the recorded row would be *lower* than the truth (and
// the caret would be parked somewhere it is not). The viewport guard treats it as
// unaddressable, which is the honest answer for a row we cannot name.
#[test]
fn a_saturated_cursor_row_is_treated_as_unaddressable() {
    let (mut r, mut term) = renderer();
    term.resize(20, 4);
    // A document taller than u16::MAX, with the caret on its first line: the
    // extractor saturates, and the guard must not then position at 65535.
    let total = usize::from(u16::MAX) + 10;
    r.render(&mut term, tall_prompt(0, total), 20, 4);
    let out = term.take_output();
    assert_recorded_row_is_on_screen(&r, 4, "a saturated caret row");
    assert!(
        out.contains("\x1b[?25l"),
        "an unnameable caret hides the hardware cursor: {out:?}"
    );
    assert!(
        !out.contains("\x1b[65535A") && !out.contains("\x1b[65536A"),
        "and never asks the terminal to move to a row it cannot reach: {out:?}"
    );
}

// Verifies: gh #371 bug 1 - the *mechanism* the issue names, stated as a row.
//
// The issue is precise: with the caret above the viewport, `compute_line_diff` gets
// `hw_cursor - p_view_top < 0` - a screen row above the top of the terminal. This row
// reconstructs that state through the public API and asserts the invariant that
// prevents it, rather than asserting the arithmetic itself (which is correct either
// way): after such a frame, no later frame may compute a negative screen row, which
// is the same statement as "the recorded row is inside the viewport".
#[test]
fn a_frame_never_leaves_a_negative_screen_row_behind() {
    let (mut r, mut term) = renderer();
    term.resize(20, 4);
    // 50 lines, caret at the very top: above the 4-row viewport.
    r.render(&mut term, tall_prompt(0, 50), 20, 4);
    let _ = term.take_output();
    for frame in 0..5 {
        let top = r.previous_viewport_top;
        let screen_row = r.hardware_cursor_row as i64 - top as i64;
        assert!(
            screen_row >= 0,
            "frame {frame}: the recorded row {} is above viewport_top {top}, so the \
             next `compute_line_diff` would start from screen row {screen_row} - \
             gh #371's desynchronisation",
            r.hardware_cursor_row
        );
        // Keep the caret above the viewport and repaint, which is the cumulative
        // case: the corruption appears on a *later* frame, not the first.
        let mut lines = tall_prompt(0, 50);
        lines[49] = format!("line 49 frame {frame}");
        r.render(&mut term, lines, 20, 4);
        let _ = term.take_output();
    }
}

// Verifies: S1 - genuine scrollback append as content exceeds terminal height.
#[test]
fn scrollback_growth_emits_newlines_without_clearing_screen() {
    let (mut r, mut term) = renderer(); // 20 cols, 4 rows
    term.resize(20, 4);
    // Start with 3 lines
    r.render(
        &mut term,
        vec!["line 1".into(), "line 2".into(), "line 3".into()],
        20,
        4,
    );
    let _ = term.take_output();

    // Append line 4 (fills height)
    r.render(
        &mut term,
        vec![
            "line 1".into(),
            "line 2".into(),
            "line 3".into(),
            "line 4".into(),
        ],
        20,
        4,
    );
    let out4 = term.take_output();
    assert!(!out4.contains("\x1b[2J"), "no screen clear on append");
    assert!(out4.contains("\r\n\x1b[2Kline 4"));

    // Append line 5 (scrolls past height: 5 > 4)
    r.render(
        &mut term,
        vec![
            "line 1".into(),
            "line 2".into(),
            "line 3".into(),
            "line 4".into(),
            "line 5".into(),
        ],
        20,
        4,
    );
    let out5 = term.take_output();
    assert!(!out5.contains("\x1b[2J"), "no screen clear when scrolling");
    assert!(out5.contains("\r\n\x1b[2Kline 5"));

    // In-place edit of line 5 (e.g. spinner tick or typing)
    r.render(
        &mut term,
        vec![
            "line 1".into(),
            "line 2".into(),
            "line 3".into(),
            "line 4".into(),
            "line 5 edited".into(),
        ],
        20,
        4,
    );
    let out_edit = term.take_output();
    assert!(
        !out_edit.contains("\x1b[2J"),
        "no screen clear on in-place edit"
    );
    assert!(out_edit.contains("\r\x1b[2Kline 5 edited"));
}
// ============================================================================
// gh #372 bug 3 - a Termux soft-keyboard height change must not wipe scrollback
// ============================================================================

/// A renderer told it is (or is not) running on Termux.
fn termux_renderer(on_termux: bool) -> MainScreenRenderer {
    let mut renderer = MainScreenRenderer::new();
    renderer.set_termux_session(on_termux);
    renderer
}

/// Paint a few frames so the renderer has a previous frame and some scrollback.
fn prime(renderer: &mut MainScreenRenderer, term: &mut FakeTerminal, height: u16) {
    for i in 1..=3 {
        let lines: Vec<String> = (0..i).map(|n| format!("line {n}")).collect();
        renderer.render(term, lines, 20, height);
        let _ = term.take_output();
    }
}

// Verifies: gh #372 bug 3 (**the defect**) - on Termux, showing or hiding the soft
// keyboard changes `height` by 10-15 rows every few seconds, and each change ran
// `full_render(true, ..)` → `\x1b[2J\x1b[H\x1b[3J`, wiping the screen **and the
// scrollback** repeatedly. A height-only change needs no rebuild: wrapping does not
// depend on height, so the visible viewport can simply re-anchor.
#[test]
fn a_height_only_change_on_termux_does_not_wipe_the_scrollback() {
    let mut r = termux_renderer(true);
    let mut term = FakeTerminal::new(20, 10);
    // The Termux soft keyboard changes the height and nothing else: the content is
    // exactly what was last painted.
    let content: Vec<String> = (0..3).map(|n| format!("line {n}")).collect();
    r.render(&mut term, content.clone(), 20, 10);
    let _ = term.take_output();
    r.render(&mut term, content, 20, 8);
    let out = term.take_output();
    assert!(
        !out.contains("\x1b[3J"),
        "a soft-keyboard height change must not clear the scrollback: {out:?}"
    );
    assert!(!out.contains("\x1b[2J"), "...nor the screen: {out:?}");
}

// Verifies: gh #372 bug 3 (the row that keeps the exemption **narrow**) - a **width**
// change on Termux still does the full render, because wrapping genuinely changes.
// An exemption from "height changed" must not become an exemption from "anything
// changed".
#[test]
fn a_width_change_on_termux_still_does_the_full_render() {
    let mut r = termux_renderer(true);
    let mut term = FakeTerminal::new(20, 10);
    prime(&mut r, &mut term, 10);
    // Width 20 -> 30, height unchanged.
    r.render(&mut term, vec!["line 0".into()], 30, 10);
    let out = term.take_output();
    assert!(
        out.contains("\x1b[3J") && out.contains("\x1b[2J"),
        "a width change invalidates wrapping, so the full render still runs: {out:?}"
    );
}

// Verifies: gh #372 bug 3 - a terminal that is **not** Termux keeps the old
// behaviour exactly: a height change re-aligns the viewport with a full render.
// The exemption is scoped to the detection, not global.
#[test]
fn a_height_change_elsewhere_still_does_the_full_render() {
    let mut r = termux_renderer(false);
    let mut term = FakeTerminal::new(20, 10);
    prime(&mut r, &mut term, 10);
    r.render(&mut term, vec!["line 0".into(), "line 1".into()], 20, 8);
    let out = term.take_output();
    assert!(
        out.contains("\x1b[3J"),
        "off Termux a height change still rebuilds the viewport: {out:?}"
    );
}

// ============================================================================
// gh #372 bug 4 - `terminal.clear_on_shrink` stops being documented-inert
// ============================================================================

/// A renderer with the clear-on-shrink setting as the user configured it.
fn shrink_renderer(clear: bool) -> MainScreenRenderer {
    let mut renderer = MainScreenRenderer::new();
    renderer.set_clear_on_shrink(clear);
    renderer
}

// Verifies: gh #372 bug 4 - with the setting **on**, content that shrinks below what
// was last painted clears the emptied rows. That is pi's `terminal.clearOnShrink`
// (`tui-main-screen.ts:357`).
#[test]
fn clear_on_shrink_clears_emptied_rows_when_enabled() {
    let mut r = shrink_renderer(true);
    let mut term = FakeTerminal::new(20, 20);
    r.render(&mut term, vec!["a".into(), "b".into(), "c".into()], 20, 20);
    let _ = term.take_output();
    r.render(&mut term, vec!["a".into()], 20, 20);
    let out = term.take_output();
    assert!(
        out.contains("\x1b[2K") || out.contains("\x1b[2J"),
        "the emptied rows must be cleared: {out:?}"
    );
}

// Verifies: gh #372 bug 4 - with the setting **off** (the default) nothing changes: a
// config key that defaults to changing behaviour is not a setting.
#[test]
fn clear_on_shrink_is_off_by_default() {
    let mut r = shrink_renderer(false);
    let mut term = FakeTerminal::new(20, 20);
    r.render(&mut term, vec!["a".into(), "b".into(), "c".into()], 20, 20);
    let _ = term.take_output();
    r.render(&mut term, vec!["a".into()], 20, 20);
    let out = term.take_output();
    assert!(
        !out.contains("\x1b[3J"),
        "off means no scrollback-clearing re-render: {out:?}"
    );
}

// Verifies: gh #372 bug 4 - **a grow does not clear**. The setting is about emptied
// rows; clearing on growth would throw away scrollback on every append.
#[test]
fn clear_on_shrink_does_not_fire_when_content_grows() {
    let mut r = shrink_renderer(true);
    let mut term = FakeTerminal::new(20, 20);
    r.render(&mut term, vec!["a".into()], 20, 20);
    let _ = term.take_output();
    r.render(&mut term, vec!["a".into(), "b".into(), "c".into()], 20, 20);
    let out = term.take_output();
    assert!(
        !out.contains("\x1b[3J"),
        "growth must not clear the scrollback: {out:?}"
    );
}

// Verifies: gh #372 bug 3 - the detection is a **named predicate keyed on Termux's own
// variable**, not a `TERM=screen` guess: screen and tmux are not Termux, and Termux
// reports `TERMUX_VERSION`.
#[test]
fn the_termux_detection_uses_termuxs_own_variable() {
    let with_version = |key: &str| (key == "TERMUX_VERSION").then(|| "0.118".to_string());
    assert!(
        is_termux_session_with(with_version),
        "TERMUX_VERSION marks Termux"
    );
    let screen = |key: &str| (key == "TERM").then(|| "screen-256color".to_string());
    assert!(
        !is_termux_session_with(screen),
        "`TERM=screen` is tmux and GNU screen too, so it is not the signal"
    );
    assert!(
        !is_termux_session_with(|_| None),
        "and absent is not Termux"
    );
}
