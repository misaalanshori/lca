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
//! Not ported (R11): pi's `Container` component tree and its
//! `MouseResult`/`Focusable` dispatch. The interface composes line
//! strings directly and the renderer owns selection
//! (`engine/alt_screen.rs`), so the tree had no caller; it was deleted
//! rather than left as dead machinery. The normalized `MouseEvent` and
//! the `MouseRegion`/`MouseRegistry` hit-test table returned for gh
//! #164 (keystone only - no tree, no focus model). The `Component`
//! trait survives because the primitive widgets implement it.

use super::text::visible_width;

/// The APC side channel a focused component emits at the cursor position
/// (pi-notes #1). The engine finds it, strips it, and moves the hardware
/// cursor there for IME candidate windows.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

/// Full line-end reset: resets all text styles and foreground color,
/// plus OSC 8 hyperlink close, so unclosed spans never leak across line ends or repaints.
pub const FULL_LINE_RESET: &str = "\x1b[22;23;24;25;27;28;29;39m\x1b]8;;\x07";

/// Style reset plus OSC 8 hyperlink close, applied to every rendered line.
pub const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

/// Apply line resets to rendered line strings so that open text styles, colors,
/// or hyperlinks never leak into subsequent lines or terminal rows.
pub fn apply_line_resets(lines: &mut [String]) {
    for line in lines.iter_mut() {
        if !line.is_empty() {
            line.push_str(FULL_LINE_RESET);
        }
    }
}

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
            SizeValue::Percent(p) => ((u32::from(base) * u32::from(p.min(100))) / 100) as u16,
        }
    }

    /// The share of `span` this value names, for a **position** rather than a size.
    ///
    /// A percentage position distributes the ring between an overlay and the two
    /// margins, so the span it is a percentage of is the *slack* (`avail - size`),
    /// never the whole viewport.
    ///
    /// Values above 100 saturate. That is belt-and-braces, not the guarantee: the clamp
    /// in [`resolve_axis`] is what a caller can observe, and it already holds an
    /// out-of-range share at the far margin. Saturating here keeps the arithmetic honest
    /// for a caller that resolves a position without the clamp.
    fn share(self, span: u16) -> u16 {
        match self {
            SizeValue::Abs(v) => v,
            SizeValue::Percent(p) => ((u32::from(span) * u32::from(p.min(100))) / 100) as u16,
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

/// A callback asking whether an overlay should be shown at a terminal size.
///
/// The predicate is what makes a *responsive* overlay possible, and it is a function
/// rather than a `bool` for the reason the feature exists: owners **cache**
/// `OverlayOptions`, so a boolean computed once is stale the moment the terminal
/// changes size, and an owner that never rebuilds its options would never update it.
/// Asking on every resolve is the whole point.
///
/// Borrowed rather than boxed so an option set stays `Copy` for a closure that has no
/// captures; a capturing closure needs `&|..|`-style borrowing at the call site and
/// cannot outlive the options value.
pub type VisibleFn<'a> = &'a dyn Fn(u16, u16) -> bool;

/// Overlay placement and sizing (pi's `OverlayOptions`, viewport-relative).
#[derive(Clone, Default)]
pub struct OverlayOptions<'a> {
    /// Fixed width.
    pub width: Option<SizeValue>,
    /// Minimum width.
    pub min_width: Option<u16>,
    /// Maximum width: a ceiling on the resolved width (gh #373 gap 6).
    pub max_width: Option<SizeValue>,
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
    /// Asked on every resolve whether the overlay should be shown at this size.
    pub visible: Option<VisibleFn<'a>>,
    /// Do not capture keyboard focus.
    pub non_capturing: bool,
}

impl std::fmt::Debug for OverlayOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayOptions")
            .field("width", &self.width)
            .field("min_width", &self.min_width)
            .field("max_width", &self.max_width)
            .field("max_height", &self.max_height)
            .field("anchor", &self.anchor)
            .field("offset_x", &self.offset_x)
            .field("offset_y", &self.offset_y)
            .field("row", &self.row)
            .field("col", &self.col)
            .field("margin", &self.margin)
            .field("visible", &self.visible.map(|_| "<predicate>"))
            .field("non_capturing", &self.non_capturing)
            .finish()
    }
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
    /// Whether the overlay should paint and own input at this size (gh #373 gap 6).
    ///
    /// Carried on the rect rather than returned beside it because every caller already
    /// has the rect, and the focus decision is made by a different layer than the paint
    /// (`crates/lca-ui`'s `keyboard_owner`). One value that both layers read is what
    /// stops a hidden overlay from owning the keyboard; a second call with the same
    /// options would be a second answer waiting to disagree.
    pub visible: bool,
}

