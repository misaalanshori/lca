//! A reusable tabs widget (gh #208, OpenCode-inspired): a horizontal bar
//! or a vertical rail of tabs with keyboard and keystone-mouse input.
//!
//! The widget renders plain marker text (`*` active, `●` dirty, `◌`
//! generating, `(n)` badge, `×` closable, `…` truncation) and reports
//! spans for the host to style - like every engine widget, it never
//! touches a theme (the engine boundary rule). The generating marker is
//! static: the host re-renders on its own tick for a pulse. The add
//! button reads `[+]` in both orientations (the V sketch's "New Panel"
//! label stays a host concern, so the primitive ships no product text).

use crate::engine::core::{MouseButton, MouseEvent};
use crate::engine::text::{truncate_to_width, visible_width};

#[cfg(test)]
#[path = "tabs_tests.rs"]
mod tests;

/// The close glyph and the space that precedes it (gh #326). A named constant
/// because two places need it and they must agree: [`TabsWidget::flags`] paints
/// it, and [`TabsWidget::vertical_cell`] has to know its width *before* it
/// decides how much title fits. Written out twice, the reserve and the paint can
/// disagree - which is the drift this issue is made of.
const CLOSE_TEXT: &str = " ×";

/// The tab layout direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabOrientation {
    /// A browser-style single-row bar.
    Horizontal,
    /// A rail-style side column.
    Vertical,
}

/// One tab.
#[derive(Debug, Clone)]
pub struct TabItem {
    /// The stable id the host maps back to its session or view.
    pub id: String,
    /// The display title (truncated with `…` past the width).
    pub title: String,
    /// An unread count or status indicator, painted as `(n)`.
    pub badge: Option<String>,
    /// A background turn is in progress (a static `◌` marker).
    pub is_generating: bool,
    /// Unsaved changes (a `●` marker).
    pub is_dirty: bool,
    /// Whether the tab carries a close button.
    pub closable: bool,
}

/// What a tab gesture asks the host to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabEvent {
    /// The tab is now active (keyboard confirm or click release).
    /// A press alone only previews (it moves `active_index` without an
    /// event), so a drag-off still previews but never commits.
    Select(usize),
    /// Close the tab (Ctrl+W or the `×` cell).
    Close(usize),
    /// The `[+]` cell: open a tab.
    Add,
}

/// What cell a local pointer cell hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabHit {
    /// A tab's body.
    Tab(usize),
    /// A tab's `×` cell.
    Close(usize),
    /// The `[+]` cell.
    Add,
}

/// The tabs widget: a building block for session tabs and side-panel
/// sections, not a wired product surface (no host owns one yet).
pub struct TabsWidget {
    /// Bar or rail.
    pub orientation: TabOrientation,
    /// The tabs, in order.
    pub items: Vec<TabItem>,
    /// The active tab (clamped at use, so hosts may hold a stale index
    /// across list edits without a panic).
    pub active_index: usize,
    /// Whether the `[+]` cell renders.
    pub show_add_button: bool,
    /// The hovered tab for the host's background lift (`None` over `×`,
    /// `[+]`, or outside).
    pub hovered: Option<usize>,
}

/// The cell the `×` occupies in a **painted** rail row (gh #326).
///
/// Measured by walking the row to the glyph rather than by a rule like "the last
/// column", because that rule is what made the region a phantom: the last column
/// was a title character or the ellipsis whenever the glyph had been truncated
/// away. `None` when the row draws no glyph, so a host can never dispatch a close
/// for a button the user cannot see.
fn rendered_close_region(row: &str, closable: bool) -> Option<(usize, usize)> {
    if !closable {
        return None;
    }
    // The column is the visible width of the prefix, **not** a per-character
    // sum: `truncate_to_width` wraps the ellipsis in `\x1b[0m` resets, and those
    // are zero cells on screen but four bytes in the string. A per-character walk
    // counts each one and lands the region three cells past the glyph - the same
    // drift, reintroduced in the fix for it.
    let bytes_before = row.find('×')?;
    let col = visible_width(&row[..bytes_before]);
    Some((col, col + 1))
}

