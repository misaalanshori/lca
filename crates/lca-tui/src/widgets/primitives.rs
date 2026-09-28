//! Widget primitives, ported from pi's `packages/tui/src/components/`
//! (`pi-tui-re/src_re/tui-widgets/primitives.md`,
//! `pickers-and-fields.md`).
//!
//! Not ported (documented skips): the box/hstack/vstack/spacer widgets (the
//! `layout` engine composes stacks instead), the image widget (LCA's
//! terminal image support is deferred; a labelled placeholder stands in),
//! and the alt-screen flash/search widgets.

use crate::engine::core::{CURSOR_MARKER, Component};
use crate::engine::keybindings::KeybindingsManager;
use crate::engine::text::{truncate_to_width, visible_width, wrap_text_with_ansi};
use std::sync::Arc;

/// A static block of already-styled lines.
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    /// From lines.
    pub fn new(lines: Vec<String>) -> Self {
        Self { lines }
    }

    /// From one string, split on newlines.
    pub fn from_text(text: &str) -> Self {
        Self {
            lines: text.split('\n').map(|s| s.to_string()).collect(),
        }
    }

    /// Replace the content.
    pub fn set_lines(&mut self, lines: Vec<String>) {
        self.lines = lines;
    }
}

impl Component for Text {
    fn render(&mut self, _width: u16) -> Vec<String> {
        self.lines.clone()
    }
}

/// Text truncated to a fixed width with an ellipsis.
pub struct TruncatedText {
    text: String,
    width: usize,
}

impl TruncatedText {
    /// A new truncated text.
    pub fn new(text: impl Into<String>, width: usize) -> Self {
        Self {
            text: text.into(),
            width,
        }
    }
}

impl Component for TruncatedText {
    fn render(&mut self, width: u16) -> Vec<String> {
        let w = if self.width == 0 {
            width as usize
        } else {
            self.width
        };
        vec![truncate_to_width(&self.text, w, "…", false)]
    }
}

/// One selectable row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectItem {
    /// The value.
    pub value: String,
    /// The display label.
    pub label: String,
    /// An optional description.
    pub description: Option<String>,
}

/// A keyboard-driven selection list.
pub struct SelectList {
    items: Vec<SelectItem>,
    cursor: usize,
    keybindings: Arc<KeybindingsManager>,
}

impl SelectList {
    /// A new select list.
    pub fn new(items: Vec<SelectItem>) -> Self {
        Self {
            items,
            cursor: 0,
            keybindings: Arc::new(KeybindingsManager::new()),
        }
    }

    /// Use a keybinding set.
    pub fn set_keybindings(&mut self, kb: Arc<KeybindingsManager>) {
        self.keybindings = kb;
    }

    /// Replace the items, resetting the cursor.
    pub fn set_items(&mut self, items: Vec<SelectItem>) {
        self.items = items;
        self.cursor = 0;
    }

    /// The current cursor index.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The items.
    pub fn items(&self) -> &[SelectItem] {
        &self.items
    }

    /// Move the cursor by a delta (wrapping).
    pub fn move_cursor(&mut self, delta: i32) {
        let n = self.items.len() as i32;
        if n == 0 {
            return;
        }
        self.cursor = ((self.cursor as i32 + delta).rem_euclid(n)) as usize;
    }

    /// The selected item.
    pub fn selected(&self) -> Option<&SelectItem> {
        self.items.get(self.cursor)
    }

    /// Handle a key. Returns `Some(value)` when confirmed.
    pub fn handle_key(&mut self, data: &str) -> Option<String> {
        let kb = self.keybindings.clone();
        if kb.matches(data, "tui.select.up") {
            self.move_cursor(-1);
        } else if kb.matches(data, "tui.select.down") {
            self.move_cursor(1);
        } else if kb.matches(data, "tui.select.confirm") {
            return self.selected().map(|i| i.value.clone());
        }
        None
    }
}

impl Component for SelectList {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let marker = if i == self.cursor { "▸ " } else { "  " };
                let desc = item
                    .description
                    .as_ref()
                    .map(|d| format!("  {d}"))
                    .unwrap_or_default();
                truncate_to_width(
                    &format!("{marker}{}{desc}", item.label),
                    width as usize,
                    "…",
                    false,
                )
            })
            .collect()
    }
}

/// A scrollable view over a component's rendered lines.
pub struct ScrollView {
    content: Box<dyn Component>,
    scroll_top: u16,
    follow_end: bool,
}

