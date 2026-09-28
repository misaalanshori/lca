//! The engine core, ported from pi's `packages/tui/src/tui.ts`
//! (`pi-tui-re/src_re/tui-engine/tui.md`).
//!
//! The load-bearing idea is unchanged: everything renders to **strings**,
//! one per logical line, with styling embedded as ANSI. There is no cell
//! buffer. The engine treats lines as opaque except for width math.
//!
//! Deviation from pi (documented): overlay coordinates here are
//! viewport-relative (row from the top), not pi's bottom-anchored
//! main-screen coordinate system. LCA's chat layout composes its own
//! viewport, so viewport-relative overlays are simpler and sufficient;
//! `ponytail:` revisit if a bottom-anchored main-screen overlay is needed.

use super::text::{extract_segments, slice_with_width, visible_width};

/// The APC side channel a focused component emits at the cursor position
/// (pi-notes #1). The engine finds it, strips it, and moves the hardware
/// cursor there for IME candidate windows.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

/// Style reset plus OSC 8 hyperlink close, applied to every rendered line.
pub const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

/// A renderable component. `render` returns one string per logical line.
pub trait Component: Send {
    /// Render at `width` visible columns.
    fn render(&mut self, width: u16) -> Vec<String>;
    /// Handle a raw key sequence when focused. Returns true if consumed.
    fn handle_input(&mut self, _data: &str) -> bool {
        false
    }
    /// Handle a mouse event. Returns a result, or `None` if unhandled.
    fn handle_mouse(&mut self, _event: &MouseEvent) -> Option<MouseResult> {
        None
    }
    /// Whether this component opts in to Kitty key-release events.
    fn wants_key_release(&self) -> bool {
        false
    }
    /// Drop cached render state (theme change, resize).
    fn invalidate(&mut self) {}
}

/// A component that can take keyboard focus.
pub trait Focusable {
    /// Whether this component currently has focus.
    fn focused(&self) -> bool;
    /// Set focus state.
    fn set_focused(&mut self, focused: bool);
}

/// Mouse event kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    /// Button press.
    Press,
    /// Button release.
    Release,
    /// Pointer move (no button).
    Move,
    /// Pointer move with a button held.
    Drag,
    /// Synthesized click (press + release on one cell).
    Click,
    /// Wheel scroll.
    Wheel,
}

/// Mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Left button.
    Left,
    /// Middle button.
    Middle,
    /// Right button.
    Right,
    /// No button (move/wheel).
    None,
}

/// A normalized mouse event (pi's `TuiMouseEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    /// Event kind.
    pub kind: MouseKind,
    /// Button.
    pub button: MouseButton,
    /// Local x within the target component.
    pub x: u16,
    /// Local y within the target component.
    pub y: u16,
    /// Screen x.
    pub screen_x: u16,
    /// Screen y.
    pub screen_y: u16,
    /// Logical wheel lines (positive = down).
    pub wheel_delta: i32,
    /// Click count (1, 2, 3).
    pub click_count: u8,
    /// Shift held.
    pub shift: bool,
    /// Alt held.
    pub alt: bool,
    /// Ctrl held.
    pub ctrl: bool,
}

/// What a component did with a mouse event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseResult {
    /// Stop propagation.
    pub handled: bool,
    /// Route subsequent drag/release here.
    pub capture: bool,
    /// Take keyboard focus.
    pub focus: bool,
    /// Request a render (per-event-type default when unspecified).
    pub render: bool,
}

impl MouseResult {
    /// A default result for a press/click/drag/wheel (handled + render).
    pub fn handled() -> Self {
        Self {
            handled: true,
            render: true,
            ..Default::default()
        }
    }
}

/// A vertical stack of components (pi's `Container`).
#[derive(Default)]
pub struct Container {
    children: Vec<Box<dyn Component>>,
    heights: Vec<u16>,
    last_width: u16,
}

impl Container {
    /// An empty container.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a child.
    pub fn add(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
    }

    /// The children.
    pub fn children(&self) -> &[Box<dyn Component>] {
        &self.children
    }