/// One laid-out tab cell: where it paints and where its `×` sits.
///
/// `text` is the **painted** row (gh #326). It lives here so `render`, the hit
/// test and the spans all read one string: the vertical rail used to rebuild its
/// row from `self.items` inside `render` while the hit test measured a separately
/// truncated copy, and the two drifting apart is exactly how a click on the `×`
/// stopped closing the tab.
struct TabCell {
    row: usize,
    start: usize,
    end: usize,
    text: String,
    close: Option<(usize, usize)>,
}

/// The laid-out widget: every reader (render, hit-test, spans) walks
/// the same layout - including the truncated titles - so they can
/// never disagree.
struct TabLayout {
    tabs: Vec<TabCell>,
    add: Option<(usize, usize, usize)>,
    rows: usize,
}

impl TabsWidget {
    /// The active index clamped to the list (gh #208): an empty list
    /// has no active tab.
    fn active(&self) -> Option<usize> {
        if self.items.is_empty() {
            None
        } else {
            Some(self.active_index.min(self.items.len() - 1))
        }
    }

    /// The tab's flags in paint order: `*` active, `●` dirty, `◌`
    /// generating, ` (n)` badge, ` ×` closable.
    fn flags(item: &TabItem, active: bool) -> String {
        let mut flags = String::new();
        if active {
            flags.push('*');
        }
        if item.is_dirty {
            flags.push('●');
        }
        if item.is_generating {
            flags.push('◌');
        }
        if let Some(badge) = &item.badge {
            flags.push_str(&format!(" ({badge})"));
        }
        if item.closable {
            flags.push_str(CLOSE_TEXT);
        }
        flags
    }

    /// One vertical rail row and its `×` region (gh #326).
    ///
    /// **Only the title is flexible.** The old code formatted the whole row -
    /// marker, index, title, flags and `×` - and truncated *that*, so past a
    /// certain width the `×` was cut away and the host still reported a close
    /// region on the last remaining column. The user saw no close button, and a
    /// click on a title character closed the tab.
    ///
    /// The priority order, outermost-in, is what this encodes:
    ///
    /// 1. **the `×`** - a close affordance, so it is reserved first and never
    ///    truncated while there is a column to put it in;
    /// 2. **the marker + `[n] `** - the index is how a digit-jump names the tab;
    /// 3. **the flags** - state the title cannot carry, dropped whole rather
    ///    than clipped (`(42k)` cut to `(4` is noise);
    /// 4. **the title** - the only part that shrinks, and the first to be
    ///    reduced to a bare `…`.
    ///
    /// Returning the region *with* the string is the point: the caller cannot
    /// compute the glyph's column by any other rule without re-deriving the
    /// truncation, which is how the two drifted apart.
    /// Lay the horizontal bar out at `width` (gh #326).
    ///
    /// Four levers, applied in this order until the bar fits - the same
    /// priority the rail uses, outermost-in:
    ///
    /// 1. **flags** - whole flags come off cells with room to spare, `(badge)`
    ///    first, then `●`/`◌`, but **never the `*`** active marker: that is how the
    ///    bar says which session the user is in;
    /// 2. **`[+]`** - a convenience, not an affordance for something on screen;
    /// 3. **whole tabs**, from the right, never the active one;
    /// 4. **the titles** - the only part that is truncated, so the frame, the index
    ///    and the `×` survive to the last cell.
    ///
    /// The old code truncated the *painted row* instead, which ate the flags and
    /// the `×` and left the bar wider than its width anyway (measured: 4 tabs at
    /// width 80 painted 82 cells).
    fn layout_horizontal(&self, width: usize) -> (Vec<TabCell>, Option<(usize, usize, usize)>) {
        let active = self.active();
        // Per surviving tab: the index into `items`, its title, and which flags it
        // may still show.
        struct Slot {
            index: usize,
            title: String,
            badge: bool,
            markers: bool,
        }
        let mut slots: Vec<Slot> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| Slot {
                index,
                title: item.title.clone(),
                badge: item.badge.is_some(),
                markers: item.is_dirty || item.is_generating,
            })
            .collect();

        // The painted cell for a slot, honouring its surviving flags.
        let cell_of = |slot: &Slot, title: &str| -> String {
            let item = &self.items[slot.index];
            let mut flags = String::new();
            if Some(slot.index) == active {
                flags.push('*');
            }
            if slot.markers {
                if item.is_dirty {
                    flags.push('●');
                }
                if item.is_generating {
                    flags.push('◌');
                }
            }
            if slot.badge
                && let Some(badge) = &item.badge
            {
                flags.push_str(&format!(" ({badge})"));
            }
            if item.closable {
                flags.push_str(CLOSE_TEXT);
            }
            format!("[ {}. {title}{flags} ]", slot.index + 1)
        };
        let span_of = |slots: &[Slot], show_add: bool| -> usize {
            slots
                .iter()
                .map(|slot| visible_width(&cell_of(slot, &slot.title)))
                .sum::<usize>()
                + slots.len().saturating_sub(1) * 2
                + if show_add { 5 } else { 0 }
        };

