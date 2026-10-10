//! Extension pointer routing (gh #172): clicks and the wheel over the
//! modal and panel regions, split from `chat.rs` for the workspace's
//! 1,200-line file ceiling.
//!
//! Input (keys and clicks alike) goes to the first extension registered
//! for the region - the interactor's rule, mirrored here. The footer and
//! status-line regions stay display-only: their rows pack native and
//! extension statuses together, so a cell cannot name one owner.

use super::chat::Chat;
use super::chat_pickers::TRUST_OPTIONS;
use super::ext_widgets::{ButtonHit, widget_render};
use super::render::{TOOLTIP_MAX_WIDTH, paint_tooltip, tooltip_lines, tooltip_place};
use crate::transcript::EntryHit;
use lca_tui::engine::core::{OverlayOptions, Rect, SizeValue, resolve_overlay_layout};
use lca_tui::engine::keybindings::key_text;
use lca_tui::engine::text::visible_width;
use std::time::Instant;

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
    pub(crate) fn click_panel(&mut self, col: u16, row: u16, width: u16) -> Option<ExtClick> {
        let panel_w = 40usize.min(width as usize / 2);
        let origin = (width as usize).saturating_sub(panel_w);
        let (col, row) = (col as usize, row as usize);
        // gh #237: the `│ ` border owns the panel's first two cells;
        // widget columns start past it, where the painter put them.
        if col < origin + 2 {
            return None;
        }
        let col = col - origin - 2;
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
                let rel_col = col;
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

// Gh #164: the scrollbar half of pointer routing. The renderer owns
// text selection, so a drag must never reach it; the geometry lives
// here with the frame, so the hit-test, the drag math, and the paint
// share one rule (pi splits the same work between `tui-alt-screen.ts`
// and `layout.ts` because pi's renderer owns its geometry - LCA's
// does not, hence the interface side).

/// What a press on the scrollbar column means (gh #164): the stepper
/// cells keep their prompt-jump clicks (gh #173), a track row starts a
/// drag, anything else misses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarHit {
    /// A track row: starts a scrollbar drag.
    Track,
    /// A stepper cell (▲/▼): keeps its click.
    Stepper,
    /// Not on the scrollbar.
    Miss,
}

impl Chat {
    /// Hit-test the scrollbar column for this frame (gh #164): pure
    /// over the same geometry the frame paints. Alt-screen only, like
    /// every other click path.
    pub fn scrollbar_hit(
        &self,
        col: u16,
        row: u16,
        width: u16,
        height: u16,
        scroll: u16,
    ) -> ScrollbarHit {
        if !self.screen_mode {
            return ScrollbarHit::Miss;
        }
        let Some(geometry) = self.scrollbar_for_frame(width, height, scroll) else {
            return ScrollbarHit::Miss;
        };
        if col != geometry.column || row >= geometry.rows {
            return ScrollbarHit::Miss;
        }
        if row == 0 || row + 1 >= geometry.rows {
            return ScrollbarHit::Stepper;
        }
        ScrollbarHit::Track
    }

    /// Refresh the scrollbar hover flag from a pointer cell (gh #164):
    /// pi re-evaluates hover on every mouse event, so a drag release
    /// off the track does not leave a stale solid thumb behind.
    pub fn refresh_scrollbar_hover(
        &mut self,
        col: u16,
        row: u16,
        width: u16,
        height: u16,
        scroll: u16,
    ) {
        self.scrollbar_hover =
            self.scrollbar_hit(col, row, width, height, scroll) == ScrollbarHit::Track;
    }

    /// Map a drag pointer row to a scroll offset (gh #164, pi's
    /// `scrollScrollbarToPointer`): the thumb slides so the grab point
    /// follows the pointer, clamped to the track, with scroll measured
    /// from the live bottom. `None` when no scrollbar paints.
    pub fn scrollbar_drag_scroll(
        &self,
        width: u16,
        height: u16,
        scroll: u16,
        pointer_row: u16,
        grab: u16,
    ) -> Option<u16> {
        if !self.screen_mode {
            return None;
        }
        let geometry = self.scrollbar_for_frame(width, height, scroll)?;
        let (content, window) = self.scroll_extent(width, height);
        if content <= window || window == 0 {
            return None;
        }
        let max = content.saturating_sub(window);
        let travel = geometry.rows.saturating_sub(geometry.thumb_height) as f64;
        let offset = f64::from(pointer_row.saturating_sub(grab)).clamp(0.0, travel);
        let top = if travel <= 0.0 {
            0.0
        } else {
            (offset / travel * max as f64).round()
        };
        u16::try_from(max.saturating_sub(top as usize)).ok()
    }
}

