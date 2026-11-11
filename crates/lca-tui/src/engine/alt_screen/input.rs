//! What an input event *means*: selection and drag, the multi-click run, focus
//! loss, and the viewport keys.
//!
//! Split from the barrel for gate 11; the code, its comments and its rows are
//! unchanged. gh #374 gaps 2-4 live here: focus loss keeps a *finished*
//! selection, a right-button press is a paste request, and a replaced transcript
//! drops the click run as well as the rectangles.

use super::super::core::extract_cursor_position;
use super::super::selection::{Granularity, SelectionPoint};
use super::super::terminal::Terminal;
use super::super::text::visible_width;
use super::{
    AltScreenRenderer, FOCUS_IN, FOCUS_OUT, SgrMouse, expand_for_image_blocks, parse_sgr_mouse,
};

impl AltScreenRenderer {
    /// Render already-composed lines with the selection highlight and the
    /// full-screen diff. Used by the agent interface, which composes its
    /// own viewport (transcript + dock) rather than a `LayoutNode`.
    pub fn render_lines(
        &mut self,
        term: &mut dyn Terminal,
        lines: Vec<String>,
        width: u16,
        height: u16,
    ) -> Option<(u16, u16)> {
        let mut lines = lines;
        lines.resize(height as usize, String::new());
        // Apply the selection highlight.
        self.selection.highlight(&mut lines);
        let (lines, cursor) = extract_cursor_position(&lines);

        let resize = self.first_render || width != self.width || height != self.height;
        // R5: the caret is frame state; see the main-screen renderer's note.
        // The row the caret sits on repaints whenever the caret moved.
        let cursor_moved = cursor.map(|(r, c, _)| (r, c)) != self.last_cursor;
        let mut out = String::from("\x1b[?2026h");
        if resize {
            out.push_str("\x1b[2J\x1b[H");
        }
        // Gh #324 gap 1: an image's reserved rows are one block. A frame that
        // repaints *any* row of a block must repaint the whole block, or the
        // sequence lands on a row whose neighbours were cleared and the image is
        // sliced - and a row inside a reserve must never be treated as text.
        let mut touched: Vec<usize> = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            let caret_row = cursor.is_some_and(|(r, _, _)| r as usize == row);
            if !resize && self.previous.get(row) == Some(line) && !(cursor_moved && caret_row) {
                continue;
            }
            touched.push(row);
        }
        if let Some(expanded) = expand_for_image_blocks(&lines, &touched) {
            touched = expanded;
        }
        // gh #18 (LCA-PROMPT-GH18FIX2): clear every row this frame rewrites
        // before painting any of them. The shared baseline - pi's own diff
        // loop, the `else` branch of `tui-alt-screen.ts`' redraw - emits
        // `CUP` + `2K` + text per changed row, which is correct the moment
        // `?2026` is honored; a terminal or capture path that ignores
        // synchronized output paints it row by row instead and can show a
        // row of the outgoing frame beside a row of the incoming one - the
        // report's stale notice. Same rows, same clears, same text, only
        // the order changes; no row outside `touched` is ever touched.
        //
        // Two rows or more, and not a resize (which cleared the screen
        // above): with a single row there is nothing to interleave with,
        // and the caret-only repaint rows pin that one-row shape.
        if touched.len() >= 2 && !resize {
            for &row in &touched {
                out.push_str(&format!("\x1b[{};1H\x1b[2K", row + 1));
            }
            for &row in &touched {
                out.push_str(&format!("\x1b[{};1H", row + 1));
                out.push_str(&lines[row]);
            }
        } else {
            for &row in &touched {
                out.push_str(&format!("\x1b[{};1H\x1b[2K", row + 1));
                out.push_str(&lines[row]);
            }
        }
        if let Some((row, col, painted)) = cursor {
            // Position at the marker either way (IME candidate windows read
            // the hardware cursor); show it only when the component did not
            // paint its own caret - V2, pi's default `showHardwareCursor =
            // false`, honored per row.
            out.push_str(&format!("\x1b[{};{}H", row + 1, col + 1));
            out.push_str(if painted { "\x1b[?25l" } else { "\x1b[?25h" });
        } else {
            out.push_str("\x1b[?25l");
        }
        out.push_str("\x1b[?2026l");
        term.write(&out);
        self.previous = lines;
        self.width = width;
        self.height = height;
        self.first_render = false;
        self.last_cursor = cursor.map(|(r, c, _)| (r, c));
        cursor.map(|(r, c, _)| (r, c))
    }

    /// Handle raw input: mouse selection, wheel scroll, and viewport keys.
    /// Returns true if consumed.
    pub fn handle_input(&mut self, data: &str) -> bool {
        if let Some(mouse) = parse_sgr_mouse(data) {
            return self.handle_mouse(mouse);
        }
        if data == FOCUS_OUT {
            self.on_focus_out();
            return true;
        }
        if data == FOCUS_IN {
            // Nothing resumes (gh #374 gap 2), and consuming it keeps the
            // sequence from reaching the composer as an unknown CSI.
            return true;
        }
        false
    }

    /// Abort every in-flight gesture on focus loss, **keeping a finished
    /// selection** (gh #374 gap 2).
    ///
    /// The defect: this used to be an unconditional `selection.clear()`, so the
    /// classic flow — drag-select a command, release, `Alt+Tab` to a browser to
    /// paste — wiped the finished highlight the user was about to copy from,
    /// because the terminal sends `FOCUS_OUT` after the release.
    ///
    /// The question is **was a gesture still in progress**, not *is there a
    /// selection*: an interrupted drag has no coherent highlight to keep, while
    /// a finished one is the thing being pasted from. [`Selection::press_active`]
    /// answers exactly that, and it is set by a press and cleared by a release, so
    /// it is genuinely both-valued without focus loss (the #377/#379 trap: a
    /// predicate focus loss is the only writer of can never be false).
    ///
    /// Everything else still aborts, because leaving any of it armed would trade
    /// one stale state for another:
    ///
    /// - `drag_edge` would keep the auto-scroll tick extending a selection nobody
    ///   is dragging;
    /// - `scrollbar_drag` would swallow the next real motion and release;
    /// - `press_cell` would make the next release look like a click on a cell from
    ///   before the focus change;
    /// - the click run (`click_count`/`last_click`) would let the next single click
    ///   continue a word or line grab nobody started.
    ///
    /// pi drops its component mouse capture here too (`clearComponentMouseGesture`);
    /// LCA has no per-component capture, and its analogues are the four above plus
    /// the host's `scroll_drag`, which the host clears on the same event.
    ///
    /// `scrollbar_hover` is dropped for pi's reason (`stopScrollbarHover`): it is a
    /// pointer position, and the pointer is no longer over this window.
    fn on_focus_out(&mut self) {
        let interrupted = self.selection.press_active;
        self.selection.press_active = false;
        self.drag_edge = None;
        self.press_cell = None;
        self.scrollbar_drag = false;
        self.scrollbar_hover = false;
        self.click_count = 0;
        self.last_click = None;
        if interrupted {
            // An interrupted gesture keeps no highlight: it has no coherent range.
            self.selection.clear();
        }
    }

    /// Whether the pointer hovers the scrollbar (gh #164).
    pub fn scrollbar_hovered(&self) -> bool {
        self.scrollbar_hover
    }

    /// Set the scrollbar hover flag (gh #164): the interface owns the
    /// hit-test (it owns the geometry), the renderer only keeps the flag
    /// the frame reads.
    pub fn set_scrollbar_hover(&mut self, hover: bool) {
        self.scrollbar_hover = hover;
    }

    /// Whether a scrollbar drag owns the gesture (gh #164).
    pub fn scrollbar_dragging(&self) -> bool {
        self.scrollbar_drag
    }

    /// Begin a scrollbar drag (gh #164): pi clears the selection on a
    /// scrollbar press, so the drag never extends a stale highlight.
    pub fn begin_scrollbar_drag(&mut self) {
        self.selection.clear();
        self.scrollbar_drag = true;
    }

    /// End a scrollbar drag (gh #164).
    pub fn end_scrollbar_drag(&mut self) {
        self.scrollbar_drag = false;
    }

    /// Drop any selection (gh #167): a press the picker eats must not
    /// leave a stale highlight behind for the release to copy.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// Drop the selection **and the multi-click run** (gh #403 gap 1): the
    /// reset for a transcript the host is about to replace.
    ///
    /// Distinct from [`clear_selection`](Self::clear_selection) on purpose.
    /// A picker press clears the rectangles because a release must not copy
    /// what the picker ate, but the click run is still about the same screen.
    /// A *replacement* is different: the rows address different text, so
    /// leaving `click_count` at 2 or 3 lets the user's next single click
    /// continue the old run and select a word or a whole line they never
    /// asked for. pi draws the same line - its `resetTextSelection()` clears
    /// the selection and drops `lastClick`.
    pub fn reset_text_selection(&mut self) {
        self.selection.clear();
        self.click_count = 0;
        self.last_click = None;
        self.press_cell = None;
        self.drag_edge = None;
    }

    /// The renderer's multi-click run (gh #403 gap 1): what `handle_input`
    /// reads to pick the next click's granularity.
    ///
    /// Exposed for tests because the run loop's reset has to clear *this*
    /// counter, not the selection's mirrored copy - asserting only the copy
    /// would pass while a stale run kept turning the next click into a word
    /// or a line grab.
    pub fn click_run(&self) -> u8 {
        self.click_count
    }

    /// Seed the click run, so a test can build the state a real double-click
    /// leaves behind without synthesizing timestamps.
    pub fn set_click_run(&mut self, count: u8) {
        self.click_count = count;
    }

    /// Handle one mouse event against the current rendered lines.
    pub fn handle_mouse(&mut self, mouse: SgrMouse) -> bool {
        // A scrollbar drag owns motion and release outright (gh #164):
        // the gesture scrolls, it never selects.
        if self.scrollbar_drag {
            return true;
        }
        let motion = mouse.bits & 32 != 0;
        let wheel = mouse.bits & 64 != 0;
        let button = mouse.bits & 3;
        let col = mouse.x.saturating_sub(1);
        let row = mouse.y.saturating_sub(1);
        let point = SelectionPoint { row, col };

        if wheel {
            let direction = if button == 0 { -1 } else { 1 };
            // SGR bit 3 (value 8) is the Alt modifier (pi's note).
            let alt = mouse.bits & 8 != 0;
            let now = self
                .wheel_epoch
                .get_or_insert_with(std::time::Instant::now)
                .elapsed()
                .as_millis()
                .min(u64::MAX as u128) as u64;
            let lines = self.wheel_step(direction, alt, now);
            self.scroll = if direction < 0 {
                self.scroll.saturating_add(lines)
            } else {
                self.scroll.saturating_sub(lines)
            };
            return true;
        }
        if motion {
            if self.selection.press_active {
                // A drag is not a click.
                self.press_cell = None;
                // Edge auto-scroll (R6): a drag at the top/bottom row arms a
                // scroll that `tick_auto_scroll` advances on the loop's tick.
                self.drag_edge = if row == 0 {
                    Some(-1)
                } else if row + 1 >= self.height {
                    Some(1)
                } else {
                    None
                };
                let line = self.previous.get(row as usize).cloned().unwrap_or_default();
                self.selection.update(point, &line);
                return true;
            }
            return false;
        }
        if mouse.press {
            // Gh #374 gap 4: a right-button press is a paste request, never a
            // selection - routing it here (before the selection arm) is the fix for
            // a right-click starting a char-granularity drag.
            if let Some(consumed) = self.right_click_press(button) {
                return consumed;
            }
            // Track double/triple click within 500 ms on the same cell.
            let now = std::time::Instant::now();
            self.click_count = match self.last_click {
                Some((t, x, y))
                    if now.duration_since(t).as_millis() < 500 && x == col && y == row =>
                {
                    (self.click_count % 3) + 1
                }
                _ => 1,
            };
            self.last_click = Some((now, col, row));
            self.press_cell = Some((col, row));
            // The press captures the word/line range (gh #374 gap 3).
            let line = self.previous.get(row as usize).cloned().unwrap_or_default();
            let granularity = Granularity::from_click_count(self.click_count);
            self.selection
                .start(point, granularity, self.click_count, &line);
            true
        } else {
            // A press and release on the same cell without movement is a
            // click: open the OSC-8 link under it, if any (R6).
            if self.press_cell == Some((col, row)) {
                let line = self.previous.get(row as usize).cloned().unwrap_or_default();
                self.clicked_link =
                    crate::engine::text::get_osc8_link_at_column(&line, col as usize);
                // gh #11: a single left-click also completes a click cell
                // for the transcript hit map. Multi-clicks belong to word
                // and line selection, not to toggling.
                if button == 0 && self.click_count <= 1 {
                    self.clicked_cell = Some((col, row));
                }
            }
            self.press_cell = None;
            self.selection.end();
            self.drag_edge = None;
            true
        }
    }

    /// Whether a line is entirely blank.
    pub fn is_blank(line: &str) -> bool {
        visible_width(line) == 0
    }
}
