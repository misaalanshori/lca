//! Scrolling and the drag's edge auto-scroll: the wheel's step (fixed lines or
//! velocity, gh #82/#206), the edge that arms a tick (R6), and the scrollbar
//! grab that owns a gesture (gh #164).
//!
//! Split from the barrel for gate 11; the code, its comments and its rows are
//! unchanged.

use super::super::selection::SelectionPoint;
use super::AltScreenRenderer;

impl AltScreenRenderer {
    /// Lines one wheel event moves (gh #206): the fixed setting, or
    /// the velocity curve in `auto` mode - with Alt multiplying by
    /// five either way (pi's `ALT_WHEEL_SCROLL_MULTIPLIER`). `now_ms`
    /// rides a parameter so tests fake time.
    pub(crate) fn wheel_step(&mut self, direction: i8, alt: bool, now_ms: u64) -> u16 {
        const ALT_WHEEL_SCROLL_MULTIPLIER: u16 = 5;
        let base = if self.wheel_auto {
            self.wheel_accel.next(direction, now_ms)
        } else {
            u16::from(self.wheel_lines.max(1))
        };
        if alt {
            base.saturating_mul(ALT_WHEEL_SCROLL_MULTIPLIER)
        } else {
            base
        }
    }

    /// Reset the wheel gesture (gh #206): the host calls this when the
    /// wheel setting changes.
    pub fn reset_wheel(&mut self) {
        self.wheel_accel.reset();
        self.wheel_epoch = None;
    }

    /// Take the OSC-8 URL a completed click landed on, if any (R6).
    pub fn take_clicked_link(&mut self) -> Option<String> {
        self.clicked_link.take()
    }

    /// Take the cell a completed single click landed on, if any (gh
    /// #11). The loop hit-tests it against the transcript; a toggle
    /// consumes it, anything else falls through to selection as before.
    pub fn take_clicked_cell(&mut self) -> Option<(u16, u16)> {
        self.clicked_cell.take()
    }

    /// Advance an edge drag by one line (R6): scroll and extend the
    /// selection to the edge. Returns whether it scrolled. The loop calls
    /// this every tick while a drag sits on an edge.
    pub fn tick_auto_scroll(&mut self) -> bool {
        let Some(edge) = self.drag_edge else {
            return false;
        };
        if edge < 0 {
            if self.scroll == 0 {
                return false;
            }
            self.scroll -= 1;
        } else {
            self.scroll = self.scroll.saturating_add(1);
        }
        let row = if edge < 0 {
            0
        } else {
            self.height.saturating_sub(1)
        };
        let line = self.previous.get(row as usize).cloned().unwrap_or_default();
        let col = u16::try_from(line.chars().count().saturating_sub(1)).unwrap_or(u16::MAX);
        self.selection.update(SelectionPoint { row, col }, &line);
        true
    }

    /// Tell the renderer where this frame's virtual scrollbar was
    /// painted (gh #35): `(column, rows)`, or `None` when the frame has
    /// none. The selection reads it to keep the adornment out of both
    /// the copied text and the highlight.
    pub fn set_scrollbar(&mut self, scrollbar: Option<(u16, u16)>) {
        self.selection.set_scrollbar(scrollbar);
    }

    /// The selected text, if any.
    pub fn selected_text(&self) -> String {
        self.selection.active_text(&self.previous)
    }
}