impl Chat {
    /// Whether a drawer tab shows this frame (gh #207): an open panel
    /// always carries its ▶, a closed one shows ◀ only when an
    /// extension actually registered for the panel region.
    pub fn has_panel(&self) -> bool {
        if self.world.panel_open {
            return true;
        }
        self.world
            .options
            .render_regions
            .as_ref()
            .is_some_and(|render| !render("panel").is_empty())
    }

    /// The drawer tab's cell (gh #207, bottom-pinned by gh #237):
    /// `(col, row)` on the transcript window's bottom row - one cell
    /// left of the scrollbar when closed, the panel edge when open.
    /// `None` with no panel region (zero clutter) or off fullscreen.
    pub fn drawer_rect(&self, width: u16, height: u16) -> Option<(u16, u16)> {
        if !self.screen_mode || !self.has_panel() || width == 0 || height == 0 {
            return None;
        }
        let row = (self.window_height(width, height) as u16).saturating_sub(1);
        let col = if self.world.panel_open {
            width.saturating_sub(super::render::panel_width(width) as u16)
        } else {
            width.saturating_sub(2)
        };
        Some((col, row))
    }

    /// Refresh the drawer hover flag (gh #207): the frame paints the
    /// tab in accent while set.
    pub fn refresh_drawer_hover(&mut self, col: u16, row: u16, width: u16, height: u16) {
        self.drawer_hover = self.drawer_rect(width, height) == Some((col, row));
    }

    /// Click the drawer tab (gh #207): the same flip as Alt+X.
    /// Returns false off the tab.
    pub fn click_drawer(&mut self, col: u16, row: u16, width: u16, height: u16) -> bool {
        if self.drawer_rect(width, height) != Some((col, row)) {
            return false;
        }
        self.world.panel_open = !self.world.panel_open;
        true
    }

    /// Click-to-focus the composer (gh #165, pi's click-to-focus
    /// routing): a click inside the editor rows dismisses any open
    /// picker and places the caret under the pointer for immediate
    /// typing. Returns false outside the editor rows.
    pub fn place_editor_caret(&mut self, col: u16, row: u16, width: u16, height: u16) -> bool {
        let Some((top, len)) = self.editor_rect(width, height) else {
            return false;
        };
        if row < top || row >= top.saturating_add(len) {
            return false;
        }
        if self.picker_open() {
            // Pi's click-to-focus: the click belongs to the composer,
            // so the picker takes the Escape path (a theme preview
            // restores, like a keyboard cancel).
            let _ = self.handle_picker_key("", Some("escape"));
        }
        // The dock paints the `> `/continuation marker in two columns
        // before the editor text (gh #27a); the editor maps the rest.
        let local_col = col.saturating_sub(2) as usize;
        let local_row = row.saturating_sub(top) as usize;
        self.editor.handle_click(local_col, local_row);
        true
    }
}

// Gh #167: the picker half of pointer routing. Every picker draws
// through `picker_overlay` (bottom-anchored above the composer), so one
// layout rule maps them all: the box rect from the picker's body length,
// then body rows to items. Complex bodies (grants groups, settings
// sections) come from the picker's own `mouse_rows` walk, which mirrors
// its compose arm.

/// What a pointer cell means against the open picker (gh #167).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerHit {
    /// An item row, in the picker's item space: highlight on hover,
    /// confirm on click.
    Item(usize),
    /// Box chrome (frame, headers, hints, dividers, position rows):
    /// eats presses, selects nothing.
    Chrome,
    /// Outside the box: dismiss without confirming.
    Backdrop,
}

