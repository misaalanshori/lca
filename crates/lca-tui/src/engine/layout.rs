//! Box layout engine, ported from pi's `layout.ts` + `layout-node.ts`
//! (`pi-tui-re/src_re/tui-engine/layout.md`).
//!
//! A box/clipping compositor with flex stacks and scroll views. It paints
//! into a screen of line strings using the `text` surgery primitives, so
//! wide characters and ANSI styles survive splicing.
//!
//! Deviation from pi (documented): this port keeps the string-stacking
//! `Container` (in `core`) for the main-screen transcript and uses this box
//! engine for the alt-screen/fullscreen layout, matching pi's two-system
//! split (RE doc §6).

use super::core::Component;
use super::text::{
    extract_ansi_code, get_grapheme_cell_range, slice_by_column, truncate_to_width, visible_width,
};

/// Vertical alignment for an hstack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Fill the allocated height.
    #[default]
    Stretch,
    /// Align to the top.
    Start,
    /// Center vertically.
    Center,
    /// Align to the bottom.
    End,
}

/// A flex-sized stack entry.
pub struct StackEntry {
    /// The entry's node.
    pub node: LayoutNode,
    /// Fixed main-axis size.
    pub basis: Option<u16>,
    /// Whether the entry grows to fill free space.
    pub grow: bool,
    /// Whether the entry shrinks when space is short.
    pub shrink: bool,
    /// Minimum main-axis size.
    pub min_size: Option<u16>,
    /// Maximum main-axis size.
    pub max_size: Option<u16>,
    /// Whether the entry participates this frame.
    pub visible: bool,
}

impl StackEntry {
    /// A leaf entry with default flex behaviour.
    pub fn leaf(node: LayoutNode) -> Self {
        Self {
            node,
            basis: None,
            grow: false,
            shrink: true,
            min_size: None,
            max_size: None,
            visible: true,
        }
    }

    /// A growable entry.
    pub fn grow(node: LayoutNode) -> Self {
        Self {
            grow: true,
            ..Self::leaf(node)
        }
    }
}

/// Scroll state carried by a scroll node.
#[derive(Debug, Clone, Copy)]
pub struct ScrollState {
    /// Rows scrolled from the top.
    pub scroll_top: u16,
    /// Whether wheel overscroll chains to the parent.
    pub overscroll_chain: bool,
    /// Last measured viewport height.
    pub viewport_height: u16,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            scroll_top: 0,
            overscroll_chain: true,
            viewport_height: 0,
        }
    }
}

/// A layout tree node.
pub enum LayoutNode {
    /// A leaf component.
    Leaf(Box<dyn Component>),
    /// A vertical stack.
    VStack {
        /// The entries.
        entries: Vec<StackEntry>,
        /// Gap between entries.
        gap: u16,
    },
    /// A horizontal stack.
    HStack {
        /// The entries.
        entries: Vec<StackEntry>,
        /// Gap between entries.
        gap: u16,
        /// Vertical alignment.
        align: Align,
    },
    /// A scroll view.
    Scroll {
        /// The scrolled component.
        component: Box<dyn Component>,
        /// Scroll state.
        state: ScrollState,
        /// Whether this is the primary scroll view.
        primary: bool,
    },
}

impl LayoutNode {
    /// A leaf node.
    pub fn leaf(component: Box<dyn Component>) -> Self {
        LayoutNode::Leaf(component)
    }

    /// A vertical stack.
    pub fn vstack(entries: Vec<StackEntry>, gap: u16) -> Self {
        LayoutNode::VStack { entries, gap }
    }

    /// A horizontal stack.
    pub fn hstack(entries: Vec<StackEntry>, gap: u16, align: Align) -> Self {
        LayoutNode::HStack {
            entries,
            gap,
            align,
        }
    }

    /// A scroll view.
    pub fn scroll(component: Box<dyn Component>, primary: bool) -> Self {
        LayoutNode::Scroll {
            component,
            state: ScrollState::default(),
            primary,
        }
    }

