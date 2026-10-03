//! Main-screen (scrollback) renderer, ported from pi's
//! `packages/tui/src/tui-main-screen.ts`
//! (`pi-tui-re/src_re/tui-engine/tui-main-screen.md`).
//!
//! The UI renders into the terminal's main screen and scrollback, which is
//! why the transcript stays selectable and survives as history. The hot path
//! is a line diff: only the changed range is repainted, and growth appends
//! with real newlines so the terminal scrolls rather than overwrites.
//!
//! Porting notes from pi:
//! - Full document lines are passed to `render`, allowing real incremental appends
//!   and terminal scrollback accumulation.
//! - State tracks `previous_lines`, `previous_width`, `previous_height`, `cursor_row`,
//!   `hardware_cursor_row`, `max_lines_rendered`, and `previous_viewport_top`.
//! - `compute_line_diff` uses screen-row arithmetic against the moving viewport.
//! - When appending past the viewport bottom, newlines (`\r\n`) are emitted to scroll
//!   and `previous_viewport_top` moves forward.
//! - `append_start` distinguishes true append (begins with `\r\n`) from in-place edit (`\r`).
//! - Changed range is only first_changed..=last_changed.
//! - Width invariant defensively truncates rather than crashes.

use super::core::{SEGMENT_RESET, extract_cursor_position, width_violation};
use super::terminal::Terminal;
use super::text::truncate_to_width;

/// The main-screen differential renderer.
#[derive(Debug, Default)]
pub struct MainScreenRenderer {
    previous_lines: Vec<String>,
    previous_width: u16,
    previous_height: u16,
    /// Logical index of the end of content.
    cursor_row: usize,
    /// Logical index of the hardware cursor.
    hardware_cursor_row: usize,
    /// High-water mark of rendered lines.
    max_lines_rendered: usize,
    /// Logical top of the previous viewport.
    previous_viewport_top: usize,
    /// The caret from the last frame (row, col).
    last_cursor: Option<(u16, u16)>,
}

impl MainScreenRenderer {
    /// A fresh renderer.
    pub fn new() -> Self {
        Self::default()
    }

    /// The lines from the last render.
    pub fn previous(&self) -> &[String] {
        &self.previous_lines
    }

    /// Reset internal render state (e.g. for full clear).
    pub fn reset_render_state(&mut self) {
        self.previous_lines.clear();
        self.previous_width = 0;
        self.previous_height = 0;
        self.cursor_row = 0;
        self.hardware_cursor_row = 0;
        self.max_lines_rendered = 0;
        self.previous_viewport_top = 0;
        self.last_cursor = None;
    }

