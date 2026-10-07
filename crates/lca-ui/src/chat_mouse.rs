//! Extension pointer routing (gh #172): clicks and the wheel over the
//! modal and panel regions, split from `chat.rs` for the workspace's
//! 1,200-line file ceiling.
//!
//! Input (keys and clicks alike) goes to the first extension registered
//! for the region - the interactor's rule, mirrored here. The footer and
//! status-line regions stay display-only: their rows pack native and
//! extension statuses together, so a cell cannot name one owner.

use super::chat::Chat;
use super::ext_widgets::{ButtonHit, widget_render};
use lca_tui::engine::core::{OverlayOptions, SizeValue};

/// What a click did: swallowed by chrome with no owner, or an input for
/// the region's extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtClick {
    /// Chrome with no owner (a dialog, a frame row): eaten, unreported.
    Swallowed,
    /// An input for the region, through the interactor.
    Event(String, lca_protocol::UiInput),
}

impl Chat {
    /// Route one extension input from real pointer input (gh #172):
    /// through the same interactor keys use, applying the effect.
    /// `false` means nothing is registered for the region.
    pub fn send_ext_input(&mut self, region: &str, input: &lca_protocol::UiInput) -> bool {
        let routed = self
            .world
            .options
            .ui_events
            .clone()
            .and_then(|interactor| interactor(region, input));
        match routed {
            Some((_, effect)) => {
                self.apply_effect(effect);
                true
            }
            None => false,
        }
    }

    /// Map a viewport click to an extension input (gh #172): the modal
    /// first, then the panel. `None` is not an extension cell (the
    /// transcript owns it); `Swallowed` is chrome with no owner. The
    /// main screen never captures the mouse, so it never maps.
    pub fn click_extension(
        &mut self,
        col: u16,
        row: u16,
        width: u16,
        height: u16,
    ) -> Option<ExtClick> {
        if !self.screen_mode {
            return None;
        }
        // A host dialog owns the screen outright (gh #124): clicks land
        // on host chrome, never on an extension.
        if self.world.dialog.is_some() {
            return Some(ExtClick::Swallowed);
        }
        if self.world.modal_open
            && let Some(hit) = self.click_modal(col, row, width, height)
        {
            return Some(hit);
        }
        if self.world.panel_open
            && let Some(hit) = self.click_panel(col, row, width)
        {
            return Some(hit);
        }
        None
    }

    /// The extension modal's concatenated body plus its viewport rect:
    /// the same layout rule the composer paints with, so clicks land
    /// where the rows drew.
    fn modal_layout(&self, width: u16) -> Option<(Vec<String>, Vec<ButtonHit>)> {
        let trees = self
            .world
            .options
            .render_regions
            .as_ref()
            .map(|render| render("modal"))
            .unwrap_or_default();
        if trees.is_empty() {
            return None;
        }
        let mut lines = Vec::new();
        let mut hits = Vec::new();
        for (_name, tree) in &trees {
            let ctx = crate::ext_widgets::widget_ctx(
                &self.theme,
                "modal",
                width as usize,
                &self.world.ext_scroll,
            );
            let (mut tree_lines, mut tree_hits) = widget_render(&tree.nodes, &ctx);
            let base = lines.len();
            for hit in tree_hits.iter_mut() {
                hit.line += base;
            }
            lines.append(&mut tree_lines);
            hits.append(&mut tree_hits);
        }
        Some((lines, hits))
    }

    /// The modal's viewport rectangle, recomputed with the composer's
    /// own options and content height.
    fn modal_rect(body_len: usize, width: u16, height: u16) -> lca_tui::engine::core::Rect {
        let options = OverlayOptions {
            width: Some(SizeValue::Percent(80)),
            min_width: Some(24),
            max_height: Some(SizeValue::Abs(height)),
            margin: 2,
            ..Default::default()
        };
        lca_tui::engine::core::resolve_overlay_layout(
            &options,
            width,
            height,
            body_len.saturating_add(2) as u16,
        )
    }