    /// Mutable children.
    pub fn children_mut(&mut self) -> &mut [Box<dyn Component>] {
        &mut self.children
    }

    /// Remove every child.
    pub fn clear(&mut self) {
        self.children.clear();
    }

    /// The number of children.
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Whether there are no children.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Per-child heights from the last render (mouse hit-testing).
    pub fn heights(&self) -> &[u16] {
        &self.heights
    }
}

impl Component for Container {
    fn render(&mut self, width: u16) -> Vec<String> {
        if width != self.last_width {
            self.last_width = width;
        }
        let mut lines = Vec::new();
        self.heights.clear();
        for child in &mut self.children {
            let child_lines = child.render(width);
            self.heights.push(child_lines.len() as u16);
            lines.extend(child_lines);
        }
        lines
    }

    fn handle_input(&mut self, data: &str) -> bool {
        for child in self.children.iter_mut().rev() {
            if child.handle_input(data) {
                return true;
            }
        }
        false
    }

    fn handle_mouse(&mut self, event: &MouseEvent) -> Option<MouseResult> {
        let mut y = 0u16;
        for (i, child) in self.children.iter_mut().enumerate() {
            let h = self.heights.get(i).copied().unwrap_or(0);
            if event.y >= y && event.y < y + h {
                let mut local = *event;
                local.y = event.y - y;
                if let Some(result) = child.handle_mouse(&local) {
                    return Some(result);
                }
            }
            y += h;
        }
        None
    }

    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }
}

// =============================================================================
// Overlays
// =============================================================================

/// A size that is absolute or a percentage of the available space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeValue {
    /// Absolute columns/rows.
    Abs(u16),
    /// Percentage (0-100).
    Percent(u16),
}

impl SizeValue {
    fn resolve(self, base: u16) -> u16 {
        match self {
            SizeValue::Abs(v) => v,
            SizeValue::Percent(p) => ((base as u32 * p as u32) / 100) as u16,
        }
    }
}

/// A nine-point anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// Top-left.
    TopLeft,
    /// Top-center.
    TopCenter,
    /// Top-right.
    TopRight,
    /// Middle-left.
    MiddleLeft,
    /// Center.
    Center,
    /// Middle-right.
    MiddleRight,
    /// Bottom-left.
    BottomLeft,
    /// Bottom-center.
    BottomCenter,
    /// Bottom-right.
    BottomRight,
}

/// Overlay placement and sizing (pi's `OverlayOptions`, viewport-relative).
#[derive(Debug, Clone, Default)]
pub struct OverlayOptions {
    /// Fixed width.
    pub width: Option<SizeValue>,
    /// Minimum width.
    pub min_width: Option<u16>,
    /// Maximum height.
    pub max_height: Option<SizeValue>,
    /// Anchor (defaults to center).
    pub anchor: Option<Anchor>,
    /// Horizontal offset from the anchor.
    pub offset_x: i32,
    /// Vertical offset from the anchor.
    pub offset_y: i32,
    /// Absolute or percentage row (overrides the anchor).
    pub row: Option<SizeValue>,
    /// Absolute or percentage column (overrides the anchor).
    pub col: Option<SizeValue>,
    /// Margin from the viewport edges.
    pub margin: u16,
    /// A predicate; when false the overlay is hidden this frame.
    pub visible: bool,
    /// Do not capture keyboard focus.
    pub non_capturing: bool,
}

/// The resolved rectangle of an overlay in the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Row (from the top).
    pub row: u16,
    /// Column.
    pub col: u16,
    /// Width.
    pub width: u16,
    /// Height.
    pub height: u16,
}