/// Which mouse button an event names (gh #164, pi's `TuiMouseButton`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// SGR button bits 0.
    Left,
    /// SGR button bits 1.
    Middle,
    /// SGR button bits 2.
    Right,
    /// No button: a hover, a release, or an unrecognized bit pattern.
    None,
}

impl MouseButton {
    /// Decode SGR's low two button bits.
    pub fn from_sgr_bits(bits: u16) -> Self {
        match bits & 3 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::None,
        }
    }
}

/// The modifier keys riding on a mouse event (gh #164): SGR value 4 is
/// shift, 8 is alt/meta, 16 is ctrl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Whether shift was held.
    pub shift: bool,
    /// Whether alt/meta was held.
    pub alt: bool,
    /// Whether ctrl was held.
    pub ctrl: bool,
}

impl Modifiers {
    /// Decode SGR's modifier bits.
    pub fn from_sgr_bits(bits: u16) -> Self {
        Modifiers {
            shift: bits & 4 != 0,
            alt: bits & 8 != 0,
            ctrl: bits & 16 != 0,
        }
    }
}

/// A normalized cell-based mouse event (gh #164, pi's `TuiMouseEvent`
/// without the component tree: R11 deleted the tree as dead machinery,
/// and only the event half returns). Coordinates are zero-based cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
    /// A button press.
    Down {
        /// Zero-based column.
        col: u16,
        /// Zero-based row.
        row: u16,
        /// The pressed button.
        button: MouseButton,
        /// Held modifiers.
        modifiers: Modifiers,
    },
    /// A button release.
    Up {
        /// Zero-based column.
        col: u16,
        /// Zero-based row.
        row: u16,
        /// The released button (`None` when the terminal reports 3).
        button: MouseButton,
        /// Held modifiers.
        modifiers: Modifiers,
    },
    /// Pointer motion: a drag when a button is held, a hover (pi's
    /// 1003 any-motion) when none is.
    Move {
        /// Zero-based column.
        col: u16,
        /// Zero-based row.
        row: u16,
        /// The held button, or `None` for a hover.
        button: Option<MouseButton>,
        /// Held modifiers.
        modifiers: Modifiers,
    },
    /// A wheel tick: negative scrolls up, positive scrolls down.
    Wheel {
        /// Zero-based column.
        col: u16,
        /// Zero-based row.
        row: u16,
        /// -1 for up, +1 for down.
        delta: i8,
        /// Held modifiers.
        modifiers: Modifiers,
    },
}

impl MouseEvent {
    /// Normalize one parsed SGR report: SGR coordinates are 1-based,
    /// bit 64 is the wheel, bit 32 is motion, the `M`/`m` suffix is the
    /// press/release edge.
    pub fn from_sgr(bits: u16, x: u16, y: u16, press: bool) -> Self {
        let col = x.saturating_sub(1);
        let row = y.saturating_sub(1);
        let modifiers = Modifiers::from_sgr_bits(bits);
        if bits & 64 != 0 {
            let delta = if bits & 3 == 0 { -1 } else { 1 };
            return MouseEvent::Wheel {
                col,
                row,
                delta,
                modifiers,
            };
        }
        if bits & 32 != 0 {
            let held = match MouseButton::from_sgr_bits(bits) {
                MouseButton::None => None,
                held => Some(held),
            };
            return MouseEvent::Move {
                col,
                row,
                button: held,
                modifiers,
            };
        }
        let button = MouseButton::from_sgr_bits(bits);
        if press {
            MouseEvent::Down {
                col,
                row,
                button,
                modifiers,
            }
        } else {
            MouseEvent::Up {
                col,
                row,
                button,
                modifiers,
            }
        }
    }
}

