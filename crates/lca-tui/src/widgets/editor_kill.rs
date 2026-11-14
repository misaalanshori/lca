//! The editor's kill ring and its forward half (gh #383 gaps 2 and 4): the
//! delete-at-boundary rules, `Alt+D`, and `Alt+Y`.
//!
//! Split out of `editor.rs` at gate 11's ceiling (1,370 lines after this cycle's
//! additions), by the seam gh #574 describes: an inherent `impl` may be partitioned
//! across files, so nothing moved out of reach and no caller's path changed.
//!
//! # What this module had to get right, and why each rule is where it is
//!
//! * **The boundary kills** (`Ctrl+K` at EOL, `Ctrl+U` at column 0) were the missing
//!   half. `current()[byte..]` is `""` at end-of-line, so the old code killed the
//!   empty string and did nothing; pi treats the **newline** as the killed text
//!   (`editor.ts:1690`/`:1640`), merging the line and pushing `"\n"` in the kill's
//!   direction.
//! * **`Alt+D`** (`tui.editor.deleteWordForward`) was declared in the binding table
//!   and matched by nothing - a key that did nothing, the `historyPrevious` bug #388
//!   fixed present again.
//! * **`Alt+Y`** (`tui.editor.yankPop`) was the second such binding. It needs the
//!   **anchor** the yank recorded, not a length: a multi-line insert ends somewhere
//!   a `len` cannot express, and cutting from `len` would eat text the yank did not
//!   insert.
//!
//! The accumulation rule is untouched: consecutive kills still fold into one ring
//! entry with the direction deciding `prepend`, because that is what makes
//! `Ctrl+W` ×3 yank back in order.

use super::*;

impl Editor {
    /// Record one kill, accumulating into the previous entry when the
    /// previous key was also a kill (pi's `accumulate`).
    fn kill(&mut self, text: String, prepend: bool) {
        // `kill_run` carries the key handler's captured state; `last_kill`
        // covers a direct API call (a test, or a future non-key caller).
        if (self.kill_run || self.last_kill)
            && let Some(last) = self.kill_ring.last_mut()
        {
            if prepend {
                last.insert_str(0, &text);
            } else {
                last.push_str(&text);
            }
        } else {
            self.kill_ring.push(text);
        }
        self.last_kill = true;
    }

    /// Delete the word before the cursor (gh #383 gap 1).
    ///
    /// The span is exactly what [`Self::cursor_word_left`] would cross - one shared
    /// classifier, so the two cannot disagree. On `foo.bar(baz_qux)` that is `baz_qux`,
    /// then `(`, then `bar`, then `.`, then `foo`; on a path it is one path segment.
    pub fn delete_word_backward(&mut self) {
        self.preferred_col = None;
        self.snapshot();
        let chars: Vec<char> = self.current().chars().collect();
        let col = crate::engine::text::word_start_before(&chars, self.cursor_col);
        let start = self.char_to_byte(col);
        let end = self.char_to_byte(self.cursor_col);
        let killed = self.current()[start..end].to_string();
        self.current_mut().replace_range(start..end, "");
        self.cursor_col = col;
        self.kill(killed, true); // backward kill prepends
        self.refresh_suggestions(false);
    }

    /// Delete from the cursor to the end of the line (kill), or **join the next
    /// line down when the caret is already at the end** (gh #383 gap 2).
    ///
    /// The boundary case was missing: `current()[byte..]` is the empty string at
    /// end-of-line, so `Ctrl+K` there killed `""` and did nothing - where pi treats
    /// the newline as the killed text (`deleteToEndOfLine`, `editor.ts:1690`: at
    /// EOL it merges the next line and pushes `"\n"` forward).
    pub fn delete_to_line_end(&mut self) {
        self.preferred_col = None;
        self.snapshot();
        let byte = self.char_to_byte(self.cursor_col);
        if self.cursor_col >= self.current().chars().count() {
            if self.cursor_line + 1 < self.lines.len() {
                let next = self.lines.remove(self.cursor_line + 1);
                self.current_mut().push_str(&next);
                self.kill("\n".to_string(), false);
            }
            self.refresh_suggestions(false);
            return;
        }
        let killed = self.current()[byte..].to_string();
        self.current_mut().truncate(byte);
        self.kill(killed, false); // forward kill appends
        self.refresh_suggestions(false);
    }

