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

/// One laid-out tab cell: where it paints and where its `×` sits.
struct TabCell {
    row: usize,
    start: usize,
    end: usize,
    close: Option<(usize, usize)>,
}

/// The laid-out widget: every reader (render, hit-test, spans) walks
/// the same layout - including the truncated titles - so they can
/// never disagree.
struct TabLayout {
    tabs: Vec<TabCell>,
    /// The painted (possibly truncated) titles, one per tab.
    titles: Vec<String>,
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
            flags.push_str(" ×");
        }
        flags
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
                    close,
                });
                col = end + 2;
            }
            let add = self.show_add_button.then(|| {
                let start = col;
                (0, start, start + 3)
            });
            TabLayout {
                tabs,
                titles,
                add,
                rows: 1,
            }
        } else {
            let mut tabs = Vec::with_capacity(self.items.len());
            for (index, item) in self.items.iter().enumerate() {
                let flags = Self::flags(item, Some(index) == active);
                let marker = if Some(index) == active { "▶ " } else { "  " };
                let mut cell = format!("{marker}[{}] {}{flags}", index + 1, item.title);
                cell = truncate_to_width(&cell, width, "…", false);
                let close = item.closable.then(|| {
                    let end = visible_width(&cell);
                    (end.saturating_sub(1), end)
                });
                tabs.push(TabCell {
                    row: index,
                    start: 0,
                    end: visible_width(&cell),
                    close,
                });
            }
            let add = self.show_add_button.then(|| {
                let row = self.items.len();
                (row, 0, 3)
            });
            TabLayout {
                tabs,
                titles,
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
                let item = &self.items[index];
                let flags = Self::flags(item, Some(index) == self.active());
                let title = &layout.titles[index];
                row.push_str(&format!("[ {}. {title}{flags} ]", index + 1));
                let _ = cell;
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
            for (index, cell) in layout.tabs.iter().enumerate() {
                let item = &self.items[index];
                let flags = Self::flags(item, Some(index) == self.active());
                let marker = if Some(index) == self.active() {
                    "▶ "
                } else {
                    "  "
                };
                let mut row = format!("{marker}[{}] {}{flags}", index + 1, item.title);
                row = truncate_to_width(&row, width as usize, "…", false);
                rows.push(row);
                let _ = cell;
            }
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