    /// The primary scroll view's state, if this node is one.
    pub fn primary_scroll(&self) -> Option<&ScrollState> {
        match self {
            LayoutNode::Scroll {
                state,
                primary: true,
                ..
            } => Some(state),
            _ => None,
        }
    }
}

/// The result of rendering a frame.
pub struct Frame {
    /// The painted lines.
    pub lines: Vec<String>,
    /// Whether a primary scroll view was present.
    pub had_primary_scroll: bool,
}

/// Allocate main-axis sizes across visible stack entries.
#[allow(clippy::too_many_arguments)] // mirrors pi's allocateStackSizes signature
pub fn allocate_stack_sizes(
    intrinsic: &[u16],
    visible: &[bool],
    grow: &[bool],
    shrink: &[bool],
    min_size: &[u16],
    max_size: &[u16],
    gap: u16,
    total: u16,
) -> Vec<u16> {
    let n = intrinsic.len();
    let mut sizes = vec![0u16; n];
    let visible_indices: Vec<usize> = (0..n).filter(|&i| visible[i]).collect();
    if visible_indices.is_empty() {
        return sizes;
    }
    let gaps = gap.saturating_mul((visible_indices.len() as u16).saturating_sub(1));
    let avail = total.saturating_sub(gaps);

    let mut fixed: u16 = 0;
    let mut grow_count = 0u16;
    for &i in &visible_indices {
        let base = intrinsic[i].clamp(min_size[i], max_size[i].max(min_size[i]));
        sizes[i] = base;
        fixed = fixed.saturating_add(base);
        if grow[i] {
            grow_count += 1;
        }
    }

    if fixed <= avail {
        if grow_count > 0 {
            let free = avail - fixed;
            if let Some(share) = free.checked_div(grow_count) {
                let mut remainder = free % grow_count;
                for &i in &visible_indices {
                    if grow[i] {
                        let extra = share + u16::from(remainder > 0);
                        remainder = remainder.saturating_sub(1);
                        sizes[i] = (sizes[i] + extra).min(max_size[i].max(sizes[i]));
                    }
                }
            }
        }
    } else {
        // Shrink proportionally from shrinkable entries.
        let mut overflow = fixed - avail;
        let shrinkable: Vec<usize> = visible_indices
            .iter()
            .copied()
            .filter(|&i| shrink[i] && sizes[i] > min_size[i])
            .collect();
        while overflow > 0 {
            let mut progressed = false;
            for &i in &shrinkable {
                if overflow == 0 {
                    break;
                }
                if sizes[i] > min_size[i] {
                    sizes[i] -= 1;
                    overflow -= 1;
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
    }
    sizes
}

/// Render a layout tree into `height` lines of `width` columns.
pub fn render_frame(node: &mut LayoutNode, width: u16, height: u16) -> Frame {
    let mut screen: Vec<String> = vec![String::new(); height as usize];
    let mut primary = false;
    paint(node, 0, 0, width, height, &mut screen, &mut primary);
    for line in &mut screen {
        if line.is_empty() {
            *line = String::new();
        }
    }
    Frame {
        lines: screen,
        had_primary_scroll: primary,
    }
}

/// Paint one node into the screen at the given rect.
pub fn paint(
    node: &mut LayoutNode,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    screen: &mut [String],
    primary: &mut bool,
) {
    match node {
        LayoutNode::Leaf(component) => {
            let lines = component.render(w);
            for (row, line) in lines.iter().enumerate() {
                if row as u16 >= h {
                    break;
                }
                paint_text(screen, y + row as u16, x, w, line);
            }
        }
        LayoutNode::VStack { entries, gap } => {
            let visible: Vec<bool> = entries.iter().map(|e| e.visible).collect();
            let grow: Vec<bool> = entries.iter().map(|e| e.grow).collect();
            let shrink: Vec<bool> = entries.iter().map(|e| e.shrink).collect();
            let min: Vec<u16> = entries.iter().map(|e| e.min_size.unwrap_or(0)).collect();
            let max: Vec<u16> = entries
                .iter()
                .map(|e| e.max_size.unwrap_or(u16::MAX))
                .collect();
            let intrinsic: Vec<u16> = entries
                .iter_mut()
                .map(|e| match &e.basis {
                    Some(b) => *b,
                    None => measure(&mut e.node, w),
                })
                .collect();
            let sizes =
                allocate_stack_sizes(&intrinsic, &visible, &grow, &shrink, &min, &max, *gap, h);
            let mut cursor = y;
            for (i, entry) in entries.iter_mut().enumerate() {
                if !entry.visible {
                    continue;
                }
                let size = sizes[i];
                if size == 0 {
                    continue;
                }
                paint(&mut entry.node, x, cursor, w, size, screen, primary);
                cursor = cursor.saturating_add(size).saturating_add(*gap);
                if cursor >= y + h {
                    break;
                }
            }
        }
        LayoutNode::HStack {
            entries,
            gap,
            align,
        } => {
            let visible: Vec<bool> = entries.iter().map(|e| e.visible).collect();
            let grow: Vec<bool> = entries.iter().map(|e| e.grow).collect();
            let shrink: Vec<bool> = entries.iter().map(|e| e.shrink).collect();
            let min: Vec<u16> = entries.iter().map(|e| e.min_size.unwrap_or(0)).collect();
            let max: Vec<u16> = entries
                .iter()
                .map(|e| e.max_size.unwrap_or(u16::MAX))
                .collect();
            let intrinsic: Vec<u16> = entries
                .iter_mut()
                .map(|e| match &e.basis {
                    Some(b) => *b,
                    None => measure_width(&mut e.node, w),
                })
                .collect();
            let sizes =
                allocate_stack_sizes(&intrinsic, &visible, &grow, &shrink, &min, &max, *gap, w);
            let mut cursor = x;
            for (i, entry) in entries.iter_mut().enumerate() {
                if !entry.visible {
                    continue;
                }
                let size = sizes[i];
                if size == 0 {
                    continue;
                }
                let child_h = measure(&mut entry.node, size).min(h);
                let child_y = match align {
                    Align::Stretch => y,
                    Align::Start => y,
                    Align::Center => y + (h.saturating_sub(child_h)) / 2,
                    Align::End => y + h.saturating_sub(child_h),
                };
                let child_h = match align {
                    Align::Stretch => h,
                    _ => child_h,
                };
                paint(
                    &mut entry.node,
                    cursor,
                    child_y,
                    size,
                    child_h,
                    screen,
                    primary,
                );
                cursor = cursor.saturating_add(size).saturating_add(*gap);
                if cursor >= x + w {
                    break;
                }
            }
        }
        LayoutNode::Scroll {
            component,
            state,
            primary: is_primary,
        } => {
            let content = component.render(w);
            let content_height = content.len() as u16;
            state.viewport_height = h;
            let max_top = content_height.saturating_sub(h);
            if state.scroll_top > max_top {
                state.scroll_top = max_top;
            }
            if *is_primary {
                *primary = true;
            }
            let top = state.scroll_top as usize;
            for row in 0..h as usize {
                let idx = top + row;
                let Some(line) = content.get(idx) else {
                    break;
                };
                paint_text(screen, y + row as u16, x, w, line);
            }
            // Scrollbar when the content overflows.
            if content_height > h && w > 0 {
                paint_scrollbar(screen, x + w - 1, y, h, content_height, state.scroll_top);
            }
        }
    }
}

fn measure(node: &mut LayoutNode, width: u16) -> u16 {
    match node {
        LayoutNode::Leaf(component) => component.render(width).len() as u16,
        LayoutNode::VStack { entries, gap } => {
            let mut total = 0u16;
            let mut count = 0u16;
            for entry in entries.iter_mut() {
                if !entry.visible {
                    continue;
                }
                total = total.saturating_add(match entry.basis {
                    Some(b) => b,
                    None => measure(&mut entry.node, width),
                });
                count += 1;
            }
            total.saturating_add(gap.saturating_mul(count.saturating_sub(1)))
        }
        LayoutNode::HStack { entries, .. } => entries
            .iter_mut()
            .filter(|e| e.visible)
            .map(|e| measure(&mut e.node, width))
            .max()
            .unwrap_or(0),
        LayoutNode::Scroll { component, .. } => component.render(width).len() as u16,
    }
}

fn measure_width(node: &mut LayoutNode, width: u16) -> u16 {
    match node {
        LayoutNode::Leaf(component) => component
            .render(width)
            .iter()
            .map(|l| visible_width(l) as u16)
            .max()
            .unwrap_or(0),
        LayoutNode::VStack { entries, .. } => entries
            .iter_mut()
            .filter(|e| e.visible)
            .map(|e| match e.basis {
                Some(b) => b,
                None => measure_width(&mut e.node, width),
            })
            .max()
            .unwrap_or(0),
        LayoutNode::HStack { entries, gap, .. } => {
            let mut total = 0u16;
            let mut count = 0u16;
            for entry in entries.iter_mut().filter(|e| e.visible) {
                total = total.saturating_add(match entry.basis {
                    Some(b) => b,
                    None => measure_width(&mut entry.node, width),
                });
                count += 1;
            }
            total.saturating_add(gap.saturating_mul(count.saturating_sub(1)))
        }
        LayoutNode::Scroll { component, .. } => component
            .render(width)
            .iter()
            .map(|l| visible_width(l) as u16)
            .max()
            .unwrap_or(0),
    }
}

fn paint_text(screen: &mut [String], row: u16, x: u16, width: u16, text: &str) {
    let Some(target) = screen.get_mut(row as usize) else {
        return;
    };
    let clipped = truncate_to_width(text, width as usize, "", false);
    let stripped = strip_osc133(&clipped);
    let base = std::mem::take(target);
    let before = slice_by_column(&base, 0, x as usize, false);
    let before_padded = format!(
        "{before}{}",
        " ".repeat(x.saturating_sub(visible_width(&before) as u16) as usize)
    );
    let after = slice_by_column(&base, (x + width) as usize, 10_000, false);
    let stripped_w = visible_width(&stripped) as u16;
    let mut result = format!("{before_padded}{stripped}");
    if !after.is_empty() {
        result.push_str(&" ".repeat(width.saturating_sub(stripped_w) as usize));
        result.push_str(&after);
    }
    *target = result;
}

/// Strip OSC 133 semantic prompt zone prefixes from a content line.
pub fn strip_osc133(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if let Some((code, len)) = extract_ansi_code(line, i) {
            let is_zone = code.starts_with("\x1b]133;");
            i += len;
            if is_zone {
                continue;
            }
            out.push_str(&code);
            continue;
        }
        let ch = line[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn paint_scrollbar(screen: &mut [String], col: u16, y: u16, height: u16, content: u16, top: u16) {
    if height == 0 || content == 0 {
        return;
    }
    let track = height as u32;
    let thumb = ((track * track) / content as u32).max(2).min(track);
    let max_top = (content.saturating_sub(height)) as u32;
    let thumb_top = (top as u32 * (track - thumb))
        .checked_div(max_top)
        .unwrap_or(0) as u16;
    for row in 0..height {
        let glyph = if row >= thumb_top && row < thumb_top + thumb as u16 {
            "█"
        } else {
            "│"
        };
        replace_cell(screen, col, y + row, glyph);
    }
}

fn replace_cell(screen: &mut [String], col: u16, row: u16, glyph: &str) {
    let Some(target) = screen.get_mut(row as usize) else {
        return;
    };
    let base = std::mem::take(target);
    // Grapheme-aware: do not split a wide char occupying `col`.
    let (start, end) =
        get_grapheme_cell_range(&base, col as usize).unwrap_or((col as usize, col as usize + 1));
    let before = slice_by_column(&base, 0, start, false);
    let before_padded = format!(
        "{before}{}",
        " ".repeat(start.saturating_sub(visible_width(&before)))
    );
    let after = slice_by_column(&base, end, 10_000, false);
    let glyph_width = visible_width(glyph);
    let filler = " ".repeat((end - start).saturating_sub(glyph_width));
    *target = format!("{before_padded}{glyph}{filler}{after}");
}

#[cfg(test)]
mod tests {
    use super::super::text::strip_terminal_sequences;
    use super::*;

    struct Block {
        lines: Vec<String>,
        height: u16,
    }
    impl Component for Block {
        fn render(&mut self, _width: u16) -> Vec<String> {
            let mut out = self.lines.clone();
            out.resize(self.height as usize, String::new());
            out
        }
    }

    fn block(lines: &[&str], height: u16) -> Box<dyn Component> {
        Box::new(Block {
            lines: lines.iter().map(|s| s.to_string()).collect(),
            height,
        })
    }

    #[test]
    fn vstack_allocates_grow_to_the_flexible_entry() {
        let mut node = LayoutNode::vstack(
            vec![
                StackEntry::leaf(LayoutNode::leaf(block(&["head"], 1))),
                StackEntry::grow(LayoutNode::leaf(block(&["body"], 0))),
                StackEntry::leaf(LayoutNode::leaf(block(&["foot"], 1))),
            ],
            0,
        );
        let frame = render_frame(&mut node, 20, 10);
        assert_eq!(frame.lines[0].trim_end(), "head");
        assert_eq!(frame.lines[9].trim_end(), "foot");
        assert_eq!(frame.lines.len(), 10);
    }

    #[test]
    fn hstack_aligns_children() {
        let mut node = LayoutNode::hstack(
            vec![
                StackEntry::leaf(LayoutNode::leaf(block(&["L"], 1))),
                StackEntry::leaf(LayoutNode::leaf(block(&["R"], 1))),
            ],
            1,
            Align::Start,
        );
        let frame = render_frame(&mut node, 10, 1);
        assert_eq!(strip_terminal_sequences(&frame.lines[0]).trim_end(), "L R");
    }

    #[test]
    fn scroll_clamps_and_paints_the_window() {
        let lines: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        let mut node = LayoutNode::scroll(
            Box::new(Block {
                lines: lines.clone(),
                height: 30,
            }),
            true,
        );
        if let LayoutNode::Scroll { state, .. } = &mut node {
            state.scroll_top = 18;
        }
        let frame = render_frame(&mut node, 10, 4);
        assert!(frame.had_primary_scroll);
        assert!(
            strip_terminal_sequences(&frame.lines[0])
                .trim_end()
                .starts_with("line 18")
        );
        assert!(visible_width(&frame.lines[0]) <= 10);
        // Clamp: scroll_top beyond content pulls back.
        let mut node = LayoutNode::scroll(block(&["only"], 1), false);
        if let LayoutNode::Scroll { state, .. } = &mut node {
            state.scroll_top = 99;
        }
        let frame = render_frame(&mut node, 10, 4);
        assert_eq!(strip_terminal_sequences(&frame.lines[0]), "only");
    }

    #[test]
    fn osc133_zone_prefixes_are_stripped_when_painted() {
        assert_eq!(strip_osc133("\x1b]133;A\x07hello"), "hello");
        assert_eq!(strip_osc133("keep\x1b]133;B\x07me"), "keepme");
    }

    #[test]
    fn scrollbar_never_splits_a_wide_char() {
        let mut screen = vec!["世世世世".to_string()];
        replace_cell(&mut screen, 2, 0, "█");
        assert_eq!(visible_width(&screen[0]), 8);
    }
}