impl Chat {
    /// The open picker's painted box and per-body-row item map (gh
    /// #167): `Some(index)` rows highlight and confirm, `None` rows are
    /// chrome. `None` when no picker is open. Body rows are assumed
    /// unwrapped - a picker row past ~90 cells wraps, and its
    /// continuation hits as the next row.
    ///
    /// The map is sliced to the painted rolling window (gh #226): the
    /// painter stored `(start, end)` on the last frame, so the hit test
    /// reads back what is drawn. The rect re-resolves over the windowed
    /// content through the same options the painter uses.
    pub(crate) fn picker_layout(
        &self,
        width: u16,
        height: u16,
    ) -> Option<(Rect, Vec<Option<usize>>)> {
        if !self.screen_mode {
            return None;
        }
        let map: Vec<Option<usize>> = if let Some(picker) = &self.tree_picker {
            std::iter::repeat_n(None, 2)
                .chain((0..picker.visible_len()).map(Some))
                .collect()
        } else if let Some(picker) = &self.resume_picker {
            let mut map = vec![None, None];
            if picker.matches.is_empty() {
                map.push(None);
            } else {
                map.extend((0..picker.matches.len()).map(Some));
            }
            map
        } else if let Some(picker) = &self.fork_picker {
            std::iter::repeat_n(None, 2)
                .chain((0..picker.messages.len()).flat_map(|index| [Some(index); 3]))
                .collect()
        } else if let Some(picker) = &self.scoped_models_picker {
            std::iter::repeat_n(None, 2)
                .chain((0..picker.rows.len()).map(Some))
                .collect()
        } else if let Some(picker) = &self.grants_picker {
            picker.mouse_rows()
        } else if let Some(picker) = &self.theme_picker {
            let _ = picker;
            std::iter::repeat_n(None, 2)
                .chain((0..self.theme_names.len()).map(Some))
                .collect()
        } else if let Some(picker) = &self.trust_picker {
            let _ = picker;
            std::iter::repeat_n(None, 4)
                .chain((0..TRUST_OPTIONS.len()).map(Some))
                .collect()
        } else if let Some(picker) = &self.thinking_picker {
            std::iter::repeat_n(None, 2)
                .chain((0..picker.offered.len() + 1).map(Some))
                .collect()
        } else if let Some(picker) = &self.settings_picker {
            picker.mouse_rows()
        } else if let Some(picker) = &self.model_picker {
            let mut map = vec![None, None];
            if picker.matches.is_empty() {
                map.push(None);
            } else {
                map.extend((0..picker.matches.len()).map(Some));
                // Pi's `(1/126)` position row rides under the items.
                map.push(None);
            }
            map
        } else {
            return None;
        };
        // The same options `overlay_box_picker` paints with, so the
        // hit box and the painted box are one rule.
        let above = self.composer_height(width);
        let options = super::render::picker_overlay_options(height, above);
        let (start, end, content) = self.picker_window.get();
        let start = start.min(map.len());
        let end = end.clamp(start, map.len());
        // The painter's own content height when this frame drew the
        // window, else the full list (the painter's arithmetic for an
        // unwindowed box).
        let content = if content == 0 {
            u16::try_from(map.len().saturating_add(4)).unwrap_or(u16::MAX)
        } else {
            content
        };
        let rect = resolve_overlay_layout(&options, width, height, content);
        Some((rect, map[start..end].to_vec()))
    }

    /// Map a viewport cell to the open picker (gh #167): items
    /// highlight and confirm, chrome eats the gesture, the backdrop
    /// dismisses. `None` when no picker is open.
    pub fn picker_hit(&self, col: u16, row: u16, width: u16, height: u16) -> Option<PickerHit> {
        let (rect, map) = self.picker_layout(width, height)?;
        // The painted rows: title + body + bottom frame, clipped to the
        // viewport exactly like `overlay_box_placed` clips them.
        let painted = (map.len().saturating_add(4)).min(height as usize);
        let bottom = (rect.row as usize).saturating_add(painted);
        if col < rect.col
            || col >= rect.col.saturating_add(rect.width)
            || row < rect.row
            || (row as usize) >= bottom
        {
            return Some(PickerHit::Backdrop);
        }
        if row == rect.row {
            return Some(PickerHit::Chrome);
        }
        match map.get((row - rect.row - 1) as usize) {
            Some(Some(item)) => Some(PickerHit::Item(*item)),
            _ => Some(PickerHit::Chrome),
        }
    }

