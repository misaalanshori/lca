//! The editor's jump mode (gh #383 gap 3, R7): find the next occurrence of a
//! character on the caret's line first, then outward.
//!
//! Split out of `editor.rs` at gate 11's ceiling, alongside `editor_kill.rs`; an
//! inherent `impl` may be partitioned, so no caller's path changed.
//!
//! # Why the search order is the fix
//!
//! `jump_to_char` used to build a `Vec` of **every** occurrence in the buffer before
//! answering a question whose answer is nearly always on the caret's own line - a
//! whole-document allocation per keystroke in a long session. pi (`editor.ts:2138`)
//! searches the current line from `col ± 1` and only then walks outward, and the
//! resulting order is identical: forward finds a later position on this line before
//! any other line's, backward an earlier one. So this is the same answer without the
//! allocation.
//!
//! The **cancel** half is in `handle_key`: re-pressing the jump key clears
//! `jump_pending` instead of overwriting its direction, because the old shape left
//! the editor armed and the next typed character became a search target.

use super::*;

impl Editor {
    /// Move the cursor to the next (or previous) occurrence of `target`,
    /// searching the whole buffer from the cursor (pi's `jumpToChar`, R7).
    /// The next/previous `target` on the caret's own line, searching from `from`.
    pub(super) fn find_in_current_line(
        &self,
        target: char,
        from: usize,
        forward: bool,
    ) -> Option<(usize, usize)> {
        let chars: Vec<char> = self.current().chars().collect();
        if forward {
            (from..chars.len())
                .find(|col| chars[*col] == target)
                .map(|col| (self.cursor_line, col))
        } else {
            (0..=from.min(chars.len().saturating_sub(1)))
                .rev()
                .find(|col| chars[*col] == target)
                .map(|col| (self.cursor_line, col))
        }
    }

    /// The first `target` on any other line, in the direction of travel.
    pub(super) fn find_in_other_lines(
        &self,
        target: char,
        forward: bool,
    ) -> Option<(usize, usize)> {
        let indices: Vec<usize> = if forward {
            (self.cursor_line + 1..self.lines.len()).collect()
        } else {
            (0..self.cursor_line).rev().collect()
        };
        for line_index in indices {
            let line = &self.lines[line_index];
            if forward {
                if let Some(col) = line.chars().position(|ch| ch == target) {
                    return Some((line_index, col));
                }
            } else if let Some(col) = line
                .chars()
                .collect::<Vec<_>>()
                .iter()
                .rposition(|c| *c == target)
            {
                return Some((line_index, col));
            }
        }
        None
    }

    pub(super) fn jump_to_char(&mut self, target: char, forward: bool) {
        // Gh #383 gap 3: **the current line first, from `col ± 1`.** This used to
        // collect every occurrence in the buffer into a `Vec` before answering a
        // question whose answer is almost always on the caret's own line - so a
        // `Ctrl+]` in a long session allocated a whole-document index to look one
        // character ahead. pi's `jumpToChar` (`editor.ts:2138`) searches the current
        // line from the caret and only then walks outward, and the ordering is
        // identical: forward finds a later position on this line before any other
        // line's, backward an earlier one.
        let found = if forward {
            self.find_in_current_line(target, self.cursor_col + 1, true)
                .or_else(|| self.find_in_other_lines(target, true))
        } else if self.cursor_col > 0 {
            self.find_in_current_line(target, self.cursor_col - 1, false)
                .or_else(|| self.find_in_other_lines(target, false))
        } else {
            self.find_in_other_lines(target, false)
        };
        if let Some((line, col)) = found {
            self.cursor_line = line;
            self.cursor_col = col;
            self.preferred_col = None;
            self.clear_suggestions();
        }
    }
}