/// An interactive rectangle a frame registers for hit-testing (gh
/// #164, pi's `MouseRegion` without the wrapped component: LCA
/// composes line strings, so regions are values the dispatcher
/// queries, not wrappers around a tree).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseRegion {
    /// The viewport rectangle.
    pub rect: Rect,
    /// Who owns a hit inside it.
    pub target: &'static str,
}

/// The per-dispatch region table (gh #164): frames register their
/// interactive rectangles, and a hit resolves to the most recently
/// registered (topmost) region covering the cell.
#[derive(Debug, Default)]
pub struct MouseRegistry {
    regions: Vec<MouseRegion>,
}

impl MouseRegistry {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one interactive rectangle for this dispatch.
    pub fn register(&mut self, rect: Rect, target: &'static str) {
        self.regions.push(MouseRegion { rect, target });
    }

    /// The topmost target covering `(col, row)`, if any.
    pub fn hit(&self, col: u16, row: u16) -> Option<&'static str> {
        self.regions
            .iter()
            .rev()
            .find(|region| {
                col >= region.rect.col
                    && col < region.rect.col.saturating_add(region.rect.width)
                    && row >= region.rect.row
                    && row < region.rect.row.saturating_add(region.rect.height)
            })
            .map(|region| region.target)
    }
}

/// Resolve an overlay's rectangle from its options and rendered height.
///
/// **A percentage positions the overlay, not its corner.** `Percent(0)` puts the
/// overlay's near edge on the margin, `Percent(100)` puts its far edge on the
/// opposite margin, and `Percent(50)` leaves equal gaps on both sides: the
/// percentage is the share of the *slack*, `avail - size`, that falls before the
/// overlay (`margin + share(avail - size)`, pi's `resolveOverlayLayout`).
///
/// The corner-relative reading of the same number — `term * p / 100` — is exactly
/// what `Percent(50)` used to do here, and it is why a half-percent modal sat with
/// its top-left corner at the middle of the screen instead of centred.
///
/// Resolution order, pinned rather than left implicit because two controls asking the
/// same question is how this bug gets filed again: an explicit `row`/`col` wins over
/// the `anchor`, and the anchor still positions the axis the explicit value leaves
/// unset. `offset_x`/`offset_y` apply on top of either, as in pi.
///
/// The clamp into the margin box runs **after** the position is resolved, and it is
/// what keeps an offset (or an absolute `row` larger than the screen) inside. It is not
/// load-bearing for percentages any more: with the slack-based form the far edge lands
/// on the margin by construction, so `Percent(100)` no longer relies on the clamp to
/// recover the box it used to push past the right edge.
///
/// An `anchor` stays **viewport-relative**, not margin-relative (the deviation from pi
/// named in this module's header). LCA's own layout composes the viewport, and the
/// picker places itself by counting rows up from the bottom edge; insetting every
/// anchor by the margin would move it two rows off the composer it is pinned above.
/// The margin is a floor and a ceiling on a *percentage*'s placement, and the clamp on
/// everything else.
pub fn resolve_overlay_layout(
    options: &OverlayOptions,
    term_width: u16,
    term_height: u16,
    content_height: u16,
) -> Rect {
    // Margin adapts when viewport is constrained so dialogs remain visible.
    let margin_w = options.margin.min(term_width.saturating_sub(4) / 2);
    let margin_h = options.margin.min(term_height.saturating_sub(4) / 2);
    let avail_w = term_width.saturating_sub(margin_w * 2).max(1);
    let avail_h = term_height.saturating_sub(margin_h * 2).max(1);

    let mut width = options.width.map(|w| w.resolve(avail_w)).unwrap_or(avail_w);
    // The ceiling is applied to the *resolved* width and **before** the position chain,
    // so a narrower box is centred as the box it will be painted as. Applied after the
    // position, the column would be computed from the unclamped width and the overlay
    // would sit off-centre by half the overhang - a silent truncation, which is the
    // shape this option exists to avoid rather than to introduce.
    if let Some(max) = options.max_width {
        width = width.min(max.resolve(avail_w));
    }
    // A floor is applied **after** the ceiling, so `min_width` wins a conflict. That is
    // CSS's resolution (a `max-width` smaller than `min-width` is ignored) and it is the
    // right one here: a minimum is a readability guarantee the caller asked for, while a
    // ceiling is a preference. What a floor may not do is exceed the screen, and the
    // margin-box bound below is what enforces that - the same bound it had before this
    // option existed, so a narrow pane never regresses.
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
    let (anchor_row, anchor_col) = match anchor {
        Anchor::TopLeft => (0, 0),
        Anchor::TopCenter => (0, (term_width.saturating_sub(width)) / 2),
        Anchor::TopRight => (0, term_width.saturating_sub(width)),
        Anchor::MiddleLeft => ((term_height.saturating_sub(height)) / 2, 0),
        Anchor::Center => (
            (term_height.saturating_sub(height)) / 2,
            (term_width.saturating_sub(width)) / 2,
        ),
        Anchor::MiddleRight => (
            (term_height.saturating_sub(height)) / 2,
            term_width.saturating_sub(width),
        ),
        Anchor::BottomLeft => (term_height.saturating_sub(height), 0),
        Anchor::BottomCenter => (
            term_height.saturating_sub(height),
            (term_width.saturating_sub(width)) / 2,
        ),
        Anchor::BottomRight => (
            term_height.saturating_sub(height),
            term_width.saturating_sub(width),
        ),
    };

    let row = resolve_axis(
        options.row,
        anchor_row,
        options.offset_y,
        margin_h,
        avail_h,
        height,
        term_height,
    );
    let col = resolve_axis(
        options.col,
        anchor_col,
        options.offset_x,
        margin_w,
        avail_w,
        width,
        term_width,
    );
    Rect {
        row,
        col,
        width,
        height,
        visible: options
            .visible
            .is_none_or(|seen| seen(term_width, term_height)),
    }
}

