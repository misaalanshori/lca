//! Extension widget rendering (gh #172): the arena walk plus the
//! table/viewport helpers, split from `state.rs` for the workspace's
//! 1,200-line file ceiling. Behavior unchanged.

use super::state::sanitize_text;

/// One node's lines, arena-style: node0 is the root and children are
/// indices. Every text node passes through [`sanitize_text`] - the one
/// choke point for FR-UI-2.
///
/// A node is rendered at most once per call. The arena is supplied by an
/// untrusted extension, so a child index that points at an ancestor (or
/// at itself) must not recurse forever or expand exponentially; the
/// `visited` set is the guard the widget-shaped sibling attack needs.
///
/// The theme paints `StyledText` spans (gh #172): the walk carries it so
/// a span resolves its roles and hex against the live palette. `width`
/// is the region's columns: markdown wraps to it, everything else is
/// width-independent (alignment pads to content, never to the region).
pub fn widget_lines(
    nodes: &[lca_protocol::Widget],
    theme: &crate::theme::Theme,
    width: usize,
) -> Vec<String> {
    fn walk(
        nodes: &[lca_protocol::Widget],
        theme: &crate::theme::Theme,
        width: usize,
        index: usize,
        out: &mut Vec<String>,
        visited: &mut [bool],
    ) {
        use lca_protocol::Widget;
        let Some(node) = nodes.get(index) else { return };
        if visited.get(index) == Some(&true) {
            return;
        }
        if let Some(seen) = visited.get_mut(index) {
            *seen = true;
        }
        match node {
            Widget::Text { content, .. } => out.push(sanitize_text(content)),
            Widget::StyledText { content, style } => {
                out.push(theme.style_text(&sanitize_text(content), style))
            }
            Widget::Image { media_type, bytes } => {
                out.push(format!("[image {media_type}, {} bytes]", bytes.len()))
            }
            // C1 fallbacks (gh #172): the new vocabulary renders as
            // its plain content until C2/C3 teach the host its chrome.
            // Every arm sanitizes - the no-escape rule never waits for
            // the styling commit.
            Widget::Markdown { source } => {
                // The host's own engine (gh #172 acceptance 2): the
                // transcript trusts this parser with model markdown,
                // so extension markdown rides the same path.
                out.extend(lca_tui::widgets::markdown::render_markdown(
                    source,
                    width.max(1),
                    &theme.markdown(),
                    &lca_tui::widgets::markdown::MarkdownOptions::default(),
                ));
            }
            Widget::Button { label, .. } => {
                // Brackets plus the accent label (hover/active arrive
                // with the mouse in C5).
                let painted = (theme.accent)(&sanitize_text(label));
                out.push(format!("[{painted}]"));
            }
            Widget::Table { headers, rows } => {
                out.extend(align_table(headers, rows));
            }
            Widget::ScrollContainer {
                max_height,
                children,
            } => {
                let mut content = Vec::new();
                for child in children {
                    walk(nodes, theme, width, *child as usize, &mut content, visited);
                }
                out.extend(clip_viewport(content, *max_height as usize));
            }
            Widget::Boxed {
                title,
                border,
                background,
                child,
            } => {
                if let Some(title) = title {
                    // The border role paints the title brackets; absent
                    // keeps the bare `[title]` the modal chrome reads.
                    let line = format!("[{}", sanitize_text(title)) + "]";
                    let line = match border {
                        Some(role) => theme.style_text(
                            &line,
                            &lca_protocol::TextStyle {
                                fg: Some(role.clone()),
                                ..Default::default()
                            },
                        ),
                        None => line,
                    };
                    out.push(line);
                }
                let mut content = Vec::new();
                walk(nodes, theme, width, *child as usize, &mut content, visited);
                match background {
                    // The tint washes each child line and closes alone.
                    Some(tint) => out.extend(content.into_iter().map(|line| {
                        theme.style_text(
                            &line,
                            &lca_protocol::TextStyle {
                                bg: Some(tint.clone()),
                                ..Default::default()
                            },
                        )
                    })),
                    None => out.extend(content),
                }
            }
            Widget::Row(children) => {
                // Side by side, first line of each (v1 layout; ponytail:
                // a real row shaper when an extension needs wrapping).
                let parts: Vec<String> = children
                    .iter()
                    .filter_map(|child| {
                        let mut lines = Vec::new();
                        walk(nodes, theme, width, *child as usize, &mut lines, visited);
                        lines.into_iter().next()
                    })
                    .collect();
                out.push(parts.join(" | "));
            }
            Widget::Column(children) => {
                for child in children {
                    walk(nodes, theme, width, *child as usize, out, visited);
                }
            }
            Widget::Spinner { frames } => {
                let ticks = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.subsec_millis())
                    .unwrap_or(0);
                let count = frames.chars().count();
                if count > 0 {
                    let pick = (ticks / 80) as usize % count;
                    out.push(frames.chars().nth(pick).unwrap_or(' ').to_string());
                } else {
                    out.push(" ".to_string());
                }
            }
            Widget::Progress { label, fill } => {
                let fill = (*fill).clamp(0.0, 1.0);
                let width = 20;
                let done = (fill * width as f32).round() as usize;
                out.push(format!(
                    "{label} [{}{}] {:>3}%",
                    "#".repeat(done),
                    "-".repeat(width - done),
                    (fill * 100.0).round() as u32
                ));
            }
            Widget::KeyValue(pairs) => {
                for (key, value) in pairs {
                    out.push(format!("{key}: {}", sanitize_text(value)));
                }
            }
            Widget::Vendor(kind) => out.push(format!("[vendor {kind}]")),
        }
    }
    let mut out = Vec::new();
    if !nodes.is_empty() {
        let mut visited = vec![false; nodes.len()];
        walk(nodes, theme, width, 0, &mut out, &mut visited);
    }
    out
}

