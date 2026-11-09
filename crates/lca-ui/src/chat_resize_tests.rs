//! gh #522: the frame is laid out for the width it is painted into.
//!
//! It lives in its own file and is declared by `chat_tests.rs` (a test module may
//! own test modules) because `chat.rs` sits exactly at gate 11's ceiling. The
//! behaviour is the frame-level half of the resize contract: `lca-tui`'s
//! `a_resumed_terminal_adopts_the_size_the_tty_has_now` pins the size adoption and
//! `lca-cli`'s `e2e_resize_handoff` pins the pane.

use super::{chat, strip};
use lca_tui::engine::text::visible_width;

// Verifies: gh #522 - **the frame is laid out for the size it is painted into.**
//
// The handoff defect was a *stale* width: after a resume the interface composed
// its frame from a size captured before the child had the tty, so a 140-column
// dock was painted into a 60-column terminal and tmux soft-wrapped it. The
// consequence a row can hold without a terminal is the composition itself: the
// frame's own cap rows must span exactly the width they were asked for.
//
// This is the frame-level half of the contract. `lca-tui`'s
// `a_resumed_terminal_adopts_the_size_the_tty_has_now` pins the size adoption, and
// `e2e_terminal`'s handoff row pins the pane.
#[test]
fn the_frame_is_laid_out_for_the_width_it_is_painted_into() {
    let mut chat = chat();
    // The remembered size is deliberately **wrong** here: the frame must be
    // composed from the width its caller passes, which is the size the paint is
    // about to use. A dock that reads `world.size` instead is the stale-width
    // defect, and this row then fails on the caps.
    chat.world.resize(140, 30);
    for width in [60u16, 80, 140] {
        let (transcript, dock) = chat.sections(width);
        // The editor frame's caps are the one row pair whose width is not a
        // content length: `╭` + dashes + `╮` spans the frame exactly, so any other
        // width means the composition used a different size than the paint.
        for row in dock.iter().filter(|row| {
            let plain = strip(std::slice::from_ref(row)).remove(0);
            let trimmed = plain.trim_start();
            trimmed.starts_with('╭') || trimmed.starts_with('╰')
        }) {
            let plain = strip(std::slice::from_ref(row)).remove(0);
            assert_eq!(
                visible_width(&plain),
                width as usize,
                "a cap painted at {width} columns is {width} cells: {:?}",
                plain
            );
        }
        // And nothing else may be composed wider than the frame it goes into,
        // in either section.
        for row in transcript.iter().chain(dock.iter()) {
            let plain = strip(std::slice::from_ref(row)).remove(0);
            assert!(
                lca_tui::engine::text::visible_width(&plain) <= width as usize,
                "no composed row exceeds the frame width ({width}): {:?}",
                plain
            );
        }
    }
}
