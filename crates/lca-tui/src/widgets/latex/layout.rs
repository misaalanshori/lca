//! The LaTeX layout engine: baseline-joined 2D blocks for fractions,
//! operator limits, stacked scripts, and matrices (pi's `latex.ts`
//! §2-3, RE `tui-widgets/latex.md`).
//!
//! pi embeds layout nodes as private-use sentinel pairs in the flat
//! string (`index`) and composes them at render time; the same
//! trick ports directly, because a Rust `String` is happy to carry
//! private-use code points and the parser stays single-pass.

use crate::engine::text::visible_width;

/// Layout marker: `` + decimal index + ``.
pub(super) const MARKER_START: char = '\u{f0000}';
/// Layout marker closer.
pub(super) const MARKER_END: char = '\u{f0001}';
/// A space that survives normalization (cases/matrix column alignment).
pub(super) const PROTECTED_SPACE: char = '\u{f0002}';
/// Named-operator sentinels (`sin`-style spacing bookends).
pub(super) const NAMED_START: char = '\u{f0004}';
pub(super) const NAMED_END: char = '\u{f0005}';
/// pi's NEGATIVE_SPACE sentinel: a trim marker for `!`-style commands.
pub(super) const NEGATIVE_SPACE: char = '\0';

/// pi's `LayoutNode` union.
#[derive(Debug, Clone)]
pub(super) enum Node {
    /// Stacked numerator/bar/denominator.
    Fraction {
        numerator: String,
        denominator: String,
    },
    /// Operator with limits above/below (display mode).
    Operator {
        operator: String,
        lower: Option<String>,
        upper: Option<String>,
    },
    /// Super/sub scripts stacked across a baseline gap.
    Script {
        lower: Option<String>,
        upper: Option<String>,
    },
    /// Pre-rendered lines with a baseline (matrices, cases).
    Matrix { lines: Vec<String>, baseline: usize },
}

/// One composed block: lines, width, and the row the baseline sits on.
#[derive(Debug, Clone)]
pub(super) struct Layout {
    /// The composed rows.
    pub(super) lines: Vec<String>,
    /// The block's width in cells.
    pub(super) width: usize,
    /// The row the baseline sits on.
    pub(super) baseline: usize,
}

fn pad_line(line: &str, width: usize, centered: bool) -> String {
    let padding = width.saturating_sub(visible_width(line));
    let left = if centered { padding / 2 } else { 0 };
    format!("{}{}{}", " ".repeat(left), line, " ".repeat(padding - left))
}

/// Join blocks on a shared baseline (pi's `joinLayouts`).
pub(super) fn join_layouts(items: &[Layout]) -> Layout {
    if items.is_empty() {
        return Layout {
            lines: vec![String::new()],
            width: 0,
            baseline: 0,
        };
    }
    let baseline = items.iter().map(|l| l.baseline).max().unwrap_or(0);
    let below = items
        .iter()
        .map(|l| l.lines.len().saturating_sub(l.baseline + 1))
        .max()
        .unwrap_or(0);
    let mut lines = Vec::new();
    for row in 0..=baseline + below {
        let mut line = String::new();
        for layout in items {
            let source_row = row as isize - baseline as isize + layout.baseline as isize;
            if source_row >= 0 && (source_row as usize) < layout.lines.len() {
                line.push_str(&pad_line(
                    &layout.lines[source_row as usize],
                    layout.width,
                    false,
                ));
            } else {
                line.push_str(&" ".repeat(layout.width));
            }
        }
        lines.push(line.trim_end().to_string());
    }
    Layout {
        lines,
        width: items.iter().map(|l| l.width).sum(),
        baseline,
    }
}

/// Find the next layout marker at or after `from`; returns the span and
/// the node index (pi's `LAYOUT_MARKER_PATTERN` match).
fn next_marker(chars: &[char], from: usize) -> Option<(usize, usize, usize)> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == MARKER_START {
            let mut j = i + 1;
            let mut index = 0usize;
            let mut any = false;
            while j < chars.len() && chars[j].is_ascii_digit() {
                index = index * 10 + usize::from(u8::try_from(chars[j]).unwrap_or(b'0') - b'0');
                j += 1;
                any = true;
            }
            if any && j < chars.len() && chars[j] == MARKER_END {
                return Some((i, j + 1, index));
            }
        }
        i += 1;
    }
    None
}

fn text_layout(text: &str) -> Layout {
    Layout {
        lines: vec![text.to_string()],
        width: visible_width(text),
        baseline: 0,
    }
}

