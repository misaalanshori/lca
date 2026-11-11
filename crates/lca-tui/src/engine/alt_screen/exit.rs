//! The exit handoff (gh #374 gap 1): what goes back to the main-screen
//! scrollback, and in what order.
//!
//! Split from the barrel for gate 11; the code, its comments and its rows are
//! unchanged. The two private strippers live here because they have exactly one
//! caller - `leave` - and the distinction they draw (LCA's own markers out, every
//! colour kept) is the whole of the handoff's contract.

use super::super::core::{CURSOR_MARKER, SEGMENT_RESET};
use super::super::terminal::Terminal;
use super::{AltScreenRenderer, MOUSE_DISABLE};

fn strip_own_markers(line: &str) -> String {
    if !line.contains('\x1b') {
        return line.to_string();
    }
    let without_marker = line.replace(CURSOR_MARKER, "");
    strip_osc133_prefix(&without_marker)
}

fn strip_osc133_prefix(line: &str) -> String {
    let mut rest = line;
    loop {
        let Some(after) = rest.strip_prefix("\x1b]133;") else {
            return rest.to_string();
        };
        let mut chars = after.chars();
        if !matches!(chars.next(), Some('A' | 'B' | 'C')) {
            return rest.to_string();
        }
        let terminator = chars.as_str();
        let consumed = if terminator.starts_with('\x07') {
            1
        } else if terminator.starts_with("\x1b\\") {
            2
        } else {
            return rest.to_string();
        };
        rest = &terminator[consumed..];
    }
}

impl AltScreenRenderer {
    /// Record the whole transcript this session has (gh #374 gap 1): the
    /// document, not the visible window.
    ///
    /// The host calls this every frame with what it composed before it cut a
    /// viewport out of it. The renderer keeps it only to hand back on exit: an
    /// alt screen that dumps its visible window loses everything the user
    /// scrolled past, which on a 200-line session in a 24-row pane is 176 lines
    /// of history (gh #374 gap 1's loss (a), and the bigger of the two).
    ///
    /// Not the rendered frame: `previous` is the frame, resized to `height`.
    pub fn set_document(&mut self, document: Vec<String>) {
        self.document = document;
    }

    /// The document as recorded for the exit dump (gh #374 gap 1). Exposed so
    /// a row can assert what the handoff *will* write without going through
    /// `leave`'s byte stream.
    pub fn document(&self) -> &[String] {
        &self.document
    }

    /// Leave the alt screen, restoring main-screen scrollback with the final
    /// document so the transcript survives.
    ///
    /// # What leaves, and how (gh #374 gap 1)
    ///
    /// pi's `afterTerminalStop` renders the whole document, strips only its own
    /// in-band markers, and writes it inside one synchronized block. LCA did the
    /// opposite of all three: it wrote `self.previous` (exactly `height` rows, so
    /// a 200-line session kept 24) and ran `strip_terminal_sequences` over every
    /// row, which removed every syntax highlight, diff, markdown and theme colour
    /// along with the markers.
    ///
    /// The dump is therefore the [document](Self::set_document), narrowed by the
    /// private `strip_own_markers` - never by `strip_terminal_sequences`, which is the
    /// right instrument for every other caller (comparisons, hit-tests, plain
    /// rendering) and the wrong one here.
    ///
    /// Order matters. `?2026h` opens a synchronized block so the terminal applies
    /// the whole write at once instead of reflowing per line; autowrap goes off
    /// (`?7l`) so a row wider than the pane cannot wrap into two; each row is
    /// `\r\x1b[2K` + text, which is pi's shape and what makes a 200-row write look
    /// atomic rather than like several redraws.
    ///
    /// `preserve_screen` keeps the bare exit: a handoff mid-session (`$EDITOR`,
    /// an interactive `!` command) must not dump the transcript, because the
    /// session resumes on the alt screen afterwards and the dump would be a
    /// duplicate.
    pub fn leave(&mut self, term: &mut dyn Terminal, preserve_screen: bool) {
        let mut out = String::from(MOUSE_DISABLE);
        if preserve_screen {
            out.push_str("\x1b[?1049l");
        } else {
            let rows = if self.document.is_empty() {
                &self.previous[..]
            } else {
                &self.document[..]
            };
            out.push_str("\x1b[?2026h\x1b[?1049l\x1b[?7l");
            for (i, line) in rows.iter().enumerate() {
                if i > 0 {
                    out.push_str("\r\n");
                }
                out.push_str("\r\x1b[2K");
                let row = strip_own_markers(line);
                // A row that left a colour, background or hyperlink open gets a
                // reset, or it bleeds into the shell prompt below the dump. Only
                // such a row: an unstyled row needs nothing, and this keeps the
                // exit's final bytes exactly as gh #33 pinned them (see below).
                let styled = row.contains('\x1b');
                out.push_str(&row);
                if styled {
                    out.push_str(SEGMENT_RESET);
                }
            }
            // Close the synchronized block, then park the cursor.
            //
            // gh #33: `?1049l` restores the cursor to its pre-alt position
            // (commonly row 1 col 1), so the dump needs a final newline of its own
            // or the returning shell prompt lands on top of it. The park is
            // `\r\n` + autowrap back on + caret shown, and it is the **last thing
            // written** - which is why the block closes before it rather than after
            // (pi ends `...\r\n?25h?2026l`; LCA's released gh #33 contract ends
            // `...\r\n?7h?25h`, and the newer of the two does not get to move the
            // older). Nothing may move the cursor after the park.
            out.push_str("\x1b[?2026l\r\n\x1b[?7h\x1b[?25h");
            term.write(&out);
            self.first_render = true;
            return;
        }
        out.push_str("\x1b[?7h\x1b[?25h");
        term.write(&out);
        self.first_render = true;
    }
}
