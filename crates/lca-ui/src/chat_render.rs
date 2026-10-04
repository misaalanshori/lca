//! The document and the viewport (gh #35), split from `chat.rs` for the
//! 1,200-line ceiling: `render`'s transcript/dock split, the fullscreen
//! viewport whose dock stays pinned while the transcript window alone
//! slices by scroll, the virtual scrollbar with pi's thumb geometry, and
//! the jump-to-bottom indicator.
//!
//! Main-screen mode keeps its contract untouched (FR-UI-24): it renders
//! the whole document bottom-anchored and lets the terminal's scrollback
//! own scrolling - only the fullscreen path splits.

use lca_tui::engine::text::{slice_by_column, truncate_to_width, visible_width};

use super::chat::Chat;
use crate::theme::Role;
use crate::widget_lines;

/// One frame's virtual scrollbar (gh #35): where its thumb sits on the
/// transcript window's right margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarGeometry {
    /// The column the scrollbar paints (the window's rightmost).
    pub column: u16,
    /// The track's height: how many top rows of the frame carry it
    /// (the transcript window - the selection needs to know which rows
    /// the adornment is on to keep it out of a copy).
    pub rows: u16,
    /// First thumb row within the track, 0-based.
    pub thumb_top: u16,
    /// Thumb height in rows.
    pub thumb_height: u16,
}

/// The scrollbar for a transcript of `content` rows in a `window`-row
/// window scrolled `from_bottom` rows up, at `width` - pi's own math
/// (`getScrollbarGeometry`): thumb height `round(track^2 / content)`
/// floored at 2, thumb offset `round(scrollTop / maxScrollTop * range)`
/// with `scrollTop` measured from the top of the content. `None` when
/// the transcript fits its window: nothing to position, so nothing to
/// draw (the hard rule's simplest case).
pub fn scrollbar_geometry(
    content: usize,
    window: usize,
    from_bottom: usize,
    width: u16,
) -> Option<ScrollbarGeometry> {
    if window == 0 || content <= window || width == 0 {
        return None;
    }
    let track = window as f64;
    let thumb_height = (2f64.min(track))
        .max((track * track / content as f64).round())
        .min(track) as u16;
    let max_scroll_top = (content - window) as f64;
    let scroll_top = max_scroll_top - from_bottom.min(content - window) as f64;
    let range = track - thumb_height as f64;
    let thumb_top = if max_scroll_top <= 0.0 {
        0
    } else {
        (scroll_top / max_scroll_top * range).round() as u16
    };
    Some(ScrollbarGeometry {
        column: width - 1,
        rows: window as u16,
        thumb_top,
        thumb_height,
    })
}

impl Chat {
    /// The document in two sections (gh #35): what scrolls - the
    /// transcript plus the extension status rows above the dock - and
    /// what does not: the dock (a blank row, the separator, the notice,
    /// the queued messages, the popup, the editor, the footer).
    fn sections(&self, width: u16) -> (Vec<String>, Vec<String>) {
        let mut transcript = self.transcript.render(width, &self.theme);

        // Extension footer regions (FR-UI-1) ride with the fixed part of
        // the frame: they are status, not history.
        if let Some(render) = &self.world.options.render_regions {
            for (_name, tree) in render("footer") {
                for line in widget_lines(&tree.nodes).into_iter().take(3) {
                    transcript.push((self.theme.dim)(&line));
                }
            }
        }

        let mut dock = Vec::new();

        // A separator above the dock: border dashes, with pi's spinner set
        // into them while work runs (R2). The dashes carry the thinking
        // level, the way pi colors its editor border.
        dock.push(String::new());
        let border =
            crate::separator::separator_border(&self.theme, self.thinking_level().as_deref());
        dock.push(self.separator.render(width, &self.theme, &border));

        if let Some(notice) = &self.world.notice {
            // A notice can be multi-line (`/help`, `/hotkeys`, a command's
            // block output); render each line rather than embedding a
            // newline in one line string (which corrupts the screen).
            for (i, line) in notice.split('\n').enumerate() {
                let prefix = if i == 0 { "• " } else { "  " };
                for wrapped in lca_tui::engine::text::wrap_text_with_ansi(
                    &format!("{prefix}{line}"),
                    width as usize,
                ) {
                    dock.push((self.theme.warn)(&wrapped));
                }
            }
        }

        // The pending-messages band (ADR-0038).
        for pending in &self.pending {
            let mark = match pending.mode {
                lca_protocol::SubmitMode::Steer => "steer",
                lca_protocol::SubmitMode::FollowUp => "next",
            };
            dock.push((self.theme.dim)(&format!("  ⏳ [{mark}] {}", pending.text)));
        }
        if !self.pending.is_empty() {
            dock.push((self.theme.dim)(&format!(
                "  {} queued · Alt+E restores them to the editor",
                self.pending.len()
            )));
        }

        // The autocomplete popup, when open.
        dock.extend(self.editor.render_popup(width));

        // The editor: the prompt marker on the first row, and its two
        // columns (`>` + space) as a plain pad on every continuation row,
        // so every row's text starts at the same visual column and the
        // cursor marker's column reads the same on lines 1..n (gh #27a).
        let editor_rows = self.editor.render(width.saturating_sub(2));
        for (index, row) in editor_rows.into_iter().enumerate() {
            dock.push(if index == 0 {
                format!("{} {row}", (self.theme.accent)(">"))
            } else {
                format!("  {row}")
            });
        }

        // The footer.
        dock.extend(self.footer_lines(width));
        (transcript, dock)
    }