/// Compose the flat marker string into 2D blocks (pi's `renderLayout`).
pub(super) fn render_layout(source: &str, nodes: &[Node]) -> Layout {
    let mut rendered_lines: Vec<String> = Vec::new();
    let mut first_baseline = 0usize;
    for source_line in source.split('\n') {
        let chars: Vec<char> = source_line.chars().collect();
        let mut layouts: Vec<Layout> = Vec::new();
        let mut position = 0usize;
        let mut previous: Option<&Node> = None;
        let mut scan_from = 0usize;
        while let Some((start, end, index)) = next_marker(&chars, scan_from) {
            let node = match nodes.get(index) {
                Some(node) => node,
                None => {
                    scan_from = end;
                    continue;
                }
            };
            if start > position {
                let sliced: String = chars[position..start].iter().collect();
                let trimmed_base = if previous.is_some() {
                    sliced.trim_start()
                } else {
                    &sliced[..]
                };
                let trimmed = trimmed_base.trim_end();
                let preserve_leading = matches!(previous, Some(Node::Matrix { .. }))
                    && sliced.chars().next().is_some_and(char::is_whitespace);
                let preserve_trailing = matches!(node, Node::Matrix { .. })
                    && sliced.chars().next_back().is_some_and(char::is_whitespace);
                let text = if !trimmed.is_empty() {
                    format!(
                        "{}{}{}",
                        if preserve_leading { " " } else { "" },
                        trimmed,
                        if preserve_trailing { " " } else { "" }
                    )
                } else if preserve_leading || preserve_trailing {
                    " ".to_string()
                } else {
                    String::new()
                };
                layouts.push(text_layout(&text));
            }
            layouts.push(render_node(node, nodes));
            position = end;
            previous = Some(node);
            scan_from = end;
        }
        if position < chars.len() {
            let sliced: String = chars[position..].iter().collect();
            let trimmed = if previous.is_some() {
                sliced.trim_start()
            } else {
                &sliced[..]
            };
            let text = if matches!(previous, Some(Node::Matrix { .. }))
                && sliced.chars().next().is_some_and(char::is_whitespace)
            {
                format!(" {trimmed}")
            } else {
                trimmed.to_string()
            };
            layouts.push(text_layout(&text));
        }
        let joined = join_layouts(&layouts);
        if rendered_lines.is_empty() {
            first_baseline = joined.baseline;
        }
        rendered_lines.extend(joined.lines);
    }
    let width = rendered_lines
        .iter()
        .map(|l| visible_width(l))
        .max()
        .unwrap_or(0);
    Layout {
        lines: rendered_lines,
        width,
        baseline: first_baseline,
    }
}

fn render_node(node: &Node, nodes: &[Node]) -> Layout {
    match node {
        Node::Fraction {
            numerator,
            denominator,
        } => {
            let num = render_layout(numerator, nodes);
            let den = render_layout(denominator, nodes);
            let content_width = num.width.max(den.width).max(1);
            let width = content_width + 2;
            let mut lines: Vec<String> =
                num.lines.iter().map(|l| pad_line(l, width, true)).collect();
            lines.push(format!(" {} ", "─".repeat(content_width)));
            lines.extend(den.lines.iter().map(|l| pad_line(l, width, true)));
            Layout {
                lines,
                width,
                baseline: num.lines.len(),
            }
        }
        Node::Operator {
            operator,
            lower,
            upper,
        } => {
            let content_width = visible_width(operator)
                .max(lower.as_deref().map(visible_width).unwrap_or(0))
                .max(upper.as_deref().map(visible_width).unwrap_or(0));
            let mut lines = Vec::new();
            if let Some(upper) = upper {
                lines.push(format!("{} ", pad_line(upper, content_width, true)));
            }
            lines.push(format!("{} ", pad_line(operator, content_width, true)));
            if let Some(lower) = lower {
                lines.push(format!("{} ", pad_line(lower, content_width, true)));
            }
            Layout {
                lines,
                width: content_width + 1,
                baseline: if upper.is_some() { 1 } else { 0 },
            }
        }
        Node::Script { lower, upper } => {
            let up = upper.as_deref().map(|s| render_layout(s, nodes));
            let low = lower.as_deref().map(|s| render_layout(s, nodes));
            let width = up
                .as_ref()
                .map_or(0, |l| l.width)
                .max(low.as_ref().map_or(0, |l| l.width));
            let mut lines = Vec::new();
            if let Some(up) = &up {
                lines.extend(up.lines.iter().map(|l| pad_line(l, width, false)));
            }
            lines.push(" ".repeat(width));
            if let Some(low) = &low {
                lines.extend(low.lines.iter().map(|l| pad_line(l, width, false)));
            }
            let baseline = up.as_ref().map_or(0, |l| l.lines.len());
            Layout {
                lines,
                width,
                baseline,
            }
        }
        Node::Matrix { lines, baseline } => {
            let width = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
            Layout {
                lines: lines.iter().map(|l| pad_line(l, width, false)).collect(),
                width,
                baseline: *baseline,
            }
        }
    }
}