/// Resolve an overlay's rectangle from its options and rendered height.
pub fn resolve_overlay_layout(
    options: &OverlayOptions,
    term_width: u16,
    term_height: u16,
    content_height: u16,
) -> Rect {
    let margin = options.margin;
    let avail_w = term_width.saturating_sub(margin * 2);
    let avail_h = term_height.saturating_sub(margin * 2);

    let mut width = options.width.map(|w| w.resolve(avail_w)).unwrap_or(avail_w);
    if let Some(min) = options.min_width {
        width = width.max(min);
    }
    width = width.min(avail_w).max(1);

    let mut height = content_height;
    if let Some(max) = options.max_height {
        height = height.min(max.resolve(avail_h));
    }
    height = height.min(avail_h).max(1);

    let anchor = options.anchor.unwrap_or(Anchor::Center);
    let (base_row, base_col) = match anchor {
        Anchor::TopLeft => (0, 0),
        Anchor::TopCenter => (0, (term_width - width) / 2),
        Anchor::TopRight => (0, term_width.saturating_sub(width)),
        Anchor::MiddleLeft => ((term_height - height) / 2, 0),
        Anchor::Center => ((term_height - height) / 2, (term_width - width) / 2),
        Anchor::MiddleRight => ((term_height - height) / 2, term_width.saturating_sub(width)),
        Anchor::BottomLeft => (term_height.saturating_sub(height), 0),
        Anchor::BottomCenter => (term_height.saturating_sub(height), (term_width - width) / 2),
        Anchor::BottomRight => (
            term_height.saturating_sub(height),
            term_width.saturating_sub(width),
        ),
    };

    let row = options
        .row
        .map(|r| r.resolve(term_height))
        .unwrap_or_else(|| {
            (base_row as i32 + options.offset_y).clamp(0, term_height as i32) as u16
        });
    let col = options
        .col
        .map(|c| c.resolve(term_width))
        .unwrap_or_else(|| (base_col as i32 + options.offset_x).clamp(0, term_width as i32) as u16);

    // Clamp into the margin box.
    let row = row
        .min(term_height.saturating_sub(height))
        .max(margin.min(term_height));
    let col = col
        .min(term_width.saturating_sub(width))
        .max(margin.min(term_width));
    Rect {
        row,
        col,
        width,
        height,
    }
}

/// One overlay entry.
pub struct Overlay {
    /// The overlay's component.
    pub component: Box<dyn Component>,
    /// Its options.
    pub options: OverlayOptions,
    /// Whether it is hidden (retained, not rendered).
    pub hidden: bool,
    /// Focus order (later = on top).
    pub focus_order: u64,
    /// The last rendered rect, for mouse re-dispatch.
    pub bounds: Option<Rect>,
}

/// Splice an overlay slice into a base line at `col`, preserving the styles
/// before and after (pi's `compositeTuiLine`).
pub fn composite_tui_line(base: &str, overlay: &str, col: u16, width: u16) -> String {
    let col = col as usize;
    let width = width as usize;
    let (before, _bw, after, _aw) = extract_segments(base, col, col + width, base.len(), false);
    let (slice, slice_w) = slice_with_width(overlay, 0, width, false);
    let padding = width.saturating_sub(slice_w);
    format!(
        "{before}{slice}{}{SEGMENT_RESET}{after}",
        " ".repeat(padding)
    )
}

/// Find the cursor position (row, col) from `CURSOR_MARKER` in rendered
/// lines, stripping the marker. Returns the (possibly modified) lines and
/// the position if present.
pub fn extract_cursor_position(lines: &[String]) -> (Vec<String>, Option<(u16, u16)>) {
    let mut position = None;
    let mut out = Vec::with_capacity(lines.len());
    for (row, line) in lines.iter().enumerate() {
        if let Some(idx) = line.find(CURSOR_MARKER) {
            let col = visible_width(&line[..idx]) as u16;
            position = Some((row as u16, col));
            out.push(line.replace(CURSOR_MARKER, ""));
        } else {
            out.push(line.clone());
        }
    }
    (out, position)
}

// =============================================================================
// Terminal capability replies (consumed before component input)
// =============================================================================

/// A reply the engine consumes before it reaches components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityReply {
    /// OSC 11 background color.
    BackgroundColor(super::colors::RgbColor),
    /// DEC color-scheme report.
    ColorScheme(super::colors::ColorScheme),
    /// Cell size (`CSI 6 ; h ; w t`).
    CellSize {
        /// Cell height in pixels.
        height: u16,
        /// Cell width in pixels.
        width: u16,
    },
}

