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
//!
//! Not ported (R11): pi's `Container` component tree and its normalized
//! mouse dispatch (`MouseEvent`/`MouseResult`/`Focusable`). The interface
//! composes line strings directly and the renderer owns selection
//! (`engine/alt_screen.rs`), so the tree had no caller; it was deleted
//! rather than left as dead machinery. The `Component` trait survives
//! because the primitive widgets implement it.

use super::text::visible_width;

/// The APC side channel a focused component emits at the cursor position
/// (pi-notes #1). The engine finds it, strips it, and moves the hardware
/// cursor there for IME candidate windows.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

/// Style reset plus OSC 8 hyperlink close, applied to every rendered line.
pub const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

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