/// One axis of the overlay's position, then clamped into the margin box.
///
/// An absolute value is a screen coordinate, as in pi. A percentage is the share of
/// the slack between the margins that falls *before* the overlay, so it positions the
/// overlay rather than its corner. The anchor is viewport-relative and is the fallback;
/// the offset applies to whichever of the three won.
fn resolve_axis(
    value: Option<SizeValue>,
    anchor_at: u16,
    offset: i32,
    margin: u16,
    avail: u16,
    size: u16,
    limit: u16,
) -> u16 {
    let base = match value {
        Some(SizeValue::Abs(abs)) => i32::from(abs),
        Some(percent) => i32::from(margin).saturating_add(i32::from(percent.share(avail - size))),
        None => i32::from(anchor_at),
    };
    let placed = base.saturating_add(offset);
    // The clamp sits here, after the position is resolved, and it is unchanged in
    // reach: the far edge may reach the terminal's own edge, and the near edge may not
    // pass the margin. It is deliberately **not** tightened to the far margin, because
    // a bottom-anchored picker places itself by counting rows up from the bottom edge
    // and would move by the margin if the bound moved. A percentage does not need the
    // tightening: `share(avail - size)` cannot exceed the slack, so `Percent(100)` lands
    // on the far margin by construction.
    //
    // Every term is subtract-only, so a size larger than the viewport (reachable with an
    // absolute width) falls back to the near margin instead of wrapping the bound around.
    let high = limit.saturating_sub(size).max(margin);
    placed.clamp(i32::from(margin), i32::from(high)) as u16
}

/// Find the cursor position (row, col) from `CURSOR_MARKER` in rendered
/// lines, stripping the marker. Returns the (possibly modified) lines and
/// the position if present, plus whether the component painted its caret
/// (a reverse-video cell) right after the marker: painted means the
/// hardware cursor stays hidden, unpainted means the hardware cursor *is*
/// the caret (V2 - pi's `showHardwareCursor` default, honored per row).
pub fn extract_cursor_position(lines: &[String]) -> (Vec<String>, Option<(u16, u16, bool)>) {
    let mut position = None;
    let mut out = Vec::with_capacity(lines.len());
    for (row, line) in lines.iter().enumerate() {
        if let Some(idx) = line.find(CURSOR_MARKER) {
            let col = u16::try_from(visible_width(&line[..idx])).unwrap_or(u16::MAX);
            let painted = line
                .get(idx + CURSOR_MARKER.len()..)
                .is_some_and(|rest| rest.starts_with("\x1b[7m"));
            position = Some((u16::try_from(row).unwrap_or(u16::MAX), col, painted));
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
#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "core_mouse_tests.rs"]
mod mouse_tests;
