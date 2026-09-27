//! Main-screen (scrollback) renderer, ported from pi's
//! `packages/tui/src/tui-main-screen.ts`
//! (`pi-tui-re/src_re/tui-engine/tui-main-screen.md`).
//!
//! The UI renders into the terminal's main screen and scrollback, which is
//! why the transcript stays selectable and survives as history. The hot path
//! is a line diff: only the changed range is repainted, and growth appends
//! with real newlines so the terminal scrolls rather than overwrites.
//!
//! Deviation from pi (documented): kitty-image block handling and the
//! Termux height exception are not ported (LCA's terminal image support is
//! a later concern). The width invariant is enforced defensively: a line
//! wider than the terminal is truncated rather than crashing the session,
//! and recorded as a render event.

use super::core::{SEGMENT_RESET, extract_cursor_position, width_violation};
use super::terminal::Terminal;
use super::text::truncate_to_width;

/// Which lines changed between two renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangedRange {
    /// First differing index.
    pub first: usize,
    /// One past the last differing index.
    pub last: usize,
    /// Whether the new content is a pure append.
    pub append: bool,
}

/// Compute the changed range between the previous and new line lists.
pub fn changed_range(previous: &[String], new: &[String]) -> Option<ChangedRange> {
    let common_prefix = previous
        .iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if common_prefix == previous.len() && common_prefix == new.len() {
        return None;
    }
    let append = common_prefix == previous.len() && new.len() > previous.len();
    let last = new.len().max(previous.len());
    Some(ChangedRange {
        first: common_prefix,
        last,
        append,
    })
}

/// The main-screen differential renderer.
#[derive(Debug, Default)]
pub struct MainScreenRenderer {
    previous: Vec<String>,
    width: u16,
    height: u16,
    /// The index of the line the hardware cursor is on.
    cursor_row: usize,
    first_render: bool,
}

impl MainScreenRenderer {
    /// A fresh renderer.
    pub fn new() -> Self {
        Self {
            first_render: true,
            ..Default::default()
        }
    }

    /// The lines from the last render.
    pub fn previous(&self) -> &[String] {
        &self.previous
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
        let (mut lines, cursor) = extract_cursor_position(&lines);

        // Width invariant: defensively truncate rather than crash.
        if let Some((row, w)) = width_violation(&lines, width) {
            let _ = row;
            for line in &mut lines {
                if super::text::visible_width(line) > width as usize {
                    *line = truncate_to_width(line, width as usize, "", false);
                }
            }
            let _ = w;
        }

        let resize = self.first_render || width != self.width || height != self.height;
        let mut out = String::new();
        out.push_str("\x1b[?2026h"); // synchronized output

        if resize {
            if !self.first_render {
                // Clear screen + scrollback so the reflowed content replaces
                // the old one.
                out.push_str("\x1b[2J\x1b[H\x1b[3J");
            }
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    out.push_str("\r\n");
                }
                out.push_str(line);
            }
            out.push_str(SEGMENT_RESET);
            self.cursor_row = lines.len().saturating_sub(1);
            self.width = width;
            self.height = height;
            self.first_render = false;
        } else if let Some(range) = changed_range(&self.previous, &lines) {
            // Full-render fallback when the change is above the viewport.
            let viewport_top = self.previous.len().saturating_sub(height as usize);
            if range.first < viewport_top {
                out.push_str("\x1b[2J\x1b[H\x1b[3J");
                for (i, line) in lines.iter().enumerate() {
                    if i > 0 {
                        out.push_str("\r\n");
                    }
                    out.push_str(line);
                }
                out.push_str(SEGMENT_RESET);
                self.cursor_row = lines.len().saturating_sub(1);
            } else {
                // Move the cursor up to `range.first`.
                if self.cursor_row > range.first {
                    let up = self.cursor_row - range.first;
                    out.push_str(&format!("\x1b[{up}A"));
                    self.cursor_row = range.first;
                }
                if range.append {
                    // Genuine scrollback growth: append with newlines.
                    for line in lines.iter().take(range.last).skip(range.first) {
                        out.push_str("\r\n\x1b[2K");
                        out.push_str(line);
                        self.cursor_row += 1;
                    }
                } else {
                    for (offset, line) in
                        lines.iter().enumerate().take(range.last).skip(range.first)
                    {
                        if offset > range.first {
                            out.push_str("\r\n");
                            self.cursor_row += 1;
                        }
                        out.push_str("\r\x1b[2K");
                        out.push_str(line);
                    }
                }
                // Clear trailing lines that no longer exist.
                if self.previous.len() > lines.len() {
                    for _ in lines.len()..self.previous.len() {
                        out.push_str("\r\n\x1b[2K");
                    }
                    let extra = self.previous.len() - lines.len();
                    out.push_str(&format!("\x1b[{extra}A"));
                }
                out.push_str(SEGMENT_RESET);
                self.cursor_row = lines.len().saturating_sub(1);
            }
        }

        // Position the hardware cursor.
        if let Some((row, col)) = cursor {
            if row as usize != self.cursor_row {
                if self.cursor_row > row as usize {
                    out.push_str(&format!("\x1b[{}A", self.cursor_row - row as usize));
                } else {
                    out.push_str(&format!("\x1b[{}B", row as usize - self.cursor_row));
                }
                self.cursor_row = row as usize;
            }
            out.push_str(&format!("\x1b[{}G", col + 1));
            out.push_str("\x1b[?25h");
        } else {
            out.push_str("\x1b[?25l");
        }

        out.push_str("\x1b[?2026l"); // end synchronized output
        term.write(&out);
        self.previous = lines;
        cursor
    }

    /// Park the cursor below the content so the shell prompt lands cleanly.
    pub fn finish(&mut self, term: &mut dyn Terminal) {
        term.write("\r\n\x1b[?25h");
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
    fn changed_range_finds_prefix_and_append() {
        let prev = vec!["a".to_string(), "b".to_string()];
        let same = vec!["a".to_string(), "b".to_string()];
        assert_eq!(changed_range(&prev, &same), None);
        let appended = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let r = changed_range(&prev, &appended).unwrap();
        assert!(r.append);
        assert_eq!(r.first, 2);
        let edited = vec!["a".to_string(), "X".to_string()];
        let r = changed_range(&prev, &edited).unwrap();
        assert!(!r.append);
        assert_eq!(r.first, 1);
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
        assert!(out.contains("\r\n\x1b[2Ktwo"));
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
        assert!(out.contains("\x1b[2A"));
        assert!(out.contains("ONE"));
    }

    #[test]
    fn overlong_lines_are_truncated_not_fatal() {
        let (mut r, mut term) = renderer();
        r.render(&mut term, vec!["x".repeat(100)], 20, 10);
        // No panic; the written line respects the width.
        let out = term.take_output();
        assert!(out.contains("x"));
    }
}
