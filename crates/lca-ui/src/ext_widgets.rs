//! Extension widget rendering (gh #172): the arena walk plus the
//! table/viewport helpers, split from `state.rs` for the workspace's
//! 1,200-line file ceiling. Behavior unchanged.

use super::state::sanitize_text;

/// What one render needs (gh #172): the theme paints, the width wraps
/// markdown, and the region plus the scroll offsets place viewports.
/// One struct so the signature stops growing with every pillar.
#[derive(Clone, Copy)]
pub struct WidgetCtx<'a> {
    /// The live theme.
    pub theme: &'a crate::theme::Theme,
    /// The region's columns.
    pub width: usize,
    /// The region (`panel`, `modal`, ...): scroll offsets key by this.
    pub region: &'a str,
    /// Scroll offsets by region (the wheel moves them, C5).
    pub offsets: &'a std::collections::HashMap<String, usize>,
}

/// One clickable button's drawn rectangle (gh #172): the line it drew
/// on plus its column span on that line, so a click maps to an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonHit {
    /// The output line.
    pub line: usize,
    /// First column.
    pub col_start: usize,
    /// One past the last column.
    pub col_end: usize,
    /// The widget id.
    pub id: String,
}

/// One walk's output: the lines plus the button rectangles drawn on
/// them, collected together so the two can never drift apart.
struct WalkOut {
    lines: Vec<String>,
    hits: Vec<ButtonHit>,
}

/// One node's lines, arena-style: node0 is the root and children are
/// indices. Every text node passes through [`sanitize_text`] - the one
/// choke point for FR-UI-2.
///
/// A node is rendered at most once per call. The arena is supplied by an
/// untrusted extension, so a child index that points at an ancestor (or
/// at itself) must not recurse forever or expand exponentially; the
/// `visited` set is the guard the widget-shaped sibling attack needs.
pub fn widget_lines(nodes: &[lca_protocol::Widget], ctx: &WidgetCtx) -> Vec<String> {
    widget_render(nodes, ctx).0
}

/// One render's widget context (gh #172): the live theme, the
/// region's width, and the wheel-moved scroll offsets.
pub fn widget_ctx<'a>(
    theme: &'a crate::theme::Theme,
    region: &'a str,
    width: usize,
    offsets: &'a std::collections::HashMap<String, usize>,
) -> WidgetCtx<'a> {
    WidgetCtx {
        theme,
        width,
        region,
        offsets,
    }
}

