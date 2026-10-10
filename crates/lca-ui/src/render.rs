//! Overlay and dock composition over the engine's line strings.
//!
//! The transcript flows as line strings; the dock (notice + editor +
//! footer) sits below; a modal composites a centered box over the visible
//! viewport, and the side panel takes the right column. Extensions never
//! write escape sequences - their widget trees pass through
//! `widget_lines` (ADR-0003).

use lca_tui::engine::core::{SEGMENT_RESET, resolve_overlay_layout};

use crate::theme::{Role, Theme};
use lca_tui::engine::text::{linkify_urls, slice_by_column, visible_width, wrap_text_with_ansi};

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
) -> (usize, usize, u16) {
    let options = picker_overlay_options(height, above_rows);
    // The granted height bounds the window: two frame rows plus the
    // two pinned hint rows, at least one item row visible.
    let content_height = u16::try_from(body.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let rect = resolve_overlay_layout(&options, width, height, content_height);
    let inner = (rect.width as usize).saturating_sub(4);
    let items_len = body.len().saturating_sub(2);
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
    // The hint rows never scroll: items, then the pinned tail.
    let mut windowed: Vec<(usize, String)> = visible;
    for (offset, line) in body.iter().enumerate().skip(items_len).take(2) {
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
pub fn splice_segment(base: &str, x: usize, width: usize, segment: &str) -> String {
    use lca_tui::engine::text::truncate_to_width;
    let before = slice_by_column(base, 0, x, false);
    let segment = truncate_to_width(segment, width, "", false);
    let segment = format!(
        "{segment}{}",
        " ".repeat(width.saturating_sub(visible_width(&segment)))
    );
    let after = slice_by_column(base, x.saturating_add(width), 10_000, false);
    format!("{before}{SEGMENT_RESET}{segment}{SEGMENT_RESET}{after}")
}

/// Composite a side panel into the right column of every line (gh
/// #238: each row splices at the seam, so transcript bands cannot
/// bleed into the panel and panel rows always close cleanly).
pub fn side_panel(base: &mut [String], width: u16, panel: &[String]) {
    let panel_w = panel_width(width);
    for (i, base_line) in base.iter_mut().enumerate() {
        let text = panel.get(i).map(String::as_str).unwrap_or("");
        let text = lca_tui::engine::text::truncate_to_width(text, panel_w, "…", false);
        let col = (width as usize).saturating_sub(panel_w);
        // Pad the base side first (the seam keeps the caller's
        // alignment), then splice the panel text to full width over
        // the padded row.
        let before = slice_by_column(base_line, 0, col, false);
        let before = format!(
            "{before}{}",
            " ".repeat(col.saturating_sub(visible_width(&before)))
        );
        let padded = format!("{before}{text}");
        *base_line = splice_segment(&padded, col, panel_w, &text);
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
mod tests {
    use super::*;
    use lca_tui::engine::text::strip_terminal_sequences;

    #[test]
    fn overlay_box_centers_over_the_viewport() {
        let mut base = vec![".".repeat(80); 24];
        overlay_box(
            &mut base,
            80,
            24,
            "login",
            &["hello".to_string()],
            &Theme::plain(),
        );
        let joined: String = base
            .iter()
            .map(|l| strip_terminal_sequences(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("hello"));
        assert!(joined.contains("login"));
    }

    // Verifies: gh #238 - the seam isolates SGR state both ways:
    // base styles never enter the segment, segment styles never leave
    // it, and the right slice re-arms the base state.
    #[test]
    fn splice_segment_isolates_sgr_state_both_directions() {
        use lca_tui::engine::core::SEGMENT_RESET;
        // Base row opens italic and never closes it; the segment opens
        // bold and never closes it.
        let base = "aa\x1b[3mbbccdd".to_string();
        let row = splice_segment(&base, 2, 2, "\x1b[1mXX");
        let reset = SEGMENT_RESET;
        assert_eq!(
            row,
            format!("aa{reset}\x1b[1mXX{reset}\x1b[3mccdd"),
            "byte-exact splice: {row:?}"
        );
        // The segment's bold cannot reach the right slice...
        let after_bold = row.find("\x1b[1m").expect("segment style");
        let exit = row.find(reset).expect("entry reset");
        let second =
            row[exit + reset.len()..].find(reset).expect("exit reset") + exit + reset.len();
        assert!(
            after_bold > exit && after_bold < second,
            "bold inside the segment"
        );
        // ...and the base row's italic re-arms after the exit reset
        // (pending_ansi carries it into the right slice).
        assert!(
            row[second + reset.len()..].contains("\x1b[3m"),
            "base state restored"
        );
    }

    // Verifies: gh #238 - short segments pad to full width (no
    // transparency gaps) and long ones clip without an ellipsis.
    #[test]
    fn splice_segment_pads_and_clips_to_exact_width() {
        use lca_tui::engine::text::visible_width;
        let row = splice_segment("0123456789", 3, 4, "AB");
        assert_eq!(visible_width(&row), 10);
        let stripped: String = row.chars().filter(|c| *c != '\x1b').collect();
        assert!(stripped.contains("AB  "), "padded: {stripped:?}");
        let row = splice_segment("0123456789", 2, 3, "ABCDEFG");
        assert_eq!(visible_width(&row), 10, "clipped, width kept");
    }

    // Verifies: gh #238 - the side panel splices at the seam: a
    // transcript background band cannot tint panel cells, the panel
    // always closes, and rows stay exactly `width` columns wide.
    #[test]
    fn side_panel_isolates_sgr_state_and_keeps_width() {
        use lca_tui::engine::core::SEGMENT_RESET;
        use lca_tui::engine::text::visible_width;
        // Base row opens a background band and never closes it.
        let mut base = vec![format!("xx\x1b[48;2;1;2;3m{}", "y".repeat(78)); 3];
        side_panel(&mut base, 80, &["panel".to_string()]);
        for line in &base {
            assert_eq!(visible_width(line), 80, "exact width: {line:?}");
            assert_eq!(
                line.matches(SEGMENT_RESET).count(),
                2,
                "open and close: {line:?}"
            );
        }
        let reset = SEGMENT_RESET;
        let first = &base[0];
        // The band code appears only before the entry reset...
        let entry = first.find(reset).expect("entry");
        assert!(
            first[..entry].contains("48;2;1;2;3"),
            "base state left of the seam"
        );
        // ...never inside the panel cells.
        let exit = first[entry + reset.len()..].find(reset).expect("exit") + entry + reset.len();
        assert!(
            !first[entry + reset.len()..exit].contains("48;2;1;2;3"),
            "no band in the panel: {first:?}"
        );
        assert!(first.contains("panel"), "content survives");
    }

    // Verifies: gh #238 - the tooltip splices at the seam: the row
    // beneath cannot tint it and its background cannot leak right.
    #[test]
    fn tooltip_isolates_sgr_state_both_directions() {
        use lca_tui::engine::core::SEGMENT_RESET;
        let theme = Theme::plain();
        let mut base = vec!["aa\x1b[3mbbccdd".to_string()];
        paint_tooltip(&mut base, 2, 0, &["\x1b[1mXX".to_string()], &theme);
        let reset = SEGMENT_RESET;
        let row = &base[0];
        // Entry reset, bold segment, exit reset, then the base row's
        // own italic re-armed over the remaining cells.
        assert!(
            row.contains(&format!("aa{reset}")),
            "entry reset after the left slice: {row:?}"
        );
        assert!(row.contains("\x1b[1mXX"), "segment content survives");
        let exit = row.find(&format!("XX{reset}")).expect("exit reset");
        assert!(
            row[exit..].contains("\x1b[3m"),
            "base state restored right of the seam: {row:?}"
        );
    }

    #[test]
    fn side_panel_takes_the_right_column() {
        let mut base = vec!["x".repeat(80); 3];
        side_panel(&mut base, 80, &["panel".to_string()]);
        let line = strip_terminal_sequences(&base[0]);
        // Rows close with a reset now (gh #238): the content still
        // lands at the right column, terminated, not dangling.
        assert!(line.contains("panel"), "{line:?}");
        assert!(line.trim_end().ends_with("panel"), "{line:?}");
    }

    // Verifies: R11 - the overlay box is a full frame, not a half one.
    #[test]
    fn the_overlay_box_draws_a_full_frame() {
        let mut base = vec![".".repeat(80); 24];
        overlay_box(
            &mut base,
            80,
            24,
            "login",
            &["hello".to_string()],
            &Theme::plain(),
        );
        let joined: Vec<String> = base.iter().map(|l| strip_terminal_sequences(l)).collect();
        assert!(
            joined.iter().any(|l| l.contains('╭') && l.contains('╮')),
            "a top frame with both corners"
        );
        assert!(
            joined.iter().any(|l| l.contains('╰') && l.contains('╯')),
            "a bottom frame with both corners"
        );
        let content = joined
            .iter()
            .find(|l| l.contains("hello"))
            .expect("a content line");
        let after = content.split("hello").nth(1).unwrap_or("");
        assert!(
            after.contains('│'),
            "a right border after the content: {content:?}"
        );
    }

    // Verifies: FR-UI-23 (R4) - every dialog row carries explicit styles:
    // a reset, the frame in the border role, a reset - so a transcript
    // color behind the dialog cannot bleed in, and the border cannot bleed
    // into the transcript beside it.
    #[test]
    fn dialog_rows_override_the_styles_around_them() {
        let theme = Theme::colored();
        let border = theme.role(Role::Border);
        // A base line painted with a strong color, as the transcript's
        // markdown and tool cards are.
        let painted = format!("\x1b[41m{}\x1b[0m", ".".repeat(80));
        let mut base = vec![painted.clone(); 24];
        overlay_box(&mut base, 80, 24, "login", &["hello".to_string()], &theme);
        let row = base
            .iter()
            .find(|line| line.contains("hello"))
            .expect("the dialog row rendered");

        let reset_at = row.find(SEGMENT_RESET).expect("a reset before the frame");
        let after_reset = &row[reset_at + SEGMENT_RESET.len()..];
        assert!(
            strip_terminal_sequences(after_reset).starts_with("│ hello"),
            "the frame starts right after the reset: {row:?}"
        );
        let border_sgr = border("x");
        let prefix = border_sgr.split('x').next().unwrap_or("");
        assert!(
            !prefix.is_empty() && row.contains(prefix),
            "the frame carries the border role's SGR: {row:?}"
        );
        assert!(
            row.matches(SEGMENT_RESET).count() >= 2,
            "a reset on both sides of the dialog text: {row:?}"
        );
    }

    // Verifies: FR-UI-23 (R4) - the base fragments keep their own styling:
    // the transcript to the left of a dialog keeps the color it had.
    #[test]
    fn the_fragments_around_a_dialog_keep_their_own_bytes() {
        let theme = Theme::colored();
        let left = format!("\x1b[42m{}\x1b[0m", "L".repeat(6));
        let line = format!("{left}{}", ".".repeat(74));
        let mut base = vec![line; 24];
        overlay_box(&mut base, 80, 24, "login", &["hello".to_string()], &theme);
        let row = base
            .iter()
            .find(|line| line.contains("hello"))
            .expect("the dialog row rendered");
        assert!(
            row.contains(&left),
            "the left fragment is untouched: {row:?}"
        );
    }

    // Verifies: R1 - a picker's selected row is pi's `selectedBg`
    // highlight: the cells in accent on the selected background, the
    // frame still in the border role, and the rows around it untouched.
    #[test]
    fn the_selected_picker_row_carries_the_selected_background() {
        let theme = Theme::colored();
        let mut base = vec![".".repeat(80); 24];
        overlay_box_selected(
            &mut base,
            80,
            24,
            "model",
            &["first".to_string(), "second".to_string()],
            &theme,
            Some(1),
        );
        let selected = base
            .iter()
            .find(|l| l.contains("second"))
            .expect("the selected row");
        let other = base
            .iter()
            .find(|l| l.contains("first"))
            .expect("the other row");
        assert!(
            selected.contains("\x1b[48;2;58;58;74m"),
            "selectedBg #3a3a4a: {selected:?}"
        );
        assert!(
            selected.contains("\x1b[38;2;138;190;183m"),
            "the selected cells are accent #8abeb7: {selected:?}"
        );
        assert!(
            selected.contains(&theme.role(Role::Border)("│ ")),
            "the frame keeps the border role: {selected:?}"
        );
        assert!(
            !other.contains("48;2;58;58;74"),
            "only the selected row is highlighted: {other:?}"
        );
    }
}

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