/// Recognize and parse a capability reply, if the input is one.
pub fn parse_capability_reply(data: &str) -> Option<CapabilityReply> {
    if let Some(color) = super::colors::parse_osc11_background_color(data) {
        return Some(CapabilityReply::BackgroundColor(color));
    }
    if let Some(scheme) = super::colors::parse_terminal_color_scheme_report(data) {
        return Some(CapabilityReply::ColorScheme(scheme));
    }
    // Cell size: ESC [ 6 ; h ; w t
    if let Some(body) = data
        .strip_prefix("\x1b[6;")
        .and_then(|s| s.strip_suffix('t'))
    {
        let parts: Vec<&str> = body.split(';').collect();
        if parts.len() == 2
            && let (Ok(h), Ok(w)) = (parts[0].parse::<u16>(), parts[1].parse::<u16>())
        {
            return Some(CapabilityReply::CellSize {
                height: h,
                width: w,
            });
        }
    }
    None
}

/// Check the width invariant: no rendered line may exceed the terminal
/// width. Returns the offending `(row, width)` if any.
pub fn width_violation(lines: &[String], width: u16) -> Option<(usize, usize)> {
    for (row, line) in lines.iter().enumerate() {
        let w = visible_width(line);
        if w > width as usize {
            return Some((row, w));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lines(Vec<String>);
    impl Component for Lines {
        fn render(&mut self, _width: u16) -> Vec<String> {
            self.0.clone()
        }
    }

    #[test]
    fn container_stacks_children_and_records_heights() {
        let mut c = Container::new();
        c.add(Box::new(Lines(vec!["a".into(), "b".into()])));
        c.add(Box::new(Lines(vec!["c".into()])));
        assert_eq!(c.render(80), vec!["a", "b", "c"]);
        assert_eq!(c.heights(), &[2, 1]);
    }

    #[test]
    fn overlay_layout_resolves_anchor_and_clamps() {
        let opts = OverlayOptions {
            width: Some(SizeValue::Abs(20)),
            anchor: Some(Anchor::Center),
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&opts, 80, 24, 6);
        assert_eq!(rect.width, 20);
        assert_eq!(rect.height, 6);
        assert_eq!(rect.col, 30);
        assert_eq!(rect.row, 9);
        // Over-large content is clamped into the margin box.
        let opts = OverlayOptions {
            margin: 2,
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&opts, 20, 10, 40);
        assert!(rect.row + rect.height <= 10);
        assert!(rect.col + rect.width <= 20);
    }

    #[test]
    fn composite_line_splices_and_pads() {
        let base = "0123456789";
        let out = composite_tui_line(base, "AB", 3, 5);
        let stripped = super::super::text::strip_terminal_sequences(&out);
        assert_eq!(stripped, "012AB   89");
    }

    #[test]
    fn cursor_marker_is_extracted_and_stripped() {
        let lines = vec!["hello".to_string(), format!("ab{CURSOR_MARKER}cd")];
        let (out, pos) = extract_cursor_position(&lines);
        assert_eq!(pos, Some((1, 2)));
        assert_eq!(out[1], "abcd");
        assert!(!out[1].contains("pi:c"));
    }

    #[test]
    fn capability_replies_are_recognized() {
        assert_eq!(
            parse_capability_reply("\x1b]11;#000000\x07"),
            Some(CapabilityReply::BackgroundColor(
                super::super::colors::RgbColor { r: 0, g: 0, b: 0 }
            ))
        );
        assert_eq!(
            parse_capability_reply("\x1b[?997;2n"),
            Some(CapabilityReply::ColorScheme(
                super::super::colors::ColorScheme::Light
            ))
        );
        assert_eq!(
            parse_capability_reply("\x1b[6;16;8t"),
            Some(CapabilityReply::CellSize {
                height: 16,
                width: 8
            })
        );
        assert_eq!(parse_capability_reply("\x1b[A"), None);
    }

    #[test]
    fn width_violation_detects_overlong_lines() {
        let lines = vec!["ok".to_string(), "this is too long".to_string()];
        assert_eq!(width_violation(&lines, 5), Some((1, 16)));
        assert_eq!(width_violation(&lines, 80), None);
    }
}
