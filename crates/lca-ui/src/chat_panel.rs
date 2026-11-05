//! The side panel's visibility predicate (gh #373 gap 6), split from
//! `chat_overlays.rs` at the workspace's 1,200-line file ceiling.
//!
//! One predicate answers for the paint and for the keyboard, which is the whole
//! safety property: a panel hidden by its own breakpoint that still consumed
//! keystrokes would make the app feel possessed. Both the key gate and the
//! painter therefore ask this module, never `world.panel_open` directly.

use super::chat::Chat;

impl Chat {
    /// Whether the extension panel is actually on screen this frame (gh #373 gap 6).
    ///
    /// One predicate for the paint and for the key gate, for the same reason the
    /// `has_overlay` guard is one: two answers to one question is how a hidden panel
    /// goes on swallowing keystrokes.
    pub fn panel_shown(&self, width: u16, height: u16) -> bool {
        self.world.panel_open
            && self
                .panel_visible
                .as_ref()
                .is_none_or(|visible| visible(width, height))
    }

    /// Whether the panel is shown, against the size the last frame painted at.
    ///
    /// Input dispatch has no size parameter, and a key must not be judged against a
    /// different size than the frame the reader is looking at. Before the first frame
    /// there is no frame to disagree with, so the predicate is answered at 0x0 - which
    /// a breakpoint predicate reports as hidden, the safe default.
    pub(crate) fn panel_shown_now(&self) -> bool {
        let (width, height) = self
            .last_frame_size
            .get()
            .unwrap_or((self.world.size.0, self.world.size.1));
        self.panel_shown(width, height)
    }

    /// Paint the drawer tab over the margin (gh #207): `◀` opens, `▶`
    /// closes - the visible face of the Alt+X binding. One cell only,
    /// accent on hover, so an idle frame stays quiet.
    pub(super) fn paint_drawer_tab(&self, viewport: &mut [String], width: u16, height: u16) {
        let Some((col, row)) = self.drawer_rect(width, height) else {
            return;
        };
        let Some(line) = viewport.get_mut(row as usize) else {
            return;
        };
        let glyph = if self.panel_shown(width, height) {
            "▶"
        } else {
            "◀"
        };
        let role = if self.drawer_hover {
            crate::theme::Role::Accent
        } else {
            crate::theme::Role::Border
        };
        // gh #238: the glyph splices at the seam, so the row behind
        // cannot tint it and its accent cannot leak right.
        *line =
            crate::render::splice_segment(line, col as usize, 1, &(self.theme.role(role))(glyph));
    }
}
