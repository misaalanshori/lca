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

/// [`overlay_box_selected`] anchored above the composer (gh #16): the
/// box's bottom sits `above_rows` rows above the viewport bottom - pi's
/// bottom-anchored picker shape - so a tall notice is overlaid, never
/// stacked under.
// The eight inputs are the picker's contract plus its anchor offset; a
// parameter struct would only rename them.
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
) {
    let options = lca_tui::engine::core::OverlayOptions {
        width: Some(lca_tui::engine::core::SizeValue::Percent(80)),
        min_width: Some(24),
        max_height: Some(lca_tui::engine::core::SizeValue::Abs(height)),
        margin: 2,
        anchor: Some(lca_tui::engine::core::Anchor::BottomCenter),
        offset_y: -(i32::try_from(above_rows).unwrap_or(i32::MAX)),
        ..Default::default()
    };
    overlay_box_placed(base, width, height, title, body, theme, selected, &options);
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
    let mut box_lines: Vec<String> = Vec::new();
    // Which body row each box row belongs to (`usize::MAX` = frame).
    let mut owner: Vec<usize> = Vec::new();
    let title_w = visible_width(title);
    box_lines.push(format!(
        "╭─ {title} {}╮",
        "─".repeat(w.saturating_sub(title_w + 5))
    ));
    owner.push(usize::MAX);
    for (index, line) in body.iter().enumerate() {
        // gh #178: a raw URL linkifies before the wrap, so every wrapped
        // segment re-opens the full link and clicks open the whole URL.
        for wrapped in wrap_text_with_ansi(&linkify_urls(line), inner) {
            let pad = inner.saturating_sub(visible_width(&wrapped));
            box_lines.push(format!("│ {wrapped}{} │", " ".repeat(pad)));
            owner.push(index);
        }
    }
    box_lines.push(format!("╰{}╯", "─".repeat(w.saturating_sub(2))));
    owner.push(usize::MAX);

    let box_h = box_lines.len().min(height as usize);
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
    for (i, line) in box_lines.iter().take(box_h).enumerate() {
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

/// Composite a side panel into the right column of every line.
pub fn side_panel(base: &mut [String], width: u16, panel: &[String]) {
    let panel_w = panel_width(width);
    for (i, base_line) in base.iter_mut().enumerate() {
        let text = panel.get(i).map(String::as_str).unwrap_or("");
        let text = lca_tui::engine::text::truncate_to_width(text, panel_w, "…", false);
        let col = (width as usize).saturating_sub(panel_w);
        let before = slice_by_column(base_line, 0, col, false);
        let before = format!(
            "{before}{}",
            " ".repeat(col.saturating_sub(visible_width(&before)))
        );
        *base_line = format!("{before}{text}");
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
        let after = slice_by_column(row, x as usize + width, 10_000, false);
        *row = format!("{before}{}{after}", bg(&ink(line)));
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

    #[test]
    fn side_panel_takes_the_right_column() {
        let mut base = vec!["x".repeat(80); 3];
        side_panel(&mut base, 80, &["panel".to_string()]);
        let line = strip_terminal_sequences(&base[0]);
        assert!(line.ends_with("panel"));
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