/// pi's `normalizeScriptValue`: trim, and pull spaces tight to `=+-`.
pub(super) fn normalize_script_value(value: &str) -> String {
    let chars: Vec<char> = value.trim().chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, '=' | '+' | '-') {
            while out.ends_with(' ') {
                out.pop();
            }
            out.push(c);
            i += 1;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn replace_characters(value: &str, table: &[(&str, &str)]) -> Option<String> {
    let mut out = String::new();
    for c in value.chars() {
        let key = c.to_string();
        out.push_str(table.iter().find(|(k, _)| *k == key)?.1);
    }
    Some(out)
}

/// pi's `formatUnicodeScript`: the Unicode map, or `None` when coverage
/// is partial (the caller falls back to `^{}`/`_()` notation).
pub(super) fn format_unicode_script(value: &str, sub: bool) -> Option<String> {
    let normalized = normalize_script_value(value);
    let table = if sub {
        super::symbols::SUBSCRIPTS
    } else {
        super::symbols::SUPERSCRIPTS
    };
    replace_characters(&normalized, table)
}

/// pi's `formatScript`: Unicode when fully mapped, `^x`/`_x` for a single
/// character (or a plain-letter subscript), `^(...)` otherwise.
pub(super) fn format_script(value: &str, sub: bool) -> String {
    let normalized = normalize_script_value(value);
    if let Some(mapped) = format_unicode_script(&normalized, sub) {
        return mapped;
    }
    let prefix = if sub { '_' } else { '^' };
    let chars: Vec<char> = normalized.chars().collect();
    let plain_letters_sub =
        sub && !normalized.is_empty() && normalized.chars().all(|c| c.is_ascii_alphabetic());
    if chars.len() == 1 || plain_letters_sub {
        return format!("{prefix}{normalized}");
    }
    format!("{prefix}({normalized})")
}

fn is_simple_word(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_alphanumeric() || c == '.')
}

fn is_simple_denominator(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_numeric() || c == '.')
        || value.chars().count() == 1
}

/// pi's `formatFraction`: inline `a/b`, parenthesizing complex sides.
pub(super) fn format_fraction(numerator: &str, denominator: &str) -> String {
    let numerator = numerator.trim();
    let denominator = denominator.trim();
    let num = if is_simple_word(numerator) {
        numerator.to_string()
    } else {
        format!("({numerator})")
    };
    let den = if is_simple_denominator(denominator) {
        denominator.to_string()
    } else {
        format!("({denominator})")
    };
    format!("{num}/{den}")
}

/// pi's `formatRoot`: `√x` or `√(x)`.
pub(super) fn format_root(value: &str, symbol: &str) -> String {
    let value = value.trim();
    if is_simple_word(value) {
        format!("{symbol}{value}")
    } else {
        format!("{symbol}({value})")
    }
}

/// pi's `normalizeOutput`: named-operator spacing, sentinel removal,
/// per-line space collapsing, and the leading/trailing blank-line filter.
pub(super) fn normalize_output(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut spaced: Vec<char> = Vec::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == NAMED_START {
            if i > 0 {
                let prev = chars[i - 1];
                if prev.is_alphanumeric() || matches!(prev, ')' | ']' | '}' | MARKER_END) {
                    spaced.push(' ');
                }
            }
            continue;
        }
        if c == NAMED_END {
            spaced.push(c);
            if let Some(&next) = chars.get(i + 1)
                && (next.is_alphanumeric() || next == '√' || next == MARKER_START)
            {
                spaced.push(' ');
            }
            continue;
        }
        spaced.push(c);
    }
    let text: String = spaced
        .into_iter()
        .filter(|c| *c != NAMED_START && *c != NAMED_END)
        .collect();
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let mut collapsed = String::new();
            let mut last_space = false;
            for c in line.chars() {
                if c == ' ' || c == '\t' {
                    if !last_space {
                        collapsed.push(' ');
                    }
                    last_space = true;
                } else {
                    collapsed.push(c);
                    last_space = false;
                }
            }
            collapsed.trim().to_string()
        })
        .collect();
    let filtered: Vec<&String> = lines
        .iter()
        .enumerate()
        .filter(|(i, line)| !line.is_empty() || (*i > 0 && *i + 1 < lines.len()))
        .map(|(_, line)| line)
        .collect();
    filtered
        .iter()
        .map(|line| line.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}