    /// Delete from the line start to the cursor (kill), or **join the previous line
    /// up when the caret is already at column 0** (gh #383 gap 2).
    ///
    /// Same missing boundary as [`Self::delete_to_line_end`]: at column 0 the killed
    /// slice is `""`, so `Ctrl+U` did nothing where pi merges upward and pushes `"\n"`
    /// backward (`deleteToStartOfLine`, `editor.ts:1640`).
    pub fn delete_to_line_start(&mut self) {
        self.preferred_col = None;
        self.snapshot();
        let byte = self.char_to_byte(self.cursor_col);
        if self.cursor_col == 0 {
            if self.cursor_line > 0 {
                let current = self.lines.remove(self.cursor_line);
                self.cursor_line -= 1;
                let previous_len = self.current().chars().count();
                self.current_mut().push_str(&current);
                self.cursor_col = previous_len;
                self.kill("\n".to_string(), true);
            }
            self.refresh_suggestions(false);
            return;
        }
        let killed = self.current()[..byte].to_string();
        let rest = self.current()[byte..].to_string();
        *self.current_mut() = rest;
        self.cursor_col = 0;
        self.kill(killed, true);
        self.refresh_suggestions(false);
    }

    /// Delete the word after the cursor, or join the next line when the caret is
    /// already at the line's end (gh #383 gap 2).
    ///
    /// `tui.editor.deleteWordForward` (`alt+d`, `alt+delete`) has been declared in the
    /// binding table since the registry was written and **matched by nothing** - a
    /// key a user can press today for no effect, the `historyPrevious` bug #388 fixed
    /// present again. The span is exactly what [`Self::cursor_word_right`] crosses, so
    /// motion and deletion cannot disagree; the newline case follows pi's
    /// `deleteWordForward` (`editor.ts:1740`), which merges down and pushes `"\n"`
    /// forward rather than doing nothing.
    pub fn delete_word_forward(&mut self) {
        self.preferred_col = None;
        self.snapshot();
        let chars: Vec<char> = self.current().chars().collect();
        if self.cursor_col >= chars.len() {
            if self.cursor_line + 1 < self.lines.len() {
                let next = self.lines.remove(self.cursor_line + 1);
                self.current_mut().push_str(&next);
                self.kill("\n".to_string(), false);
            }
            self.refresh_suggestions(false);
            return;
        }
        let end = crate::engine::text::word_end_after(&chars, self.cursor_col);
        let start = self.char_to_byte(self.cursor_col);
        let end = self.char_to_byte(end);
        let killed = self.current()[start..end].to_string();
        self.current_mut().replace_range(start..end, "");
        self.kill(killed, false); // forward kill appends
        self.refresh_suggestions(false);
    }

    /// Yank the most recent kill, remembering where it started so
    /// [`Self::yank_pop`] can take it back out.
    pub fn yank(&mut self) {
        self.preferred_col = None;
        let Some(k) = self.kill_ring.last().cloned() else {
            return;
        };
        self.yank_anchor = Some((self.cursor_line, self.cursor_col));
        self.insert_str(&k);
        self.last_yank = true;
    }

    /// Replace the block the last yank inserted with the kill ring's previous entry
    /// (gh #383 gap 2, pi's `yankPop`).
    ///
    /// `tui.editor.yankPop` (`alt+y`) was declared in the binding table and matched by
    /// **nothing** - a key that did nothing, the same shape as `alt+d` and as #388's
    /// `historyPrevious`. pi's rule (`editor.ts:2013`) is narrow on purpose: only
    /// immediately after a yank, and only when the ring holds more than one entry.
    /// The removal uses the **anchor** the yank recorded rather than a length
    /// heuristic, because a multi-line insert ends somewhere `len` cannot express.
    pub fn yank_pop(&mut self) {
        self.preferred_col = None;
        if !self.last_yank || self.kill_ring.len() <= 1 {
            return;
        }
        let Some((anchor_line, anchor_col)) = self.yank_anchor else {
            return;
        };
        self.snapshot();
        // Cut the inserted block from the anchor to the caret. The caret is at the end
        // of the yanked text, so this is one contiguous span - on the same line, or
        // across the lines the yank added.
        if anchor_line < self.cursor_line {
            let head: String = self.lines[anchor_line].chars().take(anchor_col).collect();
            let tail: String = self.lines[self.cursor_line]
                .chars()
                .skip(self.cursor_col)
                .collect();
            self.lines
                .splice(anchor_line..=self.cursor_line, [head + &tail]);
        } else {
            let line = self.lines[anchor_line].clone();
            let head: String = line.chars().take(anchor_col).collect();
            let tail: String = line.chars().skip(self.cursor_col).collect();
            self.lines[anchor_line] = head + &tail;
        }
        self.cursor_line = anchor_line;
        self.cursor_col = anchor_col;
        // Rotate the ring (pi's `killRing.rotate()`: newest to the front), then yank the
        // new newest - which is now the previous entry.
        if let Some(newest) = self.kill_ring.pop() {
            self.kill_ring.insert(0, newest);
        }
        let Some(next) = self.kill_ring.last().cloned() else {
            return;
        };
        self.yank_anchor = Some((self.cursor_line, self.cursor_col));
        self.insert_str(&next);
        self.last_yank = true;
    }
}