    /// Highlight one item row (gh #167, pi's press-to-highlight): the
    /// theme picker live-previews like its keyboard path does.
    pub fn hover_picker_item(&mut self, item: usize) {
        if let Some(picker) = self.tree_picker.as_mut() {
            picker.selected = item.min(picker.visible_len().saturating_sub(1));
        } else if let Some(picker) = self.resume_picker.as_mut() {
            picker.selected = item.min(picker.matches.len().saturating_sub(1));
        } else if let Some(picker) = self.fork_picker.as_mut() {
            picker.selected = item.min(picker.messages.len().saturating_sub(1));
        } else if let Some(picker) = self.scoped_models_picker.as_mut() {
            picker.selected = item.min(picker.rows.len().saturating_sub(1));
        } else if let Some(picker) = self.grants_picker.as_mut() {
            picker.selected = item.min(picker.entries.len().saturating_sub(1));
        } else if self.theme_picker.is_some() {
            let selected = item.min(self.theme_names.len().saturating_sub(1));
            if let Some(picker) = self.theme_picker.as_mut() {
                picker.selected = selected;
            }
            self.preview_theme(selected);
        } else if let Some(picker) = self.trust_picker.as_mut() {
            picker.selected = item.min(TRUST_OPTIONS.len().saturating_sub(1));
        } else if let Some(picker) = self.thinking_picker.as_mut() {
            picker.selected = item.min(picker.offered.len());
        } else if let Some(picker) = self.settings_picker.as_mut() {
            picker.selected = item.min(picker.rows.len().saturating_sub(1));
        } else if let Some(picker) = self.model_picker.as_mut() {
            picker.selected = item.min(picker.matches.len().saturating_sub(1));
        }
    }

    /// Roll the wheel over the open picker (gh #226, pi's
    /// select-list wheel): one row per tick, the rolling window
    /// following `selected` on the next frame. Silent when no picker
    /// is open.
    pub fn wheel_picker(&mut self, delta: i8) {
        fn step(selected: usize, max: usize, delta: i8) -> usize {
            (selected as i64 + delta as i64).clamp(0, max as i64) as usize
        }
        if let Some(picker) = self.tree_picker.as_mut() {
            picker.selected = step(
                picker.selected,
                picker.visible_len().saturating_sub(1),
                delta,
            );
        } else if let Some(picker) = self.resume_picker.as_mut() {
            picker.selected = step(
                picker.selected,
                picker.matches.len().saturating_sub(1),
                delta,
            );
        } else if let Some(picker) = self.fork_picker.as_mut() {
            picker.selected = step(
                picker.selected,
                picker.messages.len().saturating_sub(1),
                delta,
            );
        } else if let Some(picker) = self.scoped_models_picker.as_mut() {
            picker.selected = step(picker.selected, picker.rows.len().saturating_sub(1), delta);
        } else if let Some(picker) = self.grants_picker.as_mut() {
            picker.selected = step(
                picker.selected,
                picker.entries.len().saturating_sub(1),
                delta,
            );
        } else if self.theme_picker.is_some() {
            let selected = step(
                self.theme_picker
                    .as_ref()
                    .map(|picker| picker.selected)
                    .unwrap_or(0),
                self.theme_names.len().saturating_sub(1),
                delta,
            );
            if let Some(picker) = self.theme_picker.as_mut() {
                picker.selected = selected;
            }
            self.preview_theme(selected);
        } else if let Some(picker) = self.trust_picker.as_mut() {
            picker.selected = step(
                picker.selected,
                TRUST_OPTIONS.len().saturating_sub(1),
                delta,
            );
        } else if let Some(picker) = self.thinking_picker.as_mut() {
            picker.selected = step(picker.selected, picker.offered.len(), delta);
        } else if let Some(picker) = self.settings_picker.as_mut() {
            picker.selected = step(picker.selected, picker.rows.len().saturating_sub(1), delta);
        } else if let Some(picker) = self.model_picker.as_mut() {
            picker.selected = step(
                picker.selected,
                picker.matches.len().saturating_sub(1),
                delta,
            );
        }
    }

    /// Confirm the highlighted item (gh #167): every picker's Enter
    /// path, except the checklist toggles its row (space) instead of
    /// saving. Silent when no picker is open.
    pub fn confirm_picker_item(&mut self) {
        if !self.picker_open() {
            return;
        }
        if self.scoped_models_picker.is_some() {
            let _ = self.handle_scoped_models_key("", Some("space"));
        } else {
            let _ = self.handle_picker_key("", Some("enter"));
        }
    }
}

// Gh #210: hover tooltips. A stationary pointer names the interactable
// under it; any motion, press, or key dismisses. The frame paints the
// stored block last, so tooltips ride over everything.

/// A tooltip waits this long on a stationary hover before showing.
const TOOLTIP_DEBOUNCE_MS: u128 = 250;

/// A shown tooltip: placed lines over the viewport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tooltip {
    /// The wrapped lines, for the painter.
    pub lines: Vec<String>,
    /// The viewport column.
    pub col: u16,
    /// The viewport row.
    pub row: u16,
}