    /// Render `lines` into the terminal, returning the hardware cursor
    /// position (row, col) if a `CURSOR_MARKER` was present.
    pub fn render(
        &mut self,
        term: &mut dyn Terminal,
        lines: Vec<String>,
        width: u16,
        height: u16,
    ) -> Option<(u16, u16)> {
        if width == 0 || height == 0 {
            return None;
        }

        let (mut new_lines, cursor_pos) = extract_cursor_position(&lines);

        // Width invariant: defensively truncate rather than crash.
        if let Some((_row, _w)) = width_violation(&new_lines, width) {
            for line in &mut new_lines {
                if super::text::visible_width(line) > width as usize {
                    *line = truncate_to_width(line, width as usize, "", false);
                }
            }
        }

        let width_changed = self.previous_width != 0 && self.previous_width != width;
        let height_changed = self.previous_height != 0 && self.previous_height != height;
        let prev_buf_len = if self.previous_height > 0 {
            self.previous_viewport_top + self.previous_height as usize
        } else {
            height as usize
        };
        let mut prev_viewport_top = if height_changed {
            prev_buf_len.saturating_sub(height as usize)
        } else {
            self.previous_viewport_top
        };
        let mut viewport_top = prev_viewport_top;
        let mut hardware_cursor_row = self.hardware_cursor_row;

        // Screen row of a document row: target - viewport_top
        // Line diff: target_screen_row - current_screen_row
        let compute_line_diff =
            |target_row: usize, hw_cursor: usize, p_view_top: usize, view_top: usize| -> i64 {
                let current_screen_row = hw_cursor as i64 - p_view_top as i64;
                let target_screen_row = target_row as i64 - view_top as i64;
                target_screen_row - current_screen_row
            };

        // Full render helper
        let mut full_render = |clear: bool, renderer: &mut Self| {
            let mut out = String::new();
            out.push_str("\x1b[?2026h"); // synchronized output
            if clear {
                out.push_str("\x1b[2J\x1b[H\x1b[3J"); // Clear screen, home, clear scrollback
            }
            for (i, line) in new_lines.iter().enumerate() {
                if i > 0 {
                    out.push_str("\r\n");
                }
                out.push_str(line);
            }
            out.push_str(SEGMENT_RESET);
            out.push_str("\x1b[?2026l");
            term.write(&out);

            renderer.cursor_row = new_lines.len().saturating_sub(1);
            renderer.hardware_cursor_row = renderer.cursor_row;
            if clear {
                renderer.max_lines_rendered = new_lines.len();
            } else {
                renderer.max_lines_rendered = renderer.max_lines_rendered.max(new_lines.len());
            }
            let buffer_length = (height as usize).max(new_lines.len());
            renderer.previous_viewport_top = buffer_length.saturating_sub(height as usize);
            renderer.position_hardware_cursor(term, cursor_pos, new_lines.len());
            renderer.previous_lines = new_lines.clone();
            renderer.previous_width = width;
            renderer.previous_height = height;
            renderer.last_cursor = cursor_pos.map(|(r, c, _)| (r, c));
        };

        // First render: output everything without clearing (assumes clean terminal/scrollback start)
        if self.previous_lines.is_empty() && !width_changed && !height_changed {
            full_render(false, self);
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        // Width changes invalidate wrapping: full clear + re-render
        if width_changed {
            full_render(true, self);
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        // Height changes need a full redraw to re-align visible viewport
        if height_changed {
            full_render(true, self);
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        // Find first and last changed lines
        let mut first_changed: Option<usize> = None;
        let mut last_changed: Option<usize> = None;
        let max_lines = new_lines.len().max(self.previous_lines.len());
        for i in 0..max_lines {
            let old_line = self.previous_lines.get(i).map(String::as_str).unwrap_or("");
            let new_line = new_lines.get(i).map(String::as_str).unwrap_or("");
            if old_line != new_line {
                if first_changed.is_none() {
                    first_changed = Some(i);
                }
                last_changed = Some(i);
            }
        }

        let appended_lines = new_lines.len() > self.previous_lines.len();
        if appended_lines {
            if first_changed.is_none() {
                first_changed = Some(self.previous_lines.len());
            }
            last_changed = Some(new_lines.len() - 1);
        }

        let append_start = appended_lines
            && first_changed == Some(self.previous_lines.len())
            && !self.previous_lines.is_empty();

        // Caret move check
        let cursor_moved = cursor_pos.map(|(r, c, _)| (r, c)) != self.last_cursor;

        // No text changes
        if first_changed.is_none() {
            if cursor_moved && let Some((row, _, _)) = cursor_pos {
                // If only the caret moved, repaint its row to ensure visible caret update
                let doc_row = (row as usize).min(new_lines.len().saturating_sub(1));
                let line_diff = compute_line_diff(
                    doc_row,
                    hardware_cursor_row,
                    prev_viewport_top,
                    viewport_top,
                );
                let mut out = String::new();
                out.push_str("\x1b[?2026h");
                if line_diff > 0 {
                    out.push_str(&format!("\x1b[{}B", line_diff));
                } else if line_diff < 0 {
                    out.push_str(&format!("\x1b[{}A", -line_diff));
                }
                out.push_str("\r\x1b[2K");
                out.push_str(&new_lines[doc_row]);
                out.push_str(SEGMENT_RESET);
                out.push_str("\x1b[?2026l");
                term.write(&out);
                self.hardware_cursor_row = doc_row;
            }
            self.position_hardware_cursor(term, cursor_pos, new_lines.len());
            self.previous_viewport_top = prev_viewport_top;
            self.previous_height = height;
            self.last_cursor = cursor_pos.map(|(r, c, _)| (r, c));
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        let first_changed_idx = first_changed.unwrap_or(0);
        let last_changed_idx = last_changed.unwrap_or(first_changed_idx);

        // All changes are in deleted lines (nothing to render, just clear trailing)
        if first_changed_idx >= new_lines.len() {
            if self.previous_lines.len() > new_lines.len() {
                let target_row = new_lines.len().saturating_sub(1);
                if target_row < prev_viewport_top {
                    full_render(true, self);
                    return cursor_pos.map(|(r, c, _)| (r, c));
                }
                let line_diff = compute_line_diff(
                    target_row,
                    hardware_cursor_row,
                    prev_viewport_top,
                    viewport_top,
                );
                let mut out = String::new();
                out.push_str("\x1b[?2026h");
                if line_diff > 0 {
                    out.push_str(&format!("\x1b[{}B", line_diff));
                } else if line_diff < 0 {
                    out.push_str(&format!("\x1b[{}A", -line_diff));
                }
                out.push('\r');
                let extra_lines = self.previous_lines.len() - new_lines.len();
                if extra_lines > height as usize {
                    full_render(true, self);
                    return cursor_pos.map(|(r, c, _)| (r, c));
                }
                let clear_start_offset = if new_lines.is_empty() { 0 } else { 1 };
                if extra_lines > 0 && clear_start_offset > 0 {
                    out.push_str(&format!("\x1b[{}B", clear_start_offset));
                }
                for i in 0..extra_lines {
                    out.push_str("\r\x1b[2K");
                    if i + 1 < extra_lines {
                        out.push_str("\x1b[1B");
                    }
                }
                let move_back = extra_lines.saturating_sub(1) + clear_start_offset;
                if move_back > 0 {
                    out.push_str(&format!("\x1b[{}A", move_back));
                }
                out.push_str("\x1b[?2026l");
                term.write(&out);
                self.cursor_row = target_row;
                self.hardware_cursor_row = target_row;
            }
            self.position_hardware_cursor(term, cursor_pos, new_lines.len());
            self.previous_lines = new_lines;
            self.previous_width = width;
            self.previous_height = height;
            self.previous_viewport_top = prev_viewport_top;
            self.last_cursor = cursor_pos.map(|(r, c, _)| (r, c));
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        // Differential rendering can only touch what was actually visible on the terminal screen.
        // If the first changed line is above the visible viewport, fall back to a full redraw.
        if first_changed_idx < prev_viewport_top {
            full_render(true, self);
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        let mut out = String::new();
        out.push_str("\x1b[?2026h");

        let prev_viewport_bottom = prev_viewport_top + height as usize - 1;
        let move_target_row = if append_start {
            first_changed_idx.saturating_sub(1)
        } else {
            first_changed_idx
        };

        if move_target_row > prev_viewport_bottom {
            let current_screen_row =
                ((hardware_cursor_row as i64 - prev_viewport_top as i64).max(0) as usize)
                    .min(height.saturating_sub(1) as usize);
            let move_to_bottom = (height as usize).saturating_sub(1 + current_screen_row);
            if move_to_bottom > 0 {
                out.push_str(&format!("\x1b[{}B", move_to_bottom));
            }
            let scroll = move_target_row - prev_viewport_bottom;
            for _ in 0..scroll {
                out.push_str("\r\n");
            }
            prev_viewport_top += scroll;
            viewport_top += scroll;
            hardware_cursor_row = move_target_row;
        }

        let line_diff = compute_line_diff(
            move_target_row,
            hardware_cursor_row,
            prev_viewport_top,
            viewport_top,
        );
        if line_diff > 0 {
            out.push_str(&format!("\x1b[{}B", line_diff));
        } else if line_diff < 0 {
            out.push_str(&format!("\x1b[{}A", -line_diff));
        }

        if append_start {
            out.push_str("\r\n");
        } else {
            out.push('\r');
        }

        let render_end = last_changed_idx.min(new_lines.len().saturating_sub(1));
        for (i, line) in new_lines
            .iter()
            .enumerate()
            .take(render_end + 1)
            .skip(first_changed_idx)
        {
            if i > first_changed_idx {
                out.push_str("\r\n");
            }
            out.push_str("\x1b[2K"); // Clear current line
            out.push_str(line);
        }
        out.push_str(SEGMENT_RESET);

        let mut final_cursor_row = render_end;

        if self.previous_lines.len() > new_lines.len() {
            if render_end < new_lines.len().saturating_sub(1) {
                let move_down = new_lines.len() - 1 - render_end;
                out.push_str(&format!("\x1b[{}B", move_down));
                final_cursor_row = new_lines.len() - 1;
            }
            let extra_lines = self.previous_lines.len() - new_lines.len();
            for _ in 0..extra_lines {
                out.push_str("\r\n\x1b[2K");
            }
            out.push_str(&format!("\x1b[{}A", extra_lines));
        }

        out.push_str("\x1b[?2026l"); // end synchronized output
        term.write(&out);

        self.cursor_row = new_lines.len().saturating_sub(1);
        self.hardware_cursor_row = final_cursor_row;
        self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len());
        self.previous_viewport_top =
            prev_viewport_top.max((final_cursor_row + 1).saturating_sub(height as usize));

        self.position_hardware_cursor(term, cursor_pos, new_lines.len());

        self.previous_lines = new_lines;
        self.previous_width = width;
        self.previous_height = height;
        self.last_cursor = cursor_pos.map(|(r, c, _)| (r, c));
        cursor_pos.map(|(r, c, _)| (r, c))
    }

    /// Position the hardware cursor for IME candidate window or caret tracking.
    fn position_hardware_cursor(
        &mut self,
        term: &mut dyn Terminal,
        cursor: Option<(u16, u16, bool)>,
        total_lines: usize,
    ) {
        if total_lines == 0 {
            term.hide_cursor();
            return;
        }

        let Some((row, col, painted)) = cursor else {
            term.hide_cursor();
            return;
        };

        let target_row = (row as usize).min(total_lines.saturating_sub(1));
        let row_delta = target_row as i64 - self.hardware_cursor_row as i64;
        let mut buffer = String::new();
        if row_delta > 0 {
            buffer.push_str(&format!("\x1b[{}B", row_delta));
        } else if row_delta < 0 {
            buffer.push_str(&format!("\x1b[{}A", -row_delta));
        }
        buffer.push_str(&format!("\x1b[{}G", col + 1));
        if painted {
            buffer.push_str("\x1b[?25l");
        } else {
            buffer.push_str("\x1b[?25h");
        }
        term.write(&buffer);
        self.hardware_cursor_row = target_row;
    }

    /// Park the cursor below the content so the shell prompt lands cleanly.
    ///
    /// The hardware cursor rests wherever the last frame left it - usually
    /// the caret, in the editor line - and the document's final rows (the
    /// footer) sit below that, so the descent comes first and the newline
    /// alone would park the prompt on the footer (gh #33).
    pub fn finish(&mut self, term: &mut dyn Terminal) {
        let mut out = String::new();
        let down = self.cursor_row.saturating_sub(self.hardware_cursor_row);
        if down > 0 {
            out.push_str(&format!("\x1b[{down}B"));
        }
        out.push_str("\r\n\x1b[?7h\x1b[?25h");
        term.write(&out);
    }
}

#[cfg(test)]
mod tests {
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
}
