//! Overlay and dock composition over the engine's line strings.
//!
//! The transcript flows as line strings; the dock (notice + editor +
//! footer) sits below; a modal composites a centered box over the visible
//! viewport, and the side panel takes the right column. Extensions never
//! write escape sequences - their widget trees pass through
//! `widget_lines` (ADR-0003).

use lca_tui::engine::core::{SEGMENT_RESET, resolve_overlay_layout};

use crate::theme::{Role, Theme};
use lca_tui::engine::text::{
    extract_segments, linkify_urls, slice_by_column, visible_width, wrap_text_with_ansi,
};

/// A centered box drawn over `base`, which is the visible viewport.
///
/// The rectangle is resolved by the engine's overlay layout (anchor,
/// margin, clamping) so the compositor and the focus machine agree on
/// where an overlay lives.
pub fn overlay_box(
    base: &mut [String],
    width: u16,
    height: u16,
    title: &str,
    body: &[String],
    theme: &Theme,
) {
    overlay_box_selected(base, width, height, title, body, theme, None);
}

/// [`overlay_box`] with one body row marked selected: pi's selectors paint
/// the selected row in the `selectedBg` background with the accent text
/// (`theme.bg("selectedBg", …)` in `session-selector.ts` /
/// `tree-selector.ts`, `getSelectListTheme`'s `selectedText`).
///
/// `selected` is an index into `body` (the caller's rows, before any hint
/// rows the picker appends - pass `None` to paint nothing).
pub fn overlay_box_selected(
    base: &mut [String],
    width: u16,
    height: u16,
    title: &str,
    body: &[String],
    theme: &Theme,
    selected: Option<usize>,
) {
    let options = lca_tui::engine::core::OverlayOptions {
        width: Some(lca_tui::engine::core::SizeValue::Percent(80)),
        min_width: Some(24),
        max_height: Some(lca_tui::engine::core::SizeValue::Abs(height)),
        margin: 2,
        ..Default::default()
    };
    overlay_box_placed(base, width, height, title, body, theme, selected, &options);
}

/// The picker's layout options: one rule for the painter and the
/// mouse hit box, so they can never disagree (gh #167's comment
/// made flesh - the mouse builds these through this helper too).
pub fn picker_overlay_options(
    height: u16,
    above_rows: usize,
) -> lca_tui::engine::core::OverlayOptions {
    lca_tui::engine::core::OverlayOptions {
        width: Some(lca_tui::engine::core::SizeValue::Percent(80)),
        min_width: Some(24),
        max_height: Some(lca_tui::engine::core::SizeValue::Abs(height)),
        margin: 2,
        anchor: Some(lca_tui::engine::core::Anchor::BottomCenter),
        offset_y: -(i32::try_from(above_rows).unwrap_or(i32::MAX)),
        ..Default::default()
    }
}

/// The cells a picker's rows have to paint in, for a terminal `width`
/// (gh #394): the same overlay layout the painter resolves, minus the
/// frame's two border cells and two padding cells per side.
///
/// The tree navigator needs this because its rows clamp and pan
/// horizontally; deriving the number here keeps one layout rule behind
/// both the frame and the row arithmetic, so a row can never be painted
/// wider than the box that holds it.
pub fn picker_inner_width(term_width: u16) -> u16 {
    let options = picker_overlay_options(1, 0);
    let rect = resolve_overlay_layout(&options, term_width, 1, 1);
    rect.width.saturating_sub(4)
}

/// The rolling window over a picker's items (gh #226, pi's
/// `getVisibleRange` in `select-list.ts`): `selected` centered when
/// there is room, clamped at both ends. Pure, so the painter and the
/// mouse derive the same window from the same inputs.
pub fn picker_window(
    items_len: usize,
    selected: Option<usize>,
    max_visible: usize,
) -> (usize, usize) {
    let max_visible = max_visible.max(1);
    if items_len == 0 {
        return (0, 0);
    }
    let selected = selected.unwrap_or(0).min(items_len.saturating_sub(1));
    let half = max_visible / 2;
    let start = selected
        .saturating_sub(half)
        .min(items_len.saturating_sub(max_visible));
    let end = (start + max_visible).min(items_len);
    (start, end)
}