        let mut show_add = self.show_add_button;

        // 1. Flags, one at a time, from the right - the least the bar can give up.
        //    `*` is never a candidate, so the active marker cannot vanish.
        'flags: while !slots.is_empty() && span_of(&slots, show_add) > width {
            for slot in slots.iter_mut().rev() {
                // The active tab's flags are fair game - only the `*` marker is not.
                if slot.badge {
                    slot.badge = false;
                    continue 'flags;
                }
                if slot.markers {
                    slot.markers = false;
                    continue 'flags;
                }
            }
            break;
        }

        // 2. `[+]`.
        if show_add && span_of(&slots, false) > width {
            show_add = false;
        }

        // 3. Whole tabs from the right, never the active one. The victim is chosen
        //    by **item index**, not slot position: once a slot is removed the two
        //    stop agreeing, and comparing them dropped the active tab (measured: an
        //    active third tab disappeared at width 40, leaving tab 1 on the bar).
        while slots.len() > 1 && span_of(&slots, show_add) > width {
            let active_slot = active.and_then(|a| slots.iter().position(|s| s.index == a));
            let victim = match active_slot {
                Some(slot) if slot + 1 == slots.len() => slots.len() - 2,
                _ => slots.len() - 1,
            };
            slots.remove(victim);
        }

        // 4. The titles take the leftover room.
        let mut remaining = width.saturating_sub(if show_add { 5 } else { 0 });
        let mut tabs: Vec<TabCell> = Vec::with_capacity(slots.len());
        for (position, slot) in slots.iter().enumerate() {
            remaining = remaining.saturating_sub(if position == 0 { 0 } else { 2 });
            let full = cell_of(slot, &slot.title);
            let text = if visible_width(&full) <= remaining {
                full
            } else {
                let frame = visible_width(&cell_of(slot, ""));
                let room = remaining.saturating_sub(frame);
                let shortened = truncate_to_width(&slot.title, room, "…", false);
                cell_of(slot, &shortened)
            };
            // A cell can still overrun when the frame alone is wider than the pane.
            // Tighten the frame, and as the true last resort hard-truncate: a bar
            // that paints past its width is the defect this function exists to
            // prevent, so the bound wins over showing a full `[ n. × ]`. At these
            // widths (1-4 cells) the pane cannot show a tab anyway.
            let text = if visible_width(&text) > remaining {
                let mut out = text;
                for tightened in [
                    out.replace(" × ]", " ×"),
                    out.replace("[ ", "["),
                    out.replace("[ ", "[").replace(" × ]", "×"),
                ] {
                    if visible_width(&tightened) < visible_width(&out) {
                        out = tightened;
                    }
                    if visible_width(&out) <= remaining {
                        break;
                    }
                }
                if visible_width(&out) > remaining && remaining > 0 {
                    out = truncate_to_width(&out, remaining, "", false);
                }
                out
            } else {
                text
            };
            remaining = remaining.saturating_sub(visible_width(&text));
            tabs.push(TabCell {
                row: 0,
                start: 0,
                end: visible_width(&text),
                text,
                close: None,
            });
        }

        // Positions, and a close region per painted glyph.
        let mut col = 0usize;
        for cell in tabs.iter_mut() {
            cell.start = col;
            cell.end = col + visible_width(&cell.text);
            cell.close = rendered_close_region(&cell.text, cell.text.contains('×'));
            col = cell.end + 2;
        }
        let add = (show_add && col + 3 <= width).then(|| (0, col, col + 3));
        (tabs, add)
    }

    fn vertical_cell(
        item: &TabItem,
        index: usize,
        active: bool,
        width: usize,
    ) -> (String, Option<(usize, usize)>) {
        let marker = if active { "▶ " } else { "  " };
        let head = format!("{marker}[{}] ", index + 1);
        // The state flags **without** the close glyph: `flags` paints the glyph as
        // its last element, and the reservation below needs the two separately.
        let state = {
            let mut item_no_close = item.clone();
            item_no_close.closable = false;
            Self::flags(&item_no_close, active)
        };
        let close_text = if item.closable { CLOSE_TEXT } else { "" };
        let head_width = visible_width(&head);
        let close_width = visible_width(close_text);
        let state_width = visible_width(&state);

        // **The `×` is reserved before everything else**, including the index
        // head, because it is the only thing on this row the user can act on.
        // `width >= head + ×` is the normal case; below that the rail is too
        // narrow to name the tab *and* close it, and the brief's own answer
        // applies - the glyph alone, right-aligned where a close button belongs,
        // rather than a row that ends in `…` with a phantom region behind it.
        if width < head_width + close_width {
            if width < close_width {
                return (truncate_to_width(&head, width, "…", false), None);
            }
            let mut out = " ".repeat(width - close_width);
            out.push_str(close_text.trim_start());
            let region = rendered_close_region(&out, item.closable);
            return (out, region);
        }
        let mut room = width - head_width - close_width;

        // State flags next, dropped **whole** rather than clipped: a lone `*` or a
        // truncated `(4` reads as state that is not there. The dirty/generating
        // markers are single cells, so they survive longer than a badge, which is
        // the right order - a badge repeats a number the host also shows.
        let state_text = if state_width <= room {
            state
        } else {
            String::new()
        };
        room -= visible_width(&state_text);

        let title = truncate_to_width(&item.title, room, "…", false);
        let out = format!("{head}{title}{state_text}{close_text}");
        let region = rendered_close_region(&out, item.closable);
        (out, region)
    }

    /// Lay out the widget at `width` (gh #208): titles shrink with `…`
    /// while the bar overflows, keeping the active tab whole longest.
    fn layout(&self, width: u16) -> TabLayout {
        let width = width as usize;
        let active = self.active();
        if self.orientation == TabOrientation::Horizontal {
            let (tabs, add) = self.layout_horizontal(width);
            TabLayout { tabs, add, rows: 1 }
        } else {
            let mut tabs = Vec::with_capacity(self.items.len());
            for (index, item) in self.items.iter().enumerate() {
                let (cell, close) = Self::vertical_cell(item, index, Some(index) == active, width);
                tabs.push(TabCell {
                    row: index,
                    start: 0,
                    end: visible_width(&cell),
                    text: cell,
                    close,
                });
            }
            let add = self.show_add_button.then(|| {
                let row = self.items.len();
                (row, 0, 3)
            });
            TabLayout {
                tabs,
                add,
                rows: self.items.len() + usize::from(self.show_add_button),
            }
        }
    }

    /// Render the widget at `width` (gh #208): one bar row, or one rail
    /// row per tab plus the add row.
    pub fn render(&self, width: u16) -> Vec<String> {
        let layout = self.layout(width);
        if self.orientation == TabOrientation::Horizontal {
            let mut row = String::new();
            for (index, cell) in layout.tabs.iter().enumerate() {
                if index > 0 {
                    row.push_str("  ");
                }
                row.push_str(&cell.text);
            }
            if layout.add.is_some() {
                if !self.items.is_empty() {
                    row.push_str("  ");
                }
                row.push_str("[+]");
                // The filler carries the bar to the width (OpenCode's rail).
                let fill = (width as usize).saturating_sub(visible_width(&row));
                row.push_str(&"─".repeat(fill));
            }
            vec![row]
        } else {
            let mut rows = Vec::with_capacity(layout.rows);
            // The painted row comes from the layout, which is also what the hit
            // test measures (gh #326): rebuilding it here is how the two drifted.
            rows.extend(layout.tabs.iter().map(|cell| cell.text.clone()));
            if layout.add.is_some() {
                rows.push("[+]".to_string());
            }
            rows
        }
    }

    /// The span of one tab for host styling (gh #208): `(row, start,
    /// end)` cells, or `None` past the list.
    pub fn tab_span(&self, index: usize, width: u16) -> Option<(usize, usize, usize)> {
        let layout = self.layout(width);
        layout
            .tabs
            .get(index)
            .map(|cell| (cell.row, cell.start, cell.end))
    }

    /// The span of the `[+]` cell for host styling (gh #208).
    pub fn add_span(&self, width: u16) -> Option<(usize, usize, usize)> {
        self.layout(width).add
    }

    /// Which cell a widget-local pointer cell hits (gh #208).
    pub fn tab_at(&self, col: u16, row: u16, width: u16) -> Option<TabHit> {
        let layout = self.layout(width);
        let (col, row) = (col as usize, row as usize);
        for (index, cell) in layout.tabs.iter().enumerate() {
            if cell.row == row && col >= cell.start && col < cell.end {
                if let Some((start, end)) = cell.close
                    && col >= start
                    && col < end
                {
                    return Some(TabHit::Close(index));
                }
                return Some(TabHit::Tab(index));
            }
        }
        if let Some((add_row, start, end)) = layout.add
            && add_row == row
            && col >= start
            && col < end
        {
            return Some(TabHit::Add);
        }
        None
    }

    /// Drive the widget from the keyboard (gh #208): arrows walk
    /// (wrapping, vertical uses up/down), digits jump, Enter confirms,
    /// Ctrl+W closes. Accepts key names and raw sequences.
    pub fn handle_key(&mut self, key: &str) -> Option<TabEvent> {
        if self.items.is_empty() {
            return None;
        }
        let last = self.items.len() - 1;
        let vertical = self.orientation == TabOrientation::Vertical;
        match key {
            "left" | "\x1b[D" if !vertical => {
                self.active_index = self.active().unwrap_or(0).checked_sub(1).unwrap_or(last);
                self.hovered = self.active();
                None
            }
            "right" | "\x1b[C" if !vertical => {
                self.active_index = (self.active().unwrap_or(0) + 1) % self.items.len();
                self.hovered = self.active();
                None
            }
            "up" | "\x1b[A" if vertical => {
                self.active_index = self.active().unwrap_or(0).checked_sub(1).unwrap_or(last);
                self.hovered = self.active();
                None
            }
            "down" | "\x1b[B" if vertical => {
                self.active_index = (self.active().unwrap_or(0) + 1) % self.items.len();
                self.hovered = self.active();
                None
            }
            "enter" | "\r" => Some(TabEvent::Select(self.active().unwrap_or(0))),
            "ctrl+w" | "\x17" => {
                let active = self.active().unwrap_or(0);
                if self.items.get(active).is_some_and(|item| item.closable) {
                    Some(TabEvent::Close(active))
                } else {
                    None
                }
            }
            // Tabs are numbered from 1, so `0` names no tab and must be a no-op
            // (gh #326). Without the guard, `0.saturating_sub(1) == 0` silently
            // made it a jump to the first tab.
            "0" => None,
            digit if digit.len() == 1 && digit.starts_with(|c: char| c.is_ascii_digit()) => {
                let index = digit.parse::<usize>().unwrap_or(0).saturating_sub(1);
                if index < self.items.len() {
                    self.active_index = index;
                    self.hovered = Some(index);
                    Some(TabEvent::Select(index))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Drive the widget from the keystone mouse (gh #208): a press
    /// previews the tab, a release on the same cell confirms; `×`
    /// closes, `[+]` adds. Coordinates are widget-local.
    pub fn handle_mouse(&mut self, event: MouseEvent, width: u16) -> Option<TabEvent> {
        match event {
            MouseEvent::Down {
                col,
                row,
                button: MouseButton::Left,
                ..
            } => {
                match self.tab_at(col, row, width) {
                    Some(TabHit::Tab(index)) => {
                        self.active_index = index;
                        self.hovered = Some(index);
                    }
                    Some(TabHit::Close(index)) => {
                        self.hovered = None;
                        let _ = index;
                    }
                    _ => {}
                }
                None
            }
            MouseEvent::Move {
                col,
                row,
                button: None,
                ..
            } => {
                self.hovered = match self.tab_at(col, row, width) {
                    Some(TabHit::Tab(index)) => Some(index),
                    _ => None,
                };
                None
            }
            MouseEvent::Up {
                col,
                row,
                button: MouseButton::Left,
                ..
            } => match self.tab_at(col, row, width) {
                Some(TabHit::Tab(index)) => {
                    self.active_index = index;
                    self.hovered = Some(index);
                    Some(TabEvent::Select(index))
                }
                Some(TabHit::Close(index)) => Some(TabEvent::Close(index)),
                Some(TabHit::Add) => Some(TabEvent::Add),
                None => None,
            },
            _ => None,
        }
    }
}
