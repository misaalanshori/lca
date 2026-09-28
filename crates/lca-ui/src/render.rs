//! Overlay and dock composition over the engine's line strings.
//!
//! The transcript flows as line strings; the dock (notice + editor +
//! footer) sits below; a modal composites a centered box over the visible
//! viewport, and the side panel takes the right column. Extensions never
//! write escape sequences - their widget trees pass through
//! `widget_lines` (ADR-0003).

use lca_tui::engine::core::resolve_overlay_layout;
use lca_tui::engine::text::{slice_by_column, visible_width, wrap_text_with_ansi};

/// A centered box drawn over `base`, which is the visible viewport.
///
/// The rectangle is resolved by the engine's overlay layout (anchor,
/// margin, clamping) so the compositor and the focus machine agree on
/// where an overlay lives.
pub fn overlay_box(base: &mut [String], width: u16, height: u16, title: &str, body: &[String]) {
    let content_height = u16::try_from(body.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let options = lca_tui::engine::core::OverlayOptions {
        width: Some(lca_tui::engine::core::SizeValue::Percent(80)),
        min_width: Some(24),
        max_height: Some(lca_tui::engine::core::SizeValue::Abs(height)),
        margin: 2,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&options, width, height, content_height);
    let w = rect.width as usize;
    let inner = w.saturating_sub(4);

    // A full frame: pi's selectors draw complete boxes (DynamicBorder),
    // and a left-only border reads as a half-drawn frame.
    let mut box_lines: Vec<String> = Vec::new();
    let title_w = visible_width(title);
    box_lines.push(format!(
        "╭─ {title} {}╮",
        "─".repeat(w.saturating_sub(title_w + 5))
    ));
    for line in body {
        for wrapped in wrap_text_with_ansi(line, inner) {
            let pad = inner.saturating_sub(visible_width(&wrapped));
            box_lines.push(format!("│ {wrapped}{} │", " ".repeat(pad)));
        }
    }
    box_lines.push(format!("╰{}╯", "─".repeat(w.saturating_sub(2))));

    let box_h = box_lines.len().min(height as usize);
    let col = rect.col as usize;
    let top = rect.row as usize;
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
            *base_line = format!("{before}{line}{pad}{after}");
        }
    }
}

/// Composite a side panel into the right column of every line.
pub fn side_panel(base: &mut [String], width: u16, panel: &[String]) {
    let panel_w = 40usize.min(width as usize / 2);
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

#[cfg(test)]
mod tests {
    use super::*;
    use lca_tui::engine::text::strip_terminal_sequences;

    #[test]
    fn overlay_box_centers_over_the_viewport() {
        let mut base = vec![".".repeat(80); 24];
        overlay_box(&mut base, 80, 24, "login", &["hello".to_string()]);
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
        overlay_box(&mut base, 80, 24, "login", &["hello".to_string()]);
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
}