/// [`overlay_box_selected`] anchored above the composer (gh #16): the
/// box's bottom sits `above_rows` rows above the viewport bottom - pi's
/// bottom-anchored picker shape - so a tall notice is overlaid, never
/// stacked under.
///
/// Tall bodies roll (gh #226): the last two rows are the picker's
/// pinned hint rows, everything above them windows around `selected`,
/// and the frame always closes with scroll counts. Returns the painted
/// `(start, end, content_height)` the mouse hit test reads back.
///
/// The window is sticky (gh #230): when `keep` carries the painted
/// window and the selection sits inside it, the window holds -
/// hover highlights in place instead of recentering the list under
/// a stationary cursor. Wheel and keys rescroll by leaving it.
// The nine inputs are the picker's contract plus its anchor offset;
// a parameter struct would only rename them.
#[allow(clippy::too_many_arguments)]
pub fn overlay_box_picker(
    base: &mut [String],
    width: u16,
    height: u16,
    title: &str,
    body: &[String],
    theme: &Theme,
    selected: Option<usize>,
    above_rows: usize,
    keep: Option<(usize, usize)>,
    footer: &[String],
) -> (usize, usize, u16) {
    let options = picker_overlay_options(height, above_rows);
    // Gh #386 gap 3: whether there is room for a footer is decided **before** the
    // painter, by [`picker_footer_fits`], because only the caller can choose what to
    // put on the hint line instead. So the footer arriving here is one that already
    // fits, and `footer_rows` is simply it.
    let footer_rows = footer;
    let mut body: std::borrow::Cow<'_, [String]> = std::borrow::Cow::Borrowed(body);
    if !footer_rows.is_empty() {
        let mut extended = body.into_owned();
        // The footer brings its own separator (its first row), so the hint's blank
        // is replaced rather than doubled.
        let tail_start = extended.len().saturating_sub(2);
        let mut rebuilt: Vec<String> = extended.drain(..tail_start).collect();
        rebuilt.extend_from_slice(footer_rows);
        rebuilt.extend(extended);
        body = std::borrow::Cow::Owned(rebuilt);
    }
    let body = body.as_ref();
    // The granted height bounds the window: two frame rows plus the
    // two pinned hint rows, at least one item row visible.
    let content_height = u16::try_from(body.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let rect = resolve_overlay_layout(&options, width, height, content_height);
    let inner = (rect.width as usize).saturating_sub(4);
    // The last `2 + footer` rows are painter furniture (the hint and the footer) and
    // never scroll, so the item count stops before them.
    //
    // The `4` is the two frame rows plus the two hint rows. A footer adds to the
    // box's height by exactly as many rows as it adds here, and the surplus gate
    // above only lets it in when that height was free - so the two cancel and the
    // item budget needs no footer term. Adding one would be a second copy of a
    // subtraction the gate already made, which is how a budget drifts from its
    // reason.
    let pinned_tail = 2usize.saturating_add(footer_rows.len());
    let items_len = body.len().saturating_sub(pinned_tail);
    let max_items = (rect.height as usize).saturating_sub(4).max(1);
    let (mut start, mut end) = picker_window(items_len, selected, max_items);
    // gh #230: a selection inside the painted window keeps it. The
    // hit test only ever reports painted rows, so a hover always
    // lands inside and never rescrolls; keys and wheel leave it.
    if let Some((prev_start, prev_end)) = keep {
        let size = end.saturating_sub(start);
        let covers = selected.is_some_and(|at| at >= prev_start && at < prev_end);
        if prev_end.saturating_sub(prev_start) == size && prev_end <= items_len && covers {
            start = prev_start;
            end = prev_end;
        }
    }
    // Wrap the window's rows; the selected row fills first so it is
    // always visible, then the rows above and below split the rest.
    let wrap_row =
        |index: usize| -> Vec<String> { wrap_text_with_ansi(&linkify_urls(&body[index]), inner) };
    let mut visible: Vec<(usize, String)> = Vec::new();
    if items_len > 0 {
        let selected_row = selected
            .filter(|s| *s < items_len)
            .unwrap_or(start.min(items_len.saturating_sub(1)));
        // Wrap once per row; take lines around the selected row with
        // leftovers flowing both ways, so a short side never starves
        // the other. A partially shown row still maps (its owner rides
        // every line).
        let rows: Vec<Vec<String>> = (start..end).map(wrap_row).collect();
        let at = |index: usize| &rows[index - start];
        let mut selected_lines = at(selected_row).clone();
        let mut budget = max_items;
        if selected_lines.len() > budget {
            selected_lines.truncate(budget);
        }
        budget -= selected_lines.len();
        let above_lines: usize = (start..selected_row).map(|index| at(index).len()).sum();
        let below_lines: usize = ((selected_row + 1)..end).map(|index| at(index).len()).sum();
        let mut above_take = (above_lines).min(budget - (below_lines).min(budget / 2));
        let below_take = (below_lines).min(budget - above_take);
        above_take += (above_lines - above_take).min(budget - above_take - below_take);
        let mut above: Vec<(usize, String)> = Vec::new();
        let mut take = above_take;
        for index in (start..selected_row).rev() {
            for line in at(index).iter().rev() {
                if take == 0 {
                    break;
                }
                above.push((index, line.clone()));
                take -= 1;
            }
            if take == 0 {
                break;
            }
        }
        above.reverse();
        let mut below: Vec<(usize, String)> = Vec::new();
        let mut take = below_take;
        for index in (selected_row + 1)..end {
            for line in at(index) {
                if take == 0 {
                    break;
                }
                below.push((index, line.clone()));
                take -= 1;
            }
            if take == 0 {
                break;
            }
        }
        visible.extend(above);
        visible.extend(selected_lines.into_iter().map(|line| (selected_row, line)));
        visible.extend(below);
        if let Some((first, _)) = visible.first() {
            start = *first;
        }
        if let Some((last, _)) = visible.last() {
            end = last + 1;
        }
    }
    // The pinned tail never scrolls: items, then the pinned rows.
    let mut windowed: Vec<(usize, String)> = visible;
    for (offset, line) in body.iter().enumerate().skip(items_len) {
        windowed.push((offset, line.clone()));
    }
    let title = if start > 0 {
        format!("{title}  ▲ {start} more")
    } else {
        title.to_string()
    };
    let bottom_note = if end < items_len {
        Some(format!("▼ {} more", items_len - end))
    } else {
        None
    };
    // Re-resolve so the box hugs the windowed content; the anchor is
    // the same, so the mouse derives the identical rect.
    let windowed_height = u16::try_from(windowed.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let rect = resolve_overlay_layout(&options, width, height, windowed_height);
    overlay_box_placed_windowed(
        base,
        &rect,
        &title,
        &windowed,
        theme,
        selected,
        bottom_note.as_deref(),
    );
    (start, end, windowed_height)
}

/// The shared box painter behind [`overlay_box_selected`] and
/// [`overlay_box_picker`]: one layout rule per caller, one painter.
#[allow(clippy::too_many_arguments)]
fn overlay_box_placed(
    base: &mut [String],
    width: u16,
    height: u16,
    title: &str,
    body: &[String],
    theme: &Theme,
    selected: Option<usize>,
    options: &lca_tui::engine::core::OverlayOptions,
) {
    let content_height = u16::try_from(body.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let rect = resolve_overlay_layout(options, width, height, content_height);
    let w = rect.width as usize;
    let inner = w.saturating_sub(4);

    // A full frame: pi's selectors draw complete boxes (DynamicBorder),
    // and a left-only border reads as a half-drawn frame.
    let mut box_lines: Vec<(usize, String)> = Vec::new();
    for (index, line) in body.iter().enumerate() {
        // gh #178: a raw URL linkifies before the wrap, so every wrapped
        // segment re-opens the full link and clicks open the whole URL.
        for wrapped in wrap_text_with_ansi(&linkify_urls(line), inner) {
            let pad = inner.saturating_sub(visible_width(&wrapped));
            box_lines.push((index, format!("│ {wrapped}{} │", " ".repeat(pad))));
        }
    }
    overlay_box_placed_windowed(base, &rect, title, &box_lines, theme, selected, None);
}

/// Fit one frame caption (`title` or the scroll note) into the box
/// width, so an indicator never pushes the border off-screen.
fn fit_caption(text: &str, inner: usize) -> String {
    let plain = text.to_string();
    if visible_width(&plain) <= inner {
        return plain;
    }
    lca_tui::engine::text::truncate_to_width(&plain, inner, "…", false)
}

/// The shared painter: a closed frame around pre-wrapped `(owner,
/// line)` rows. `owner` is the body index for selection and mouse
/// mapping (`usize::MAX` never matches a selection).
fn overlay_box_placed_windowed(
    base: &mut [String],
    rect: &lca_tui::engine::core::Rect,
    title: &str,
    box_lines: &[(usize, String)],
    theme: &Theme,
    selected: Option<usize>,
    bottom_note: Option<&str>,
) {
    let w = rect.width as usize;
    let inner = w.saturating_sub(4);
    // Which body row each painted row belongs to (`usize::MAX` = frame).
    let mut owner: Vec<usize> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let title = fit_caption(title, inner);
    let title_w = visible_width(&title);
    lines.push(format!(
        "╭─ {title} {}╮",
        "─".repeat(w.saturating_sub(title_w + 5))
    ));
    owner.push(usize::MAX);
    for (index, line) in box_lines {
        lines.push(line.clone());
        owner.push(*index);
    }
    let bottom = match bottom_note {
        Some(note) => {
            let note = fit_caption(note, inner);
            let note_w = visible_width(&note);
            format!("╰─ {note} {}╯", "─".repeat(w.saturating_sub(note_w + 5)))
        }
        None => format!("╰{}╯", "─".repeat(w.saturating_sub(2))),
    };
    lines.push(bottom);
    owner.push(usize::MAX);

    let col = rect.col as usize;
    let top = rect.row as usize;
    // R4: the diff renderer writes only the spans that changed, so a cell
    // without an explicit style inherits whatever SGR the terminal last
    // saw - which is how transcript colors used to bleed into dialogs (and
    // dialog borders into the transcript beside them). Every dialog row is
    // therefore painted whole: a reset, the frame in the border role, a
    // reset, and then the untouched fragments on either side.
    let border = theme.role(Role::Border);
    let accent = theme.role(Role::Accent);
    let selected_bg = theme.bg(Role::SelectedBg);
    for (i, line) in lines.iter().enumerate() {
        let row = top + i;
        if let Some(base_line) = base.get_mut(row) {
            let before = slice_by_column(base_line, 0, col, false);
            let before = format!(
                "{before}{}",
                " ".repeat(col.saturating_sub(visible_width(&before)))
            );
            let after_start = col + visible_width(line);
            let after = slice_by_column(base_line, after_start, 10_000, false);
            let pad = " ".repeat(w.saturating_sub(visible_width(line)));
            let row_text = if selected == Some(owner[i]) {
                // Frame in the border role, the row's cells in accent on
                // the selected background - so the highlight stops at the
                // frame like pi's.
                let inner_span = format!("{line}{pad}");
                match inner_span
                    .strip_prefix("│ ")
                    .and_then(|s| s.strip_suffix(" │"))
                {
                    Some(cells) => format!(
                        "{}{}{}",
                        border("│ "),
                        selected_bg(&accent(cells)),
                        border(" │")
                    ),
                    None => border(&inner_span),
                }
            } else {
                border(&format!("{line}{pad}"))
            };
            *base_line = format!("{before}{SEGMENT_RESET}{row_text}{SEGMENT_RESET}{after}");
        }
    }
}

/// Splice an overlay segment into a base row at the seam (gh #238):
/// the one place the layer contract lives, so call sites cannot drift.
/// Entry reset opens the segment in clean state (no bleed in from the
/// base row); the segment pads to exactly `width` columns (no
/// transparency gaps); exit reset closes it (no bleed out); the right
/// slice re-arms the base row's own state through `pending_ansi`
/// (FR-UI-23's mechanism). `x` is the segment's start column.
///
/// **`x` is honoured on short rows** (gh #373 bug 1). `before` used to be
/// only `slice_by_column(base, 0, x)` with no pad, so a base row shorter
/// than `x` - which is most rows, since a row's *text* is usually shorter
/// than the column an overlay is placed at - put the segment right after the
/// text instead of at the column it was placed at. The drawer tab `◀` and the
/// `↓ Jump to latest message` indicator were both drawn wherever the line
/// happened to end.
///
/// The pad is the same three lines its two neighbours already used
/// ([`side_panel`] and the tooltip placement in `chat_render`), which is why
/// this is convergence rather than a third shape. Widening the row to reach
/// `x` is deliberate: an overlay must be where it was placed, and a row whose
/// text stopped early is a row with room after it.
///
/// The wide-character seams are handled by the slices themselves -
/// [`slice_by_column`] with `strict = false` keeps a grapheme that straddles a
/// boundary, which is why the *left* seam pads to the measured width of what
/// survived rather than to `x` blindly, and why the *right* slice starts after
/// what the segment actually drew.
pub fn splice_segment(base: &str, x: usize, width: usize, segment: &str) -> String {
    use lca_tui::engine::text::truncate_to_width;
    // Gh #373 bugs 1 and 2: both slices go through `extract_segments`, which
    // already has the two properties this seam needs and which two neighbours
    // (`side_panel`, the tooltip) were compensating for by hand.
    //
    // * `before` is **strict** by construction there (`current_col + w <=
    //   before_end`), so a 2-cell glyph straddling `x` is dropped rather than
    //   half-kept. With `slice_by_column(strict = false)` it survived, `before`
    //   measured one cell wider than `x`, and the segment shifted a column right
    //   (and the row overflowed).
    // * `after` re-arms through `AnsiCodeTracker`, which emits the **net** style
    //   entering the slice instead of every code seen before it - so a style the
    //   base row had already closed does not come back (bug 2).
    //
    // Both are `strict = false` for the *glyph* decision the brief calls for on
    // the right seam: taking the net style is independent of whether a trailing
    // glyph is kept.
    // `after_len` is "everything from here" - `extract_segments` treats `0` as
    // "nothing after", so the length must be a real (large) bound.
    let (before, _, after, _) =
        extract_segments(base, x, x.saturating_add(width), usize::MAX / 2, false);
    // Gh #373 bug 1: pad to `x`, measured in cells. `before` is now at most `x`
    // wide, so this fills the short-row case exactly.
    let before = format!(
        "{before}{}",
        " ".repeat(x.saturating_sub(visible_width(&before)))
    );
    let segment = truncate_to_width(segment, width, "", false);
    let segment = format!(
        "{segment}{}",
        " ".repeat(width.saturating_sub(visible_width(&segment)))
    );
    format!("{before}{SEGMENT_RESET}{segment}{SEGMENT_RESET}{after}")
}

/// Composite a side panel into the right column of every line (gh
/// #238: each row splices at the seam, so transcript bands cannot
/// bleed into the panel and panel rows always close cleanly).
/// The extension side panel (gh #207, tinted by gh #237): a `│`
/// border in [`Role::Border`] plus panel cells on a [`Role::SidePanel`]
/// tint that covers the transcript behind them - empty rows pad with
/// tinted spaces, so nothing bleeds through. One [`splice_segment`]
/// seam per row keeps the #238 reset hygiene.
pub fn side_panel(base: &mut [String], width: u16, panel: &[String], theme: &Theme) {
    let panel_w = panel_width(width);
    let col = (width as usize).saturating_sub(panel_w);
    let tint = theme.bg(Role::SidePanel);
    let edge = theme.role(Role::Border);
    for (i, base_line) in base.iter_mut().enumerate() {
        let text = panel.get(i).map(String::as_str).unwrap_or("");
        let content_w = panel_w.saturating_sub(2);
        let text = lca_tui::engine::text::truncate_to_width(text, content_w, "…", false);
        // Pad the base side first (the seam keeps the caller's
        // alignment), then splice the bordered, tinted panel span to
        // full width over the padded row.
        let before = slice_by_column(base_line, 0, col, false);
        let before = format!(
            "{before}{}",
            " ".repeat(col.saturating_sub(visible_width(&before)))
        );
        let padded = format!("{before}{}", " ".repeat(panel_w));
        let fill = content_w.saturating_sub(visible_width(&text));
        let span = tint(&format!("{} {text}{}", edge("│"), " ".repeat(fill)));
        *base_line = splice_segment(&padded, col, panel_w, &span);
    }
}

/// The extension panel's width (gh #207): one rule for the paint
/// and the drawer tab, so they can never disagree.
pub fn panel_width(width: u16) -> usize {
    40usize.min(width as usize / 2)
}

/// A tooltip's wrap width (gh #210): past this the text wraps.
pub const TOOLTIP_MAX_WIDTH: usize = 40;

/// Wrap tooltip text into lines (gh #210): manual newlines split, long
/// lines wrap at `max` columns.
pub fn tooltip_lines(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for part in text.split('\n') {
        let wrapped = lca_tui::engine::text::wrap_text_with_ansi(part, max.max(1));
        if wrapped.is_empty() {
            out.push(String::new());
        } else {
            out.extend(wrapped);
        }
    }
    out
}

/// Place a tooltip block (gh #210): below-right of the pointer when it
/// fits, flipping above/left past the edges, always clamped inside the
/// frame. `(W, H)` frame, `(mx, my)` pointer, `(tw, th)` block.
pub fn tooltip_place(w: u16, h: u16, mx: u16, my: u16, tw: usize, th: usize) -> (u16, u16) {
    let (w, h) = (w as usize, h as usize);
    let x = if (mx as usize) < w.saturating_sub(tw) {
        mx as usize + 1
    } else {
        (mx as usize).saturating_sub(tw)
    };
    let y = if (my as usize) < h.saturating_sub(th) {
        my as usize + 1
    } else {
        (my as usize).saturating_sub(th)
    };
    let x = x.min(w.saturating_sub(tw)).min(w.saturating_sub(1));
    let y = y.min(h.saturating_sub(th)).min(h.saturating_sub(1));
    (x as u16, y as u16)
}

/// Paint a borderless tooltip block (gh #210): each line hugs its text
/// in the selected-background role with text-role ink - no frames, no
/// padding rows.
pub fn paint_tooltip(base: &mut [String], x: u16, y: u16, lines: &[String], theme: &Theme) {
    use lca_tui::engine::text::{slice_by_column, visible_width};
    let bg = theme.bg(crate::theme::Role::SelectedBg);
    let ink = theme.role(crate::theme::Role::Text);
    for (offset, line) in lines.iter().enumerate() {
        let Some(row) = base.get_mut(y as usize + offset) else {
            continue;
        };
        let width = visible_width(line);
        if width == 0 {
            continue;
        }
        let before = slice_by_column(row, 0, x as usize, false);
        let before = format!(
            "{before}{}",
            " ".repeat((x as usize).saturating_sub(visible_width(&before)))
        );
        // gh #238: the tooltip splices at the seam, so the underlying
        // row cannot tint it and its background cannot leak right. The
        // padded row keeps the original tail past the splice point.
        let span = bg(&ink(line));
        let tail = slice_by_column(row, x as usize, 10_000, false);
        let padded = format!("{before}{tail}");
        *row = splice_segment(&padded, x as usize, width, &span);
    }
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;

#[cfg(test)]
mod picker_window_tests {
    use super::*;
    use lca_tui::engine::text::strip_terminal_sequences;

    // Verifies: gh #226 - the window centers `selected` with room,
    // clamps at both ends, and stays empty-safe.
    #[test]
    fn the_window_centers_and_clamps() {
        assert_eq!(picker_window(50, Some(25), 16), (17, 33));
        assert_eq!(picker_window(50, Some(2), 16), (0, 16));
        assert_eq!(picker_window(50, Some(49), 16), (34, 50));
        assert_eq!(picker_window(5, Some(4), 16), (0, 5));
        assert_eq!(picker_window(0, None, 16), (0, 0));
        assert_eq!(picker_window(50, None, 16), (0, 16));
    }

    fn tall_body(items: usize) -> Vec<String> {
        let mut body: Vec<String> = (0..items).map(|index| format!("item {index:02}")).collect();
        body.push(String::new());
        body.push("hint row".to_string());
        body
    }

    fn stripped(base: &[String]) -> Vec<String> {
        base.iter()
            .map(|line| strip_terminal_sequences(line))
            .collect()
    }

    // Verifies: gh #226 - a 50-item picker on an 80x24 pane draws a
    // closed frame (both borders), the hint row, scroll counts, and
    // the selected row highlighted.
    #[test]
    fn a_tall_picker_keeps_frame_hints_and_selection() {
        let mut base = vec![".".repeat(80); 24];
        let (start, end, _) = overlay_box_picker(
            &mut base,
            80,
            24,
            "models",
            &tall_body(50),
            &Theme::plain(),
            Some(25),
            3,
            None,
            &[],
        );
        assert!(
            start > 0 && end < 50,
            "windowed, not clipped: {start}..{end}"
        );
        let text = stripped(&base).join("\n");
        assert!(text.contains("╭─"), "top border draws");
        assert!(text.contains("▲"), "scroll-up indicator draws: {text}");
        assert!(text.contains("▼"), "scroll-down indicator draws: {text}");
        assert!(text.contains("╰─"), "bottom border draws");
        assert!(text.contains("hint row"), "the pinned hint shows");
        assert!(text.contains("item 25"), "the selected row shows");
    }

    // Verifies: gh #226 - at the top there is no upward indicator and
    // the first rows show; at the bottom the reverse.
    #[test]
    fn the_indicators_track_the_window_edges() {
        let mut base = vec![".".repeat(80); 24];
        overlay_box_picker(
            &mut base,
            80,
            24,
            "models",
            &tall_body(50),
            &Theme::plain(),
            Some(0),
            3,
            None,
            &[],
        );
        let text = stripped(&base).join("\n");
        assert!(!text.contains("▲"), "nothing above: {text}");
        assert!(text.contains("▼"), "plenty below");
        assert!(text.contains("item 00"), "starts at the top");

        let mut base = vec![".".repeat(80); 24];
        overlay_box_picker(
            &mut base,
            80,
            24,
            "models",
            &tall_body(50),
            &Theme::plain(),
            Some(49),
            3,
            None,
            &[],
        );
        let text = stripped(&base).join("\n");
        assert!(text.contains("▲"), "plenty above");
        assert!(!text.contains("▼"), "nothing below: {text}");
        assert!(text.contains("item 49"), "ends at the bottom");
    }

    // Verifies: gh #226 - a short picker paints whole with a plain
    // frame (no indicators, no window).
    #[test]
    fn a_short_picker_paints_whole() {
        let mut base = vec![".".repeat(80); 24];
        let (start, end, _) = overlay_box_picker(
            &mut base,
            80,
            24,
            "models",
            &tall_body(3),
            &Theme::plain(),
            Some(1),
            3,
            None,
            &[],
        );
        assert_eq!((start, end), (0, 3));
        let text = stripped(&base).join("\n");
        assert!(
            !text.contains("▲") && !text.contains("▼"),
            "no indicators: {text}"
        );
        assert!(text.contains("item 02"), "every row shows");
    }
}
