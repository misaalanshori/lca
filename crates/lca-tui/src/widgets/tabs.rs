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
        let mut titles: Vec<String> = self.items.iter().map(|item| item.title.clone()).collect();
        if self.orientation == TabOrientation::Horizontal {
            // The chrome around each title: `[ {n}. ` + flags + ` ]`
            // plus the two-space gutter and the `[+]` cell.
            let chrome = |index: usize| {
                let item = &self.items[index];
                let flags = Self::flags(item, Some(index) == active);
                3 + format!("{}. ", index + 1).len() + visible_width(&flags) + 2
            };
            let gutter = self.items.len().saturating_sub(1) * 2;
            let add_cell = if self.show_add_button { 5 } else { 0 };
            let total_of = |titles: &[String]| {
                titles
                    .iter()
                    .enumerate()
                    .map(|(i, t)| chrome(i) + visible_width(t))
                    .sum::<usize>()
                    + gutter
                    + add_cell
            };
            let mut total = total_of(&titles);
            // Shave the longest titles first; the active tab shaves
            // last. Each pass strictly narrows one title, so the loop
            // always terminates (a 1-cell title keeps its `…`).
            while total > width {
                let mut victim: Option<usize> = None;
                let mut best = 1usize;
                for (index, title) in titles.iter().enumerate() {
                    if Some(index) == active {
                        continue;
                    }
                    let w = visible_width(title);
                    if w > best {
                        best = w;
                        victim = Some(index);
                    }
                }
                let victim = victim.or_else(|| {
                    titles
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| Some(*i) != active)
                        .max_by_key(|(_, t)| visible_width(t))
                        .map(|(i, _)| i)
                });
                let victim = victim.or_else(|| {
                    titles
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, t)| visible_width(t))
                        .map(|(i, _)| i)
                });
                let Some(victim) = victim else {
                    break;
                };
                if visible_width(&titles[victim]) <= 1 {
                    break;
                }
                let spare = total.saturating_sub(width).max(1);
                let narrowed = visible_width(&titles[victim]).saturating_sub(spare).max(1);
                titles[victim] = truncate_to_width(&self.items[victim].title, narrowed, "…", false);
                let now = total_of(&titles);
                if now >= total {
                    break;
                }
                total = now;
            }
            let mut tabs = Vec::with_capacity(self.items.len());
            let mut col = 0usize;
            for (index, title) in titles.iter().enumerate() {
                let item = &self.items[index];
                let flags = Self::flags(item, Some(index) == active);
                let cell = format!("[ {}. {title}{flags} ]", index + 1);
                let start = col;
                let end = start + visible_width(&cell);
                let close = item.closable.then(|| {
                    // The `×` owns its own cell before the frame.
                    (end.saturating_sub(3), end.saturating_sub(2))
                });
                tabs.push(TabCell {
                    row: 0,
                    start,
                    end,
                    text: cell,
                    close,
                });
                col = end + 2;
            }
            let add = self.show_add_button.then(|| {
                let start = col;
                (0, start, start + 3)
            });
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