/// Lines plus button rectangles, one walk (gh #172's hit-testing).
pub fn widget_render(
    nodes: &[lca_protocol::Widget],
    ctx: &WidgetCtx,
) -> (Vec<String>, Vec<ButtonHit>) {
    fn walk(
        nodes: &[lca_protocol::Widget],
        ctx: &WidgetCtx,
        index: usize,
        out: &mut WalkOut,
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
        let theme = ctx.theme;
        let width = ctx.width;
        match node {
            Widget::Text { content, .. } => out.lines.push(sanitize_text(content)),
            Widget::StyledText { content, style } => out
                .lines
                .push(theme.style_text(&sanitize_text(content), style)),
            Widget::Image { media_type, bytes } => out
                .lines
                .push(format!("[image {media_type}, {} bytes]", bytes.len())),
            Widget::Markdown { source } => {
                // The host's own engine (gh #172 acceptance 2): the
                // transcript trusts this parser with model markdown,
                // so extension markdown rides the same path.
                out.lines
                    .extend(lca_tui::widgets::markdown::render_markdown(
                        source,
                        width.max(1),
                        &theme.markdown(),
                        &lca_tui::widgets::markdown::MarkdownOptions::default(),
                    ));
            }
            Widget::Button { id, label } => {
                // Brackets plus the accent label; the rectangle covers
                // the whole line (row-level hit-testing, gh #172).
                let painted = (theme.accent)(&sanitize_text(label));
                let line = format!("[{painted}]");
                let end = lca_tui::engine::text::visible_width(&line);
                out.hits.push(ButtonHit {
                    line: out.lines.len(),
                    col_start: 0,
                    col_end: end,
                    id: id.clone(),
                });
                out.lines.push(line);
            }
            Widget::Table { headers, rows } => {
                out.lines.extend(align_table(headers, rows));
            }
            Widget::ScrollContainer {
                max_height,
                children,
            } => {
                let mut content = WalkOut {
                    lines: Vec::new(),
                    hits: Vec::new(),
                };
                for child in children {
                    walk(nodes, ctx, *child as usize, &mut content, visited);
                }
                let offset = ctx.offsets.get(ctx.region).copied().unwrap_or(0);
                let base = out.lines.len();
                out.lines
                    .extend(clip_viewport(content.lines, *max_height as usize, offset));
                // Kept hits rebase onto the viewport's lines (columns
                // stand still: the thumb appends at the line end).
                out.hits.extend(content.hits.into_iter().filter_map(|hit| {
                    hit.line.checked_sub(offset).and_then(|line| {
                        (line < *max_height as usize).then(|| ButtonHit {
                            line: base + line,
                            col_start: hit.col_start,
                            col_end: hit.col_end,
                            id: hit.id,
                        })
                    })
                }));
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
                    out.lines.push(line);
                }
                // The child walks into the same output (sequential
                // lines, hits included); the tint washes the child's
                // lines only, never its rectangles.
                let claims = out.lines.len();
                walk(nodes, ctx, *child as usize, out, visited);
                if let Some(tint) = background {
                    for line in out.lines.iter_mut().skip(claims) {
                        let plain = std::mem::take(line);
                        *line = theme.style_text(
                            &plain,
                            &lca_protocol::TextStyle {
                                bg: Some(tint.clone()),
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            Widget::Row(children) => {
                // Side by side, first line of each (v1 layout; ponytail:
                // a real row shaper when an extension needs wrapping).
                // Hits from the kept first lines rebase onto the joined
                // line with their column offset.
                let base = out.lines.len();
                let mut parts = Vec::new();
                let mut offset = 0;
                for child in children {
                    let mut temp = WalkOut {
                        lines: Vec::new(),
                        hits: Vec::new(),
                    };
                    walk(nodes, ctx, *child as usize, &mut temp, visited);
                    if let Some(part) = temp.lines.into_iter().next() {
                        out.hits.extend(temp.hits.into_iter().filter_map(|hit| {
                            (hit.line == 0).then(|| ButtonHit {
                                line: base,
                                col_start: hit.col_start + offset,
                                col_end: hit.col_end + offset,
                                id: hit.id,
                            })
                        }));
                        offset += lca_tui::engine::text::visible_width(&part) + 3;
                        parts.push(part);
                    }
                }
                out.lines.push(parts.join(" | "));
            }
            Widget::Column(children) => {
                for child in children {
                    walk(nodes, ctx, *child as usize, out, visited);
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
                    out.lines
                        .push(frames.chars().nth(pick).unwrap_or(' ').to_string());
                } else {
                    out.lines.push(" ".to_string());
                }
            }
            Widget::Progress { label, fill } => {
                let fill = (*fill).clamp(0.0, 1.0);
                let width = 20;
                let done = (fill * width as f32).round() as usize;
                out.lines.push(format!(
                    "{label} [{}{}] {:>3}%",
                    "#".repeat(done),
                    "-".repeat(width - done),
                    (fill * 100.0).round() as u32
                ));
            }
            Widget::KeyValue(pairs) => {
                for (key, value) in pairs {
                    out.lines.push(format!("{key}: {}", sanitize_text(value)));
                }
            }
            Widget::Vendor(kind) => out.lines.push(format!("[vendor {kind}]")),
        }
    }
    let mut out = WalkOut {
        lines: Vec::new(),
        hits: Vec::new(),
    };
    if !nodes.is_empty() {
        let mut visited = vec![false; nodes.len()];
        walk(nodes, ctx, 0, &mut out, &mut visited);
    }
    (out.lines, out.hits)
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
/// Clip child lines to a viewport (gh #172): `offset` lines scrolled
/// off the top (the wheel moves it, C5), then up to `max_height` lines
/// show with a scrollbar thumb on the rows the thumb covers (pi's
/// thumb geometry, content-anchored: the thumb appends at the line
/// end, so content columns stand still).
fn clip_viewport(content: Vec<String>, max_height: usize, offset: usize) -> Vec<String> {
    if max_height == 0 {
        return Vec::new();
    }
    let offset = offset.min(content.len().saturating_sub(1));
    let from_bottom = content.len().saturating_sub(offset + max_height);
    let thumb = crate::chat_render::scrollbar_geometry(content.len(), max_height, from_bottom, 2);
    let kept: Vec<String> = content
        .into_iter()
        .skip(offset)
        .take(max_height)
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
        .collect();
    kept
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
            widget_lines(
                &nodes,
                &WidgetCtx {
                    theme: &theme,
                    width: 60,
                    region: "panel",
                    offsets: &std::collections::HashMap::new(),
                },
            ),
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

    fn panel_ctx<'a>(
        theme: &'a crate::theme::Theme,
        offsets: &'a std::collections::HashMap<String, usize>,
    ) -> WidgetCtx<'a> {
        WidgetCtx {
            theme,
            width: 60,
            region: "panel",
            offsets,
        }
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
        let theme = theme();
        let offsets = std::collections::HashMap::new();
        assert_eq!(
            widget_lines(&nodes, &panel_ctx(&theme, &offsets)),
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
        let theme = theme();
        let offsets = std::collections::HashMap::new();
        let lines = widget_lines(&nodes, &panel_ctx(&theme, &offsets));
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
        let theme = theme();
        let offsets = std::collections::HashMap::new();
        let lines = widget_lines(&nodes, &panel_ctx(&theme, &offsets));
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
        let theme = theme();
        let offsets = std::collections::HashMap::new();
        let lines = widget_lines(&nodes, &panel_ctx(&theme, &offsets));
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
        let theme = theme();
        let offsets = std::collections::HashMap::new();
        let lines = widget_lines(&nodes, &panel_ctx(&theme, &offsets));
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

#[cfg(test)]
mod hit_tests {
    use super::*;

    fn ctx<'a>(
        theme: &'a crate::theme::Theme,
        offsets: &'a std::collections::HashMap<String, usize>,
    ) -> WidgetCtx<'a> {
        WidgetCtx {
            theme,
            width: 60,
            region: "panel",
            offsets,
        }
    }

    // Verifies: gh #172 - buttons report their line, columns, and id in
    // one walk with the lines (no second traversal to drift from).
    #[test]
    fn buttons_report_line_columns_and_id() {
        use lca_protocol::Widget;
        let theme = crate::theme::Theme::colored();
        let offsets = std::collections::HashMap::new();
        let nodes = vec![
            Widget::Column(vec![1, 2]),
            Widget::Text {
                content: "head".into(),
                role: "default".into(),
            },
            Widget::Button {
                id: "ok".into(),
                label: "OK".into(),
            },
        ];
        let (lines, hits) = widget_render(&nodes, &ctx(&theme, &offsets));
        assert_eq!(lines.len(), 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[0].id, "ok");
        assert_eq!(hits[0].col_start, 0);
        assert_eq!(
            hits[0].col_end,
            lca_tui::engine::text::visible_width(&lines[1])
        );
    }

    // Verifies: gh #172 - buttons nested in rows and scroll containers
    // map to the joined/clipped line they actually drew on.
    #[test]
    fn nested_buttons_map_to_their_drawn_line() {
        use lca_protocol::Widget;
        let theme = crate::theme::Theme::colored();
        let offsets = std::collections::HashMap::new();
        let nodes = vec![
            Widget::Column(vec![1, 4]),
            Widget::Row(vec![2, 3]),
            Widget::Button {
                id: "left".into(),
                label: "L".into(),
            },
            Widget::Button {
                id: "right".into(),
                label: "R".into(),
            },
            Widget::ScrollContainer {
                max_height: 1,
                children: vec![5, 6],
            },
            Widget::Button {
                id: "top".into(),
                label: "T".into(),
            },
            Widget::Button {
                id: "buried".into(),
                label: "B".into(),
            },
        ];
        let (lines, hits) = widget_render(&nodes, &ctx(&theme, &offsets));
        // Row joins on line 0, the scroll viewport clips to line 1.
        assert_eq!(lines.len(), 2, "{lines:?}");
        let left = hits.iter().find(|hit| hit.id == "left").expect("left");
        let right = hits.iter().find(|hit| hit.id == "right").expect("right");
        assert_eq!((left.line, right.line), (0, 0));
        assert!(
            right.col_start > left.col_end,
            "side by side, not stacked: {hits:?}"
        );
        assert!(
            hits.iter().any(|hit| hit.id == "top" && hit.line == 1),
            "the visible scroll child: {hits:?}"
        );
        assert!(
            hits.iter().all(|hit| hit.id != "buried"),
            "the clipped child draws no hit: {hits:?}"
        );
    }
}