    /// Hit-test the modal box: the title and bottom frames swallow, a
    /// button span names its widget, any other content cell reports
    /// relative coordinates (unwrapped line, content origin).
    fn click_modal(&mut self, col: u16, row: u16, width: u16, height: u16) -> Option<ExtClick> {
        let (body, hits) = self.modal_layout(width)?;
        let rect = Self::modal_rect(body.len(), width, height);
        let (col, row) = (col as usize, row as usize);
        let (rcol, rrow) = (rect.col as usize, rect.row as usize);
        if col < rcol || col >= rcol + rect.width as usize {
            return None;
        }
        if row < rrow || row >= rrow + rect.height as usize {
            return None;
        }
        // Row 0 is the title frame, the last row the bottom frame.
        let content = row - rrow;
        if content == 0 || content >= rect.height as usize - 1 {
            return Some(ExtClick::Swallowed);
        }
        // Content rows: `│ {wrapped} │` - two frame columns, then the
        // wrapped segments of each body line in order.
        let inner = (rect.width as usize).saturating_sub(4);
        let mut wrapped: Vec<usize> = Vec::with_capacity(body.len() + 1);
        let mut rows = 0;
        for line in &body {
            wrapped.push(rows);
            rows += wrap_count(line, inner).max(1);
        }
        wrapped.push(rows);
        let line_index = wrapped
            .iter()
            .rposition(|start| *start < content)
            .unwrap_or(0);
        let rel_col = col.saturating_sub(rcol + 2);
        let rel_row = line_index;
        let input = match hits
            .iter()
            .find(|hit| hit.line == line_index && rel_col >= hit.col_start && rel_col < hit.col_end)
        {
            Some(hit) => lca_protocol::UiInput::ClickWidget { id: hit.id.clone() },
            None => lca_protocol::UiInput::Click {
                col: rel_col as u32,
                row: rel_row as u32,
            },
        };
        self.send_ext_input("modal", &input);
        Some(ExtClick::Event("modal".to_string(), input))
    }

    /// Hit-test the panel: the right columns, one row per line. A
    /// button span names its widget, any other panel cell reports
    /// relative coordinates.
    fn click_panel(&mut self, col: u16, row: u16, width: u16) -> Option<ExtClick> {
        let panel_w = 40usize.min(width as usize / 2);
        let origin = (width as usize).saturating_sub(panel_w);
        let (col, row) = (col as usize, row as usize);
        if col < origin {
            return None;
        }
        let trees = self
            .world
            .options
            .render_regions
            .as_ref()
            .map(|render| render("panel"))
            .unwrap_or_default();
        let mut line = 0;
        for (_name, tree) in &trees {
            let ctx = crate::ext_widgets::widget_ctx(
                &self.theme,
                "panel",
                width as usize,
                &self.world.ext_scroll,
            );
            let (lines, hits) = widget_render(&tree.nodes, &ctx);
            if row >= line && row < line + lines.len() {
                let rel_row = row - line;
                let rel_col = col - origin;
                let input = match hits.iter().find(|hit| {
                    hit.line == rel_row && rel_col >= hit.col_start && rel_col < hit.col_end
                }) {
                    Some(hit) => lca_protocol::UiInput::ClickWidget { id: hit.id.clone() },
                    None => lca_protocol::UiInput::Click {
                        col: rel_col as u32,
                        row: rel_row as u32,
                    },
                };
                self.send_ext_input("panel", &input);
                return Some(ExtClick::Event("panel".to_string(), input));
            }
            line += lines.len();
        }
        // Inside the panel columns but past the last line: chrome.
        Some(ExtClick::Swallowed)
    }

    /// Roll the wheel over an extension region (gh #172): the region's
    /// scroll offset moves and a `Scroll` input rides the interactor, so
    /// scroll containers glide and the extension hears the gesture too.
    /// `true` means the wheel was consumed.
    pub fn wheel_extension(
        &mut self,
        col: u16,
        row: u16,
        width: u16,
        _height: u16,
        delta: i32,
    ) -> bool {
        if !self.screen_mode {
            return false;
        }
        // The dialog's own select scrolls its highlight (gh #124): the
        // wheel is keys the loop never saw.
        if self.world.dialog.is_some() {
            if matches!(
                self.world
                    .dialog
                    .as_ref()
                    .map(|modal| &modal.exchange.dialog),
                Some(lca_protocol::UiDialog::Select { .. })
            ) {
                let key = if delta > 0 { "down" } else { "up" };
                crate::dialogs::handle_dialog(self, "", Some(key));
            }
            return true;
        }
        if self.world.modal_open {
            self.bump_scroll("modal", delta);
            self.send_ext_input("modal", &lca_protocol::UiInput::Scroll { delta });
            return true;
        }
        if self.world.panel_open {
            let panel_w = 40usize.min(width as usize / 2);
            if (col as usize) >= (width as usize).saturating_sub(panel_w) {
                let _ = row;
                self.bump_scroll("panel", delta);
                self.send_ext_input("panel", &lca_protocol::UiInput::Scroll { delta });
                return true;
            }
        }
        false
    }

    /// Move one region's scroll offset, clamped at zero (the viewport
    /// clamps the top: an offset past the content shows the tail).
    fn bump_scroll(&mut self, region: &str, delta: i32) {
        let offset = self.world.ext_scroll.entry(region.to_string()).or_insert(0);
        if delta > 0 {
            *offset = offset.saturating_add(delta as usize);
        } else {
            *offset = offset.saturating_sub((-delta) as usize);
        }
    }
}

/// How many terminal rows one box-content line wraps to (gh #178's
/// wrap, without the link pass: links never change the row count).
fn wrap_count(line: &str, inner: usize) -> usize {
    lca_tui::engine::text::wrap_text_with_ansi(line, inner.max(1)).len()
}
