//! Hover tooltips (gh #210): quadrant placement, wrapping, and the
//! hover clock. Split out for the workspace's 1,200-line file ceiling.

use super::chat;

// Verifies: gh #210 - every corner flips inside the frame, and an
// oversized tip clamps to the origin.
#[test]
fn tooltip_corners_flip_inside() {
    use crate::render::{tooltip_lines, tooltip_place};
    assert_eq!(
        tooltip_place(80, 24, 0, 0, 10, 1),
        (1, 1),
        "top-left goes below-right"
    );
    assert_eq!(
        tooltip_place(80, 24, 79, 23, 10, 1),
        (69, 22),
        "bottom-right flips above-left"
    );
    assert_eq!(
        tooltip_place(80, 24, 79, 0, 10, 1),
        (69, 1),
        "top-right flips left"
    );
    assert_eq!(
        tooltip_place(80, 24, 0, 23, 10, 1),
        (1, 22),
        "bottom-left flips up"
    );
    assert_eq!(
        tooltip_place(80, 24, 40, 12, 200, 50),
        (0, 0),
        "giant tips clamp"
    );
    let _ = tooltip_lines;
}

// Verifies: gh #210 - long text wraps at the tooltip max width.
#[test]
fn tooltip_text_wraps_at_max_width() {
    use crate::render::{TOOLTIP_MAX_WIDTH, tooltip_lines};
    let lines = tooltip_lines(
        "Toggle extension panel over a very long sentence that must wrap somewhere",
        TOOLTIP_MAX_WIDTH,
    );
    assert!(lines.len() >= 2, "wraps: {lines:?}");
    for line in &lines {
        assert!(
            lca_tui::engine::text::visible_width(line) <= TOOLTIP_MAX_WIDTH,
            "fits: {line:?}"
        );
    }
}

// Verifies: gh #210 - a stationary hover shows the tooltip; moving
// off or pressing a key dismisses it.
#[test]
fn hover_shows_and_move_or_key_hides() {
    use crate::transcript::ToolStatus;
    use std::time::{Duration, Instant};
    let mut chat = chat();
    chat.screen_mode = true;
    chat.transcript.start_tool("bash", "ls");
    chat.transcript
        .finish_tool(ToolStatus::Ok, Some("a\nb\nc\nd\ne\nf".to_string()));
    let (w, h) = (80u16, 24u16);
    // The card header paints at content row 3 (hint, separator, band).
    let text = chat.tooltip_at(4, 3, w, h, 0).expect("a tip");
    assert!(text.contains("Expand"), "names the action: {text:?}");
    let then = Instant::now() - Duration::from_millis(300);
    chat.hover_at = Some((4, 3, then));
    assert!(chat.poll_tooltip(w, h, 0, Instant::now()), "due shows");
    assert!(chat.tooltip.is_some(), "stored");
    assert!(!chat.poll_tooltip(w, h, 0, Instant::now()), "shown once");
    chat.note_hover(0, 0, w, h, 0);
    assert!(chat.tooltip.is_none(), "moving off dismisses");
    chat.hover_at = Some((4, 3, then));
    assert!(chat.poll_tooltip(w, h, 0, Instant::now()), "due again");
    chat.handle_key("q");
    assert!(chat.tooltip.is_none(), "a key dismisses");
    assert!(chat.hover_at.is_none(), "and rearms");
}

// Verifies: gh #210 - main-screen mode never tooltips (there is no
// hover without mouse tracking).
#[test]
fn main_screen_never_tooltips() {
    let chat = chat();
    assert!(chat.tooltip_at(4, 3, 80, 24, 0).is_none());
}

// Verifies: gh #210 - a due tooltip paints its text into the frame.
#[test]
fn due_tooltips_paint_into_the_frame() {
    use crate::transcript::ToolStatus;
    use std::time::{Duration, Instant};
    let mut chat = chat();
    chat.screen_mode = true;
    chat.transcript.start_tool("bash", "ls");
    chat.transcript
        .finish_tool(ToolStatus::Ok, Some("a\nb\nc\nd\ne\nf".to_string()));
    let (w, h) = (80u16, 24u16);
    chat.hover_at = Some((4, 3, Instant::now() - Duration::from_millis(300)));
    assert!(chat.poll_tooltip(w, h, 0, Instant::now()));
    let frame = chat.viewport(w, h, 0).join("\n");
    let plain: String = lca_tui::engine::text::strip_terminal_sequences(&frame);
    assert!(plain.contains("Expand output"), "the tip paints: {plain:?}");
}