    /// Render the whole document (transcript + dock) at a width.
    pub fn render(&self, width: u16) -> Vec<String> {
        let (mut out, dock) = self.sections(width);
        out.extend(dock);
        out
    }

    /// The transcript's line count at this width (gh #35): scroll and
    /// the prompt jump are both measured against the transcript, not the
    /// dock under it.
    pub(super) fn transcript_len(&self, width: u16) -> usize {
        self.transcript.render(width, &self.theme).len()
    }

    /// The transcript window's height for this frame (gh #35): the
    /// viewport's `height - dock_height`, the same arithmetic `viewport`
    /// slices by.
    pub(super) fn window_height(&self, width: u16, height: u16) -> usize {
        let (_, dock) = self.sections(width);
        height.saturating_sub(dock.len() as u16) as usize
    }

    /// The viewport the renderer paints.
    ///
    /// Fullscreen (gh #35) splits the frame: a fixed bottom dock - the
    /// blank row, separator, notice, queue, popup, editor, and footer -
    /// and a transcript window of `height - dock_height` rows that alone
    /// slices by `scroll` (bottom-anchored, clamped so a scroll past the
    /// start shows the start). The window is padded, so the dock pins to
    /// the bottom whatever the transcript's length; the virtual
    /// scrollbar paints onto the window's right margin while the
    /// transcript overflows it, and the jump indicator sits on the
    /// window's last row while scrolled away from the live bottom.
    ///
    /// Main-screen mode is untouched: the whole document, composed
    /// bottom-anchored (FR-UI-24 - its append contract is the other
    /// renderer's point).
    pub fn viewport(&self, width: u16, height: u16, scroll: u16) -> Vec<String> {
        if !self.screen_mode {
            let mut lines = self.render(width);
            self.compose_overlays_bottom_anchored(&mut lines, width, height);
            return lines;
        }
        let (transcript, dock) = self.sections(width);
        let window = height.saturating_sub(dock.len() as u16) as usize;
        let content = transcript.len();
        let from_bottom = (scroll as usize).min(content.saturating_sub(window));
        let end = content - from_bottom;
        let start = end.saturating_sub(window);
        let mut lines: Vec<String> = transcript[start..end].to_vec();
        // Pad the window so the dock sits at the bottom of the frame no
        // matter how short the transcript is.
        lines.resize(window, String::new());

        // The scrollbar (pi's geometry) on the window's right margin: it
        // takes a column from the window's width, thumb over track.
        let geometry = scrollbar_geometry(content, window, from_bottom, width);
        if let Some(geometry) = geometry {
            let content_width = geometry.column as usize;
            for (row, line) in lines.iter_mut().take(window).enumerate() {
                let thumb = (geometry.thumb_top..geometry.thumb_top + geometry.thumb_height)
                    .contains(&(row as u16));
                let glyph = if thumb { "┃" } else { "│" };
                let role = if thumb {
                    Role::ScrollbarThumb
                } else {
                    Role::ScrollbarTrack
                };
                let mut cell = truncate_to_width(line, content_width, "", false);
                let pad = content_width.saturating_sub(visible_width(&cell));
                cell.push_str(&" ".repeat(pad));
                cell.push_str(&self.theme.role(role)(glyph));
                *line = cell;
            }
        }

        // The jump indicator (pi's shape): on the window's last row,
        // centered, its right edge stopping before the scrollbar, and
        // naming the key that returns to the live bottom.
        if from_bottom > 0 && window > 0 {
            let label = format!(
                " ↓ Jump to latest message · {} ",
                lca_tui::engine::keybindings::key_text("tui.altScreen.bottom")
            );
            let avail = (width as usize).saturating_sub(u16::from(geometry.is_some()) as usize);
            let label = truncate_to_width(&label, avail, "", false);
            let label_width = visible_width(&label);
            let column = avail.saturating_sub(label_width) / 2;
            let row = &mut lines[window - 1];
            let left = slice_by_column(row, 0, column, false);
            let right = slice_by_column(row, column + label_width, width as usize, false);
            let styled = (self.theme.bg(Role::SelectedBg))(&(self.theme.role(Role::Text))(&label));
            *row = format!("{left}{styled}{right}");
        }

        lines.extend(dock);
        if self.world.modal_active() || self.picker_open() {
            lines.resize(height as usize, String::new());
        }
        self.compose_overlays(&mut lines, width, height);
        lines
    }