impl ScrollView {
    /// A new scroll view; `follow_end` keeps the bottom visible.
    pub fn new(content: Box<dyn Component>, follow_end: bool) -> Self {
        Self {
            content,
            scroll_top: 0,
            follow_end,
        }
    }

    /// The current scroll offset.
    pub fn scroll_top(&self) -> u16 {
        self.scroll_top
    }

    /// Scroll by a delta.
    pub fn scroll_by(&mut self, delta: i32) {
        // Saturate rather than wrap: past u16::MAX a truncated offset would
        // jump the list back near the top.
        self.scroll_top =
            u16::try_from((i32::from(self.scroll_top) + delta).max(0)).unwrap_or(u16::MAX);
        self.follow_end = false;
    }

    /// Scroll to the top.
    pub fn scroll_to_top(&mut self) {
        self.scroll_top = 0;
        self.follow_end = false;
    }

    /// Scroll to the bottom.
    pub fn scroll_to_end(&mut self) {
        self.follow_end = true;
    }

    /// Render the visible window at a height.
    pub fn render_window(&mut self, width: u16, height: u16) -> Vec<String> {
        let content = self.content.render(width);
        let content_len = content.len() as u16;
        let max_top = content_len.saturating_sub(height);
        if self.follow_end {
            self.scroll_top = max_top;
        }
        self.scroll_top = self.scroll_top.min(max_top);
        let top = self.scroll_top as usize;
        content
            .into_iter()
            .skip(top)
            .take(height as usize)
            .collect()
    }
}

/// An animated loader line.
pub struct Loader {
    frames: Vec<&'static str>,
    frame: usize,
    label: String,
}

impl Loader {
    /// A new loader.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            frames: vec!["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
            frame: 0,
            label: label.into(),
        }
    }

    /// Advance the animation.
    pub fn tick(&mut self) {
        self.frame = (self.frame + 1) % self.frames.len();
    }

    /// Change the label.
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = label.into();
    }
}

impl Component for Loader {
    fn render(&mut self, _width: u16) -> Vec<String> {
        vec![format!("{} {}", self.frames[self.frame], self.label)]
    }
}

/// A labelled placeholder for an image (LCA's terminal image support is
/// deferred; the data still travels to vision providers — ADR-0029).
pub struct ImagePlaceholder {
    label: String,
}

impl ImagePlaceholder {
    /// A placeholder for `label`.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl Component for ImagePlaceholder {
    fn render(&mut self, width: u16) -> Vec<String> {
        let inner = format!("🖼  {}", self.label);
        let w = visible_width(&inner).min(width as usize);
        vec![truncate_to_width(
            &format!("[{}]", inner),
            w + 2,
            "…",
            false,
        )]
    }
}

/// Render a component's lines, wrapping long lines to `width`.
pub fn wrapped_lines(component: &mut dyn Component, width: u16) -> Vec<String> {
    component
        .render(width)
        .into_iter()
        .flat_map(|l| wrap_text_with_ansi(&l, width as usize))
        .collect()
}

/// A cursor marker constant re-export for widgets.
pub const MARKER: &str = CURSOR_MARKER;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_list_moves_and_confirms() {
        let mut list = SelectList::new(vec![
            SelectItem {
                value: "a".into(),
                label: "A".into(),
                description: None,
            },
            SelectItem {
                value: "b".into(),
                label: "B".into(),
                description: None,
            },
        ]);
        list.handle_key("\x1b[B");
        assert_eq!(list.cursor(), 1);
        assert_eq!(list.handle_key("\r"), Some("b".to_string()));
    }

    #[test]
    fn scroll_view_follows_end() {
        let content = Box::new(Text::new((0..20).map(|i| format!("l{i}")).collect()));
        let mut view = ScrollView::new(content, true);
        let window = view.render_window(10, 5);
        assert_eq!(window, vec!["l15", "l16", "l17", "l18", "l19"]);
    }

    #[test]
    fn scroll_view_clamps_at_top() {
        let content = Box::new(Text::new((0..3).map(|i| format!("l{i}")).collect()));
        let mut view = ScrollView::new(content, false);
        view.scroll_by(-5);
        assert_eq!(view.scroll_top(), 0);
    }

    #[test]
    fn loader_animates() {
        let mut loader = Loader::new("working");
        let a = loader.render(20);
        loader.tick();
        let b = loader.render(20);
        assert_ne!(a, b);
    }
}