impl Chat {
    /// The tooltip text for a viewport cell (gh #210): drawer, the
    /// scrollbar steppers, the jump indicator, tool headers, thinking
    /// runs. `None` for plain rows - and always off fullscreen, where
    /// no hover exists to name anything.
    pub fn tooltip_at(
        &self,
        col: u16,
        row: u16,
        width: u16,
        height: u16,
        scroll: u16,
    ) -> Option<String> {
        if !self.screen_mode {
            return None;
        }
        if self.drawer_rect(width, height) == Some((col, row)) {
            return Some("Toggle extension panel\nAlt+X".to_string());
        }
        let geometry = self.scrollbar_for_frame(width, height, scroll);
        if let Some(geometry) = &geometry
            && col == geometry.column
            && row < geometry.rows
        {
            if row == 0 {
                return Some(format!(
                    "Previous prompt · {}",
                    key_text("app.prompt.previous")
                ));
            }
            if row + 1 >= geometry.rows {
                return Some(format!("Next prompt · {}", key_text("app.prompt.next")));
            }
            return None;
        }
        let (content, window) = self.scroll_extent(width, height);
        if window == 0 || row as usize >= window {
            return None;
        }
        let from_bottom = (scroll as usize).min(content.saturating_sub(window));
        // The jump indicator stops before the scrollbar (gh #173), so a
        // tooltip there would lie over the stepper: skip its column.
        if from_bottom > 0
            && row as usize + 1 == window
            && geometry
                .as_ref()
                .is_none_or(|geometry| col != geometry.column)
        {
            return Some(format!(
                "Back to latest · {}",
                key_text("tui.altScreen.bottom")
            ));
        }
        // The same window math `click_at` hit-tests by (gh #35).
        let end = content.saturating_sub(from_bottom);
        let start = end.saturating_sub(window);
        match self
            .transcript
            .entry_at_row(width, &self.theme, start + row as usize)
        {
            Some(EntryHit::Thinking(_)) => Some(format!(
                "Cycle thinking · {}",
                key_text("app.thinking.toggle")
            )),
            Some(EntryHit::ToolHeader(index)) => match self.transcript.tool_card_expanded(index) {
                Some(true) => Some("Collapse output".to_string()),
                Some(false) => Some("Expand output".to_string()),
                None => None,
            },
            None => None,
        }
    }

    /// Record a hover cell (gh #210): a cell with tooltip text holds a
    /// showing tooltip, anything else dismisses and rearms the clock.
    pub fn note_hover(&mut self, col: u16, row: u16, width: u16, height: u16, scroll: u16) {
        let now = self.tooltip_at(col, row, width, height, scroll);
        let shown = self.tooltip.as_ref().map(|tip| tip.lines.join("\n"));
        if now.is_some() && shown.as_deref() == now.as_deref() {
            return;
        }
        self.tooltip = None;
        self.hover_at = now.map(|_| (col, row, Instant::now()));
    }

    /// Dismiss any tooltip and rearm the hover clock (gh #210): keys,
    /// presses, wheels, and resizes all clear the air.
    pub fn dismiss_tooltip(&mut self) {
        self.tooltip = None;
        self.hover_at = None;
    }

    /// Show a due tooltip (gh #210): the pointer sat 250 ms over a
    /// tipped cell. Returns whether a tooltip newly showed (the loop
    /// repaints on `true`). `now` rides a parameter so tests fake time.
    pub fn poll_tooltip(&mut self, width: u16, height: u16, scroll: u16, now: Instant) -> bool {
        if self.tooltip.is_some() {
            return false;
        }
        let Some((col, row, at)) = self.hover_at else {
            return false;
        };
        if now.duration_since(at).as_millis() < TOOLTIP_DEBOUNCE_MS {
            return false;
        }
        let Some(text) = self.tooltip_at(col, row, width, height, scroll) else {
            return false;
        };
        let lines = tooltip_lines(&text, TOOLTIP_MAX_WIDTH);
        let wide = lines
            .iter()
            .map(|line| visible_width(line))
            .max()
            .unwrap_or(0);
        let (x, y) = tooltip_place(width, height, col, row, wide, lines.len());
        self.tooltip = Some(Tooltip {
            lines,
            col: x,
            row: y,
        });
        true
    }

    /// Paint a due tooltip over the frame (gh #210): last, so it rides
    /// over transcript, dock, and overlays alike.
    pub(super) fn paint_tooltip(&self, lines: &mut [String]) {
        if let Some(tip) = &self.tooltip {
            paint_tooltip(lines, tip.col, tip.row, &tip.lines, &self.theme);
        }
    }
}