/// Align a data grid (gh #172): every column pads to its widest cell
/// (header included), cells join on ` | `, and a dash rule sits under
/// the header. Cells sanitize like every other extension string.
fn align_table(headers: &[String], rows: &[Vec<String>]) -> Vec<String> {
    let cells: Vec<Vec<String>> = std::iter::once(headers)
        .chain(rows.iter().map(Vec::as_slice))
        .map(|row| row.iter().map(|cell| sanitize_text(cell)).collect())
        .collect();
    let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            cells
                .iter()
                .filter_map(|row| row.get(column))
                .map(|cell| lca_tui::engine::text::visible_width(cell))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let rule: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    let pad = |row: &[String]| {
        (0..columns)
            .map(|column| {
                let cell = row.get(column).map(String::as_str).unwrap_or("");
                let pad = widths[column].saturating_sub(lca_tui::engine::text::visible_width(cell));
                format!("{cell}{}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join(" | ")
    };
    let mut out = vec![pad(&cells[0]), rule.join(" | ")];
    out.extend(cells[1..].iter().map(|row| pad(row)));
    out
}

/// Clip child lines to a viewport (gh #172): the first `max_height`
/// lines show, and a scrollbar thumb rides the rows the thumb covers
/// (pi's thumb geometry, content-anchored: the lines are unpadded, so
/// the thumb floats after the content rather than at the margin).
/// The scroll offset arrives with the wheel in C5; until then every
/// viewport opens at the top.
fn clip_viewport(content: Vec<String>, max_height: usize) -> Vec<String> {
    if max_height == 0 {
        return Vec::new();
    }
    let window = content.len().min(max_height);
    let thumb = crate::chat_render::scrollbar_geometry(content.len(), max_height, 0, 2);
    content
        .into_iter()
        .take(window)
        .enumerate()
        .map(|(row, line)| match thumb {
            Some(geometry)
                if row >= geometry.thumb_top as usize
                    && row < geometry.thumb_top as usize + geometry.thumb_height as usize =>
            {
                format!("{line}█")
            }
            Some(_) => format!("{line}│"),
            None => line,
        })
        .collect()
}
#[cfg(test)]
mod styled_widget_tests {
    use super::*;

    // Verifies: gh #172 - a styled span paints through the walk with
    // the live theme, and hostile bytes in styled content sanitize
    // exactly like plain text (FR-UI-2 never waits for styling).
    #[test]
    fn styled_text_paints_and_sanitizes() {
        use lca_protocol::{TextStyle, Widget};
        let theme = crate::theme::Theme::colored();
        let nodes = vec![Widget::StyledText {
            content: "hi \u{1b}[31mx".to_string(),
            style: TextStyle {
                fg: Some("#50fa7b".to_string()),
                bg: None,
                bold: true,
                dim: false,
                italic: false,
                underline: false,
            },
        }];
        assert_eq!(
            widget_lines(&nodes, &theme, 60),
            vec!["\x1b[1;38;2;80;250;123mhi \\x1b[31mx\x1b[22;39m"],
        );
    }
}

#[cfg(test)]
mod arena_widget_tests {
    use super::*;

    fn theme() -> crate::theme::Theme {
        crate::theme::Theme::colored()
    }

    // Verifies: gh #172 - the table widget aligns every column, with a
    // separator under the header (the data-grid half of FR-UI-7).
    #[test]
    fn table_aligns_columns() {
        use lca_protocol::Widget;
        let nodes = vec![Widget::Table {
            headers: vec!["name".to_string(), "value".to_string()],
            rows: vec![
                vec!["a".to_string(), "1".to_string()],
                vec!["longer".to_string(), "22".to_string()],
            ],
        }];
        assert_eq!(
            widget_lines(&nodes, &theme(), 60),
            vec![
                "name   | value".to_string(),
                "------ | -----".to_string(),
                "a      | 1    ".to_string(),
                "longer | 22   ".to_string(),
            ]
        );
    }

    // Verifies: gh #172 - a button draws its brackets with the label in
    // the accent role (hover/active arrive with the mouse in C5).
    #[test]
    fn button_draws_chrome() {
        use lca_protocol::Widget;
        let nodes = vec![Widget::Button {
            id: "ok".to_string(),
            label: "OK".to_string(),
        }];
        let lines = widget_lines(&nodes, &theme(), 60);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with('[') && lines[0].ends_with(']'),
            "brackets: {:?}",
            lines[0]
        );
        assert!(
            lines[0].contains("38;2;") && lines[0].contains("OK"),
            "the label paints accent: {:?}",
            lines[0]
        );
    }

    // Verifies: gh #172 acceptance 2 - markdown renders through the
    // host's engine: headings painted, lists bulleted, code fences
    // highlighted.
    #[test]
    fn markdown_renders_through_the_engine() {
        use lca_protocol::Widget;
        let nodes = vec![Widget::Markdown {
            source: "# hi\n\n- one\n\n```rust\nfn f() {}\n```".to_string(),
        }];
        let lines = widget_lines(&nodes, &theme(), 60);
        let text = lines.join("\n");
        assert!(
            text.contains("38;2;240;198;116m") || text.contains("38;2;240;198;116;"),
            "the heading paints mdHeading: {text:?}"
        );
        assert!(text.contains("one"), "the list item survives: {text:?}");
        let stripped: String = lines
            .iter()
            .map(|l| lca_tui::engine::text::strip_terminal_sequences(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            stripped.contains("fn f()"),
            "the fence's code survives: {stripped:?}"
        );
        assert!(text.contains("38;2;"), "something highlighted: {text:?}");
    }

    // Verifies: gh #172 - a scroll container clips to its viewport and
    // paints the thumb on the rows the thumb covers.
    #[test]
    fn scroll_container_clips_and_thumbs() {
        use lca_protocol::Widget;
        let mut nodes = vec![Widget::ScrollContainer {
            max_height: 2,
            children: vec![1, 2, 3, 4, 5],
        }];
        for i in 1..=5 {
            nodes.push(Widget::Text {
                content: format!("line{i}"),
                role: "default".to_string(),
            });
        }
        let lines = widget_lines(&nodes, &theme(), 60);
        assert_eq!(lines.len(), 2, "the viewport clips: {lines:?}");
        assert!(
            lines.iter().all(|l| l.contains('█')),
            "a two-row thumb over two rows: {lines:?}"
        );
    }

    // Verifies: gh #172 - a box paints its title in the border role
    // and washes its children in the background tint, each channel
    // closed alone.
    #[test]
    fn boxed_paints_border_and_tint() {
        use lca_protocol::Widget;
        let nodes = vec![
            Widget::Boxed {
                title: Some("box".to_string()),
                border: Some("accent".to_string()),
                background: Some("#282a36".to_string()),
                child: 1,
            },
            Widget::Text {
                content: "inside".to_string(),
                role: "default".to_string(),
            },
        ];
        let lines = widget_lines(&nodes, &theme(), 60);
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].contains("38;2;") && lines[0].contains("box"),
            "the title paints the border role: {:?}",
            lines[0]
        );
        assert!(
            lines[1].contains("48;2;40;42;54m")
                && lines[1].contains("inside")
                && lines[1].ends_with("\x1b[49m"),
            "the child rides the tint, closed alone: {:?}",
            lines[1]
        );
    }
}