    /// The scrollbar `viewport` paints for this frame (gh #35): the same
    /// inputs and the same pure geometry, so the render side paints it
    /// here and the input side can keep it out of a copy - one rule, two
    /// readers, no drift.
    pub fn scrollbar_for_frame(
        &self,
        width: u16,
        height: u16,
        scroll: u16,
    ) -> Option<ScrollbarGeometry> {
        if !self.screen_mode {
            return None;
        }
        let (transcript, dock) = self.sections(width);
        let window = height.saturating_sub(dock.len() as u16) as usize;
        let content = transcript.len();
        let from_bottom = (scroll as usize).min(content.saturating_sub(window));
        scrollbar_geometry(content, window, from_bottom, width)
    }

    /// One frame's scroll adjustment (gh #35): clamp to what the
    /// transcript can show, and hold the reader's place the way pi's
    /// ScrollView does. Scroll is measured from the live bottom, so
    /// `0` follows every new line (pi's `followingEnd`); away from the
    /// bottom the offset grows with the transcript, which keeps the same
    /// lines on screen while a turn streams (pi's top-anchored hold,
    /// expressed in bottom coordinates). A width change re-wraps the
    /// transcript, so the growth it reports is not new output - the
    /// clamp applies and the offset does not move.
    pub fn clamp_scroll(&mut self, scroll: u16, width: u16, height: u16) -> u16 {
        let (transcript, dock) = self.sections(width);
        let window = height.saturating_sub(dock.len() as u16) as usize;
        let content = transcript.len();
        let max = content.saturating_sub(window) as u16;
        // Growth only counts as new output when the width did not move: a
        // re-wrap changes the line count without adding a word.
        let delta = match self.last_transcript_len {
            Some(previous) if self.last_render_width == width => {
                content.saturating_sub(previous) as u16
            }
            _ => 0,
        };
        self.last_transcript_len = Some(content);
        self.last_render_width = width;
        if scroll == 0 {
            return 0;
        }
        scroll.saturating_add(delta).min(max)
    }

    /// The footer lines, refreshed from the live model label and usage.
    fn footer_lines(&self, width: u16) -> Vec<String> {
        let mut footer = self.footer.clone();
        footer.usage = self.usage.clone();
        // FR-UI-20: the window follows the live model choice (a cell), and
        // the used side is the last call's prompt size.
        footer.context_window = *self
            .world
            .options
            .context_window
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let model = self.model_label();
        footer.model = if model.trim().is_empty() {
            "no model".to_string()
        } else {
            model
        };
        footer.thinking = self.thinking_level();
        if let Some(cue) = &self.turn_status {
            footer.statuses.push(cue.text.clone());
        }
        if let Some(notice) = self
            .world
            .options
            .update_notice
            .as_ref()
            .and_then(|cell| cell.get())
        {
            footer.statuses.push(notice.clone());
        }
        if !self.pending.is_empty() {
            footer
                .statuses
                .push(format!("{} queued", self.pending.len()));
        }
        if let Some(render) = &self.world.options.render_regions {
            for (_name, tree) in render("status-line") {
                for line in widget_lines(&tree.nodes).into_iter().take(1) {
                    footer.statuses.push(line);
                }
            }
        }
        footer.render(width, &self.theme)
    }
}
