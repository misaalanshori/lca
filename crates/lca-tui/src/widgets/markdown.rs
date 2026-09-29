//! Streaming-tolerant terminal markdown, ported from pi's
//! `packages/tui/src/components/markdown.ts`
//! (`pi-tui-re/src_re/tui-widgets/markdown.md`).
//!
//! This is the answer to owner issue #6 ("the chat history is hard to
//! read"): headings, lists, tables rendered as aligned columns, framed code
//! blocks, blockquotes, and inline styles.
//!
//! Not ported (documented skips): LaTeX math (the brief says skip it) and
//! mermaid (pi shells out). **Syntax highlighting is a known gap, corrected
//! 2026-09-29:** cycle 4 closed it as "not a gap" from a color-stripped
//! `tmux capture-pane`, but pi does highlight — its markdown calls
//! `theme.highlightCode` (`markdown.ts` §523) and its read/write tool
//! renderers highlight too. LCA's [`MarkdownTheme`] carries no `highlight`
//! hook and the theme's `Syntax*` roles have no consumer; see the cycle-5
//! report. HTML is rendered as literal text by construction — no markup
//! reaches the terminal (the hostile-input stance).
//!
//! The tokenizer is hand-written and line-based rather than `pulldown-cmark`
//! (ADR-0036 adds no such dependency). It covers the block and inline shapes
//! a coding transcript uses; `ponytail:` upgrade to `pulldown-cmark` if a
//! real document needs constructs this misses.

use std::sync::Arc;

use crate::engine::text::{truncate_to_width, visible_width, wrap_text_with_ansi};

/// A styling function.
pub type StyleFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

fn identity(s: &str) -> String {
    s.to_string()
}

/// The style vocabulary markdown rendering uses (pi's `MarkdownTheme`).
#[derive(Clone)]
pub struct MarkdownTheme {
    /// Heading style.
    pub heading: StyleFn,
    /// Bold style.
    pub bold: StyleFn,
    /// Underline style (pi underlines the level-1 heading).
    pub underline: StyleFn,
    /// Italic style.
    pub italic: StyleFn,
    /// Strikethrough style.
    pub strike: StyleFn,
    /// Inline code style.
    pub code: StyleFn,
    /// Code-block content style.
    pub code_block: StyleFn,
    /// Code-block border style.
    pub code_block_border: StyleFn,
    /// Link style.
    pub link: StyleFn,
    /// Blockquote border style.
    pub quote: StyleFn,
    /// Horizontal-rule style.
    pub hr: StyleFn,
}

impl Default for MarkdownTheme {
    fn default() -> Self {
        Self {
            heading: Arc::new(identity),
            bold: Arc::new(identity),
            underline: Arc::new(identity),
            italic: Arc::new(identity),
            strike: Arc::new(identity),
            code: Arc::new(identity),
            code_block: Arc::new(identity),
            code_block_border: Arc::new(identity),
            link: Arc::new(identity),
            quote: Arc::new(identity),
            hr: Arc::new(identity),
        }
    }
}

/// Whether a terminal hyperlink should be emitted for links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    /// Emit OSC 8 hyperlinks (the URL is not printed).
    Hyperlink,
    /// Print `text (url)`.
    Inline,
}

/// Options for rendering.
#[derive(Clone)]
pub struct MarkdownOptions {
    /// Left/right padding.
    pub padding_x: usize,
    /// Blank lines above and below.
    pub padding_y: usize,
    /// Link rendering.
    pub link_mode: LinkMode,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            padding_x: 0,
            padding_y: 0,
            link_mode: LinkMode::Hyperlink,
        }
    }
}

/// Render markdown to styled lines.
pub fn render_markdown(
    text: &str,
    width: usize,
    theme: &MarkdownTheme,
    options: &MarkdownOptions,
) -> Vec<String> {
    let inner_width = width.saturating_sub(options.padding_x * 2).max(1);
    let expanded = text.replace('\t', "   ");
    let normalized = expanded.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();

    let mut out: Vec<String> = Vec::new();
    for _ in 0..options.padding_y {
        out.push(String::new());
    }

    let mut i = 0;
    // pi's default list rendering renumbers an ordered run from its start
    // (`${start + i}. `) and uses `- ` for every unordered bullet.
    let mut ordered_next: Option<u64> = None;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            ordered_next = None;
            let fence: String = trimmed
                .chars()
                .take_while(|c| *c == '`' || *c == '~')
                .collect();
            let lang = trimmed[fence.len()..].trim().to_string();
            let mut body: Vec<String> = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(&fence) {
                body.push(lines[i].to_string());
                i += 1;
            }
            // Streaming tolerance: a partial closing fence on the last line
            // is trimmed so the block does not flicker (pi issue #5825).
            if i >= lines.len() {
                if let Some(last) = body.last_mut() {
                    let fence_char = fence.chars().next().unwrap_or('`');
                    let partial: String = last
                        .trim_start()
                        .chars()
                        .take_while(|c| *c == fence_char)
                        .collect();
                    if !partial.is_empty() && partial.len() < fence.len() {
                        let cut = last.len() - last.trim_start().len();
                        last.truncate(cut);
                        if last.trim().is_empty() {
                            body.pop();
                        }
                    }
                }
            } else {
                i += 1; // consume the closing fence
            }
            render_code_block(&body, &lang, inner_width, theme, &mut out);
            continue;
        }

        if is_table_start(&lines, i) {
            let (table, consumed) = collect_table(&lines, i);
            render_table(&table, inner_width, theme, &mut out);
            i += consumed;
            continue;
        }

        if let Some((level, content)) = heading(trimmed) {
            // pi: h1 is heading(bold(underline(text))), h2 is
            // heading(bold(text)), and h3+ prepend the literal prefix.
            let styled = match level {
                1 => (theme.heading)(&(theme.bold)(&(theme.underline)(&content))),
                2 => (theme.heading)(&(theme.bold)(&content)),
                _ => (theme.heading)(&format!("{} {content}", "#".repeat(level))),
            };
            out.extend(wrap_text_with_ansi(&styled, inner_width));
            out.push(String::new());
            i += 1;
            continue;
        }

        if is_hr(trimmed) {
            out.push((theme.hr)(&"─".repeat(inner_width.min(80))));
            i += 1;
            continue;
        }

        if trimmed.starts_with('>') {
            let mut body = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                body.push(
                    lines[i]
                        .trim_start()
                        .trim_start_matches('>')
                        .trim_start()
                        .to_string(),
                );
                i += 1;
            }
            for line in &body {
                let rendered = render_inline(line, theme, options);
                let prefix = (theme.quote)("│ ");
                for wrapped in wrap_text_with_ansi(&rendered, inner_width.saturating_sub(2)) {
                    out.push(format!("{prefix}{wrapped}"));
                }
            }
            continue;
        }

        if let Some((kind, content, indent)) = list_item(trimmed) {
            let marker = match kind {
                ListMarker::Bullet { task } => match task {
                    Some(true) => "- [x] ".to_string(),
                    Some(false) => "- [ ] ".to_string(),
                    None => "- ".to_string(),
                },
                ListMarker::Ordered { start } => {
                    let n = *ordered_next.get_or_insert(start);
                    ordered_next = Some(n + 1);
                    format!("{n}. ")
                }
            };
            let rendered = render_inline(&content, theme, options);
            let prefix = " ".repeat(indent) + &marker;
            let marker_width = visible_width(&prefix);
            let wrapped = wrap_text_with_ansi(&rendered, inner_width.saturating_sub(marker_width));
            for (j, wrapped_line) in wrapped.iter().enumerate() {
                if j == 0 {
                    out.push(format!("{prefix}{wrapped_line}"));
                } else {
                    out.push(format!("{}{wrapped_line}", " ".repeat(marker_width)));
                }
            }
            i += 1;
            continue;
        }

        ordered_next = None;

        if trimmed.is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            i += 1;
            continue;
        }

        // Paragraph.
        let rendered = render_inline(trimmed, theme, options);
        out.extend(wrap_text_with_ansi(&rendered, inner_width));
        i += 1;
    }

    // Apply horizontal padding.
    if options.padding_x > 0 {
        let pad = " ".repeat(options.padding_x);
        out = out.into_iter().map(|l| format!("{pad}{l}")).collect();
    }
    for _ in 0..options.padding_y {
        out.push(String::new());
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn heading(line: &str) -> Option<(usize, String)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&level) && line.chars().nth(level) == Some(' ') {
        Some((level, line[level..].trim().to_string()))
    } else {
        None
    }
}

fn is_hr(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

/// The kind of list item a line starts, before rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListMarker {
    /// An unordered item, with its task state when it is a task item.
    Bullet { task: Option<bool> },
    /// An ordered item carrying the number the run starts at.
    Ordered { start: u64 },
}

fn list_item(line: &str) -> Option<(ListMarker, String, usize)> {
    let indent = line.len() - line.trim_start().len();
    let rest = line.trim_start();
    for bullet in ["- ", "* ", "+ "] {
        if let Some(content) = rest.strip_prefix(bullet) {
            // pi keeps the literal `[x]`/`[ ]` marker (`markdown.ts`'s
            // `taskMarker`), after the bullet.
            let (task, content) = match content
                .strip_prefix("[x] ")
                .or_else(|| content.strip_prefix("[X] "))
            {
                Some(rest) => (Some(true), rest.to_string()),
                None => match content.strip_prefix("[ ] ") {
                    Some(rest) => (Some(false), rest.to_string()),
                    None => (None, content.to_string()),
                },
            };
            return Some((ListMarker::Bullet { task }, content, indent));
        }
    }
    // Ordered: N. or N)
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &rest[digits.len()..];
        if let Some(content) = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix(") "))
        {
            let start = digits.parse::<u64>().unwrap_or(1);
            return Some((ListMarker::Ordered { start }, content.to_string(), indent));
        }
    }
    None
}

fn is_table_start(lines: &[&str], i: usize) -> bool {
    if i + 1 >= lines.len() {
        return false;
    }
    let header = lines[i].trim();
    // Streaming tolerance: a table needs a complete header row and an
    // intact separator row; a partial pipe or a half-written separator
    // renders as text until the next chunk completes it.
    header.starts_with('|') && header.ends_with('|') && is_table_separator(lines[i + 1])
}

/// Whether a row is a complete GFM separator: every cell is `---`, `:--`,
/// `--:`, or `:-:`.
fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    if !t.starts_with('|') || !t.ends_with('|') {
        return false;
    }
    let cells = split_table_row(t);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.contains('-') && cell.chars().all(|c| c == '-' || c == ':')
        })
}

fn split_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn collect_table(lines: &[&str], start: usize) -> (Vec<Vec<String>>, usize) {
    let mut rows = Vec::new();
    let header = split_table_row(lines[start]);
    rows.push(header);
    let mut i = start + 2; // skip the separator
    while i < lines.len() {
        let t = lines[i].trim();
        // A partial row (no closing pipe) is not yet a table row.
        if !t.starts_with('|') || !t.ends_with('|') {
            break;
        }
        rows.push(split_table_row(lines[i]));
        i += 1;
    }
    (rows, i - start)
}

fn render_code_block(
    body: &[String],
    lang: &str,
    width: usize,
    theme: &MarkdownTheme,
    out: &mut Vec<String>,
) {
    // D10: the border caps at the content width, not the terminal width.
    let content_width = body.iter().map(|l| visible_width(l)).max().unwrap_or(0);
    // A full frame: 2 border + 2 padding around the content, capped at the
    // terminal width. A left-only border reads as a half-drawn frame.
    let frame = (content_width + 4).min(width).max(4);
    let inner = frame.saturating_sub(4);
    let title = if lang.is_empty() {
        String::new()
    } else {
        format!(" {lang} ")
    };
    let top_fill = frame.saturating_sub(visible_width(&title) + 3);
    out.push((theme.code_block_border)(&format!(
        "╭─{title}{}╮",
        "─".repeat(top_fill)
    )));
    for line in body {
        let styled = (theme.code_block)(line);
        let pad = inner.saturating_sub(visible_width(line));
        out.push(format!("│ {styled}{} │", " ".repeat(pad)));
    }
    out.push((theme.code_block_border)(&format!(
        "╰{}╯",
        "─".repeat(frame.saturating_sub(2))
    )));
}

fn render_table(rows: &[Vec<String>], width: usize, theme: &MarkdownTheme, out: &mut Vec<String>) {
    #[allow(clippy::needless_range_loop)] // columns index parallel width vectors
    if rows.is_empty() {
        return;
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    // Border overhead is 3n+1; too narrow to be stable -> raw fallback.
    if width < cols * 3 + 1 {
        for row in rows {
            out.push(row.join(" | "));
        }
        return;
    }
    let natural: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .map(|r| visible_width(r.get(c).map(String::as_str).unwrap_or("")))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let min_word: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .flat_map(|r| r.get(c).into_iter().flat_map(|s| s.split_whitespace()))
                .map(visible_width)
                .max()
                .unwrap_or(1)
                .min(30)
        })
        .collect();
    let available = width - (cols * 3 + 1);
    let mut widths = natural.clone();
    let total: usize = natural.iter().sum();
    if total > available {
        // Start at min-word, distribute the remainder by grow potential.
        widths = min_word.clone();
        let mut used: usize = widths.iter().sum();
        let mut fixup = 0;
        while used < available && fixup < 1000 {
            let mut progressed = false;
            for c in 0..cols {
                if widths[c] < natural[c] && used < available {
                    widths[c] += 1;
                    used += 1;
                    progressed = true;
                }
            }
            fixup += 1;
            if !progressed {
                break;
            }
        }
        // If min-words alone overflow, shrink proportionally.
        if used > available {
            let mut over = used - available;
            while over > 0 {
                let mut progressed = false;
                for w in widths.iter_mut() {
                    if over == 0 {
                        break;
                    }
                    if *w > 1 {
                        *w -= 1;
                        over -= 1;
                        progressed = true;
                    }
                }
                if !progressed {
                    break;
                }
            }
        }
    }

    let border = |left: &str, mid: &str, right: &str| {
        let mut s = String::from(left);
        for (c, w) in widths.iter().enumerate() {
            s.push_str(&"─".repeat(w + 2));
            s.push_str(if c + 1 == cols { right } else { mid });
        }
        (theme.code_block_border)(&s)
    };
    out.push(border("┌", "┬", "┐"));
    for (r, row) in rows.iter().enumerate() {
        // Wrap each cell to its column width, then emit as many lines as the
        // tallest cell needs (pi's `wrapCellText`). A long cell wraps instead
        // of being truncated.
        let cells: Vec<Vec<String>> = (0..cols)
            .map(|c| {
                let raw = row.get(c).map(String::as_str).unwrap_or("");
                let rendered = render_inline(raw, theme, &MarkdownOptions::default());
                let mut lines = wrap_text_with_ansi(&rendered, widths[c].max(1));
                if lines.is_empty() {
                    lines.push(String::new());
                }
                lines
            })
            .collect();
        let height = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for line in 0..height {
            let parts: Vec<String> = cells
                .iter()
                .enumerate()
                .map(|(c, cell)| {
                    pad_or_truncate(cell.get(line).map(String::as_str).unwrap_or(""), widths[c])
                })
                .collect();
            out.push(format!("│ {} │", parts.join(" │ ")));
        }
        // pi draws a separator after every row except the last.
        if r + 1 < rows.len() {
            out.push(border("├", "┼", "┤"));
        }
    }
    out.push(border("└", "┴", "┘"));
}

fn pad_or_truncate(text: &str, width: usize) -> String {
    let w = visible_width(text);
    if w > width {
        truncate_to_width(text, width, "…", false)
    } else {
        format!("{text}{}", " ".repeat(width - w))
    }
}

/// Render inline spans (bold, italic, code, strike, links) for one line.
pub fn render_inline(text: &str, theme: &MarkdownTheme, options: &MarkdownOptions) -> String {
    let mut out = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`'
            && let Some(end) = find_char(&chars, i + 1, '`')
            && end > i + 1
        {
            let code: String = chars[i + 1..end].iter().collect();
            out.push_str(&(theme.code)(&code));
            i = end + 1;
            continue;
        }
        if chars[i] == '*'
            && i + 1 < chars.len()
            && chars[i + 1] == '*'
            && let Some(end) = find_str(&chars, i + 2, &['*', '*'])
        {
            let inner: String = chars[i + 2..end].iter().collect();
            out.push_str(&(theme.bold)(&render_inline(&inner, theme, options)));
            i = end + 2;
            continue;
        }
        if chars[i] == '~'
            && i + 1 < chars.len()
            && chars[i + 1] == '~'
            && let Some(end) = find_str(&chars, i + 2, &['~', '~'])
        {
            let inner: String = chars[i + 2..end].iter().collect();
            out.push_str(&(theme.strike)(&inner));
            i = end + 2;
            continue;
        }
        if (chars[i] == '*' || chars[i] == '_')
            && let Some(end) = find_char(&chars, i + 1, chars[i])
            && end > i + 1
        {
            let inner: String = chars[i + 1..end].iter().collect();
            out.push_str(&(theme.italic)(&render_inline(&inner, theme, options)));
            i = end + 1;
            continue;
        }
        if chars[i] == '['
            && let Some(close) = find_char(&chars, i + 1, ']')
            && chars.get(close + 1) == Some(&'(')
            && let Some(paren) = find_char(&chars, close + 2, ')')
        {
            let label: String = chars[i + 1..close].iter().collect();
            let url: String = chars[close + 2..paren].iter().collect();
            out.push_str(&render_link(&label, &url, theme, options));
            i = paren + 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn render_link(label: &str, url: &str, theme: &MarkdownTheme, options: &MarkdownOptions) -> String {
    let styled = (theme.link)(label);
    match options.link_mode {
        LinkMode::Hyperlink => format!("\x1b]8;;{url}\x07{styled}\x1b]8;;\x07"),
        LinkMode::Inline => {
            if label == url || url.strip_prefix("mailto:") == Some(label) {
                styled
            } else {
                format!("{styled} ({url})")
            }
        }
    }
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&j| chars[j] == target)
}

fn find_str(chars: &[char], from: usize, target: &[char; 2]) -> Option<usize> {
    (from..chars.len().saturating_sub(1))
        .find(|&j| chars[j] == target[0] && chars[j + 1] == target[1])
}

/// A markdown component wrapper.
pub struct Markdown {
    text: String,
    theme: MarkdownTheme,
    options: MarkdownOptions,
}

impl Markdown {
    /// A new markdown component.
    pub fn new(text: impl Into<String>, theme: MarkdownTheme, options: MarkdownOptions) -> Self {
        Self {
            text: text.into(),
            theme,
            options,
        }
    }

    /// Replace the source text (streaming updates).
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }

    /// Render at a width.
    pub fn render(&self, width: usize) -> Vec<String> {
        render_markdown(&self.text, width, &self.theme, &self.options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> MarkdownTheme {
        MarkdownTheme::default()
    }

    fn strip(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|l| crate::engine::text::strip_terminal_sequences(l))
            .collect()
    }

    #[test]
    fn headings_and_paragraphs() {
        let out = strip(&render_markdown(
            "# Title\n\nsome text",
            40,
            &plain(),
            &MarkdownOptions::default(),
        ));
        assert_eq!(out[0], "Title");
        assert!(out.iter().any(|l| l.contains("some text")));
    }

    // pi: h1 = heading(bold(underline(text))), h2 = heading(bold(text)).
    #[test]
    fn the_level_one_heading_is_underlined() {
        let theme = MarkdownTheme {
            heading: Arc::new(|s| format!("[36m{s}[0m")),
            bold: Arc::new(|s| format!("[1m{s}[0m")),
            underline: Arc::new(|s| format!("[4m{s}[0m")),
            ..Default::default()
        };
        let h1 = render_markdown("# Title", 40, &theme, &MarkdownOptions::default());
        assert!(h1[0].contains("\x1b[4m"), "h1 underlines: {:?}", h1[0]);
        let h2 = render_markdown("## Title", 40, &theme, &MarkdownOptions::default());
        assert!(!h2[0].contains("\x1b[4m"), "h2 does not: {:?}", h2[0]);
    }

    #[test]
    fn lists_get_bullets_and_continuation_indent() {
        let out = strip(&render_markdown(
            "- first item\n- second item that is quite long and wraps around the width",
            20,
            &plain(),
            &MarkdownOptions::default(),
        ));
        assert!(out[0].starts_with("- first"), "{:?}", out[0]);
        assert!(out[1].starts_with("- second"), "{:?}", out[1]);
        assert!(out[2].starts_with("  ")); // continuation aligns under the text
    }

    #[test]
    fn code_blocks_are_framed() {
        let out = strip(&render_markdown(
            "```rust\nlet x = 1;\n```",
            30,
            &plain(),
            &MarkdownOptions::default(),
        ));
        assert!(out[0].starts_with('╭'));
        assert!(out.iter().any(|l| l.starts_with("│ let x = 1;")));
        assert!(out.last().unwrap().starts_with('╰'));
    }

    #[test]
    fn tables_render_aligned_columns() {
        let md = "| name | value |\n| --- | --- |\n| a | 1 |\n| longer | 22 |";
        let out = strip(&render_markdown(
            md,
            40,
            &plain(),
            &MarkdownOptions::default(),
        ));
        assert!(out[0].starts_with('┌'));
        // Every body row has the same visible width.
        let widths: Vec<usize> = out
            .iter()
            .filter(|l| l.starts_with('│'))
            .map(|l| visible_width(l))
            .collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn inline_styles_apply() {
        let theme = MarkdownTheme {
            bold: Arc::new(|s| format!("\x1b[1m{s}\x1b[0m")),
            code: Arc::new(|s| format!("\x1b[36m{s}\x1b[0m")),
            ..Default::default()
        };
        let out = render_inline("a **b** `c`", &theme, &MarkdownOptions::default());
        assert!(out.contains("\x1b[1mb\x1b[0m"));
        assert!(out.contains("\x1b[36mc\x1b[0m"));
    }

    #[test]
    fn links_use_osc8_or_inline_fallback() {
        let theme = plain();
        let hyper = render_inline(
            "[site](http://x)",
            &theme,
            &MarkdownOptions {
                link_mode: LinkMode::Hyperlink,
                ..Default::default()
            },
        );
        assert!(hyper.contains("\x1b]8;;http://x\x07"));
        assert!(!hyper.contains("(http"));
        let inline = render_inline(
            "[site](http://x)",
            &theme,
            &MarkdownOptions {
                link_mode: LinkMode::Inline,
                ..Default::default()
            },
        );
        assert!(inline.contains("site (http://x)"));
    }

    #[test]
    fn partial_closing_fence_is_trimmed_during_streaming() {
        let out = strip(&render_markdown(
            "```\ncode line\n``",
            20,
            &plain(),
            &MarkdownOptions::default(),
        ));
        assert!(out.iter().any(|l| l.contains("code line")));
        assert!(!out.iter().any(|l| l.contains("``")));
    }

    #[test]
    fn html_is_rendered_as_literal_text() {
        let out = render_inline("<b>hi</b>", &plain(), &MarkdownOptions::default());
        assert_eq!(
            crate::engine::text::strip_terminal_sequences(&out),
            "<b>hi</b>"
        );
    }

    #[test]
    fn wide_content_never_exceeds_width() {
        let out = render_markdown(
            "# a very long heading that should wrap nicely across lines",
            12,
            &plain(),
            &MarkdownOptions::default(),
        );
        assert!(out.iter().all(|l| visible_width(l) <= 12));
    }

    // Verifies: FR-UI-8 - an unterminated fence draws a complete frame, so
    // the block never appears half-drawn while streaming (pi's behavior).
    #[test]
    fn an_unterminated_fence_draws_a_complete_frame() {
        let out = strip(&render_markdown(
            "```rust\nfn main() {",
            40,
            &plain(),
            &MarkdownOptions::default(),
        ));
        assert!(out[0].starts_with('╭'));
        assert!(out.last().unwrap().starts_with('╰'));
        assert!(out.iter().any(|l| l.contains("fn main() {")));
    }

    // Verifies: FR-UI-8 - a half-written opening fence is plain text.
    #[test]
    fn a_partial_opening_fence_is_plain_text() {
        let out = strip(&render_markdown(
            "``",
            40,
            &plain(),
            &MarkdownOptions::default(),
        ));
        assert!(out.iter().any(|l| l.contains("``")));
        assert!(!out.iter().any(|l| l.starts_with('╭')));
    }

    // Verifies: FR-UI-7 - the transcript renders markdown: headings, lists,
    // tables with aligned columns, framed code blocks, and links.
    #[test]
    fn markdown_covers_the_transcript_vocabulary() {
        let raw = render_markdown(GOLDEN, 60, &plain(), &MarkdownOptions::default());
        let out = strip(&raw);
        assert!(out.iter().any(|l| l == "Title"), "heading");
        assert!(out.iter().any(|l| l.starts_with("- one")), "list");
        assert!(out.iter().any(|l| l.starts_with('┌')), "table");
        assert!(out.iter().any(|l| l.starts_with('╭')), "framed code");
        assert!(out.iter().any(|l| l.contains("quoted")), "quote");
        assert!(
            raw.iter().any(|l| l.contains("\x1b]8;;http://x\x07")),
            "link as OSC 8"
        );
    }

    /// A golden document with every construct the streaming renderer must
    /// tolerate.
    const GOLDEN: &str = "# Title\n\nIntro with **bold**, `code`, and a [link](http://x).\n\n\
        - one\n- two\n\n> quoted\n\n\
        | name | value |\n| --- | --- |\n| alpha | 1 |\n\n\
        ```rust\nfn main() {\n    let x = 1;\n}\n```\n\n\
        Math $x^2$ and $$E=mc^2$$ stay literal.\n";

    fn frame_count(lines: &[String], open: char) -> usize {
        lines
            .iter()
            .filter(|l| {
                crate::engine::text::strip_terminal_sequences(l)
                    .trim_start()
                    .starts_with(open)
            })
            .count()
    }

    // Verifies: FR-UI-8 - streaming tolerance. For every prefix of a golden
    // document, rendering neither panics nor emits an unterminated frame,
    // and a table never appears without its intact separator row.
    #[test]
    fn every_prefix_renders_without_an_unterminated_frame() {
        for width in [40usize, 80] {
            for end in 0..=GOLDEN.len() {
                if !GOLDEN.is_char_boundary(end) {
                    continue;
                }
                let prefix = &GOLDEN[..end];
                let lines = render_markdown(prefix, width, &plain(), &MarkdownOptions::default());
                let code_open = frame_count(&lines, '╭');
                let code_close = frame_count(&lines, '╰');
                assert_eq!(
                    code_open, code_close,
                    "unbalanced code frame at width {width}, prefix ending {end}\n{lines:?}"
                );
                let table_open = frame_count(&lines, '┌');
                let table_close = frame_count(&lines, '└');
                assert_eq!(
                    table_open, table_close,
                    "unbalanced table frame at width {width}, prefix ending {end}\n{lines:?}"
                );
                if !prefix.lines().any(is_table_separator) {
                    assert_eq!(
                        table_open, 0,
                        "a table rendered without an intact separator at prefix ending {end}\n{lines:?}"
                    );
                }
                assert!(
                    lines.iter().all(|l| visible_width(l) <= width),
                    "a prefix exceeded the width at {width}, ending {end}"
                );
            }
        }
    }

    // Verifies: FR-UI-8 - a half-written separator row renders as text.
    #[test]
    fn a_partial_separator_row_is_not_yet_a_table() {
        let out = strip(&render_markdown(
            "| name | value |\n| --- | ---",
            40,
            &plain(),
            &MarkdownOptions::default(),
        ));
        assert!(!out.iter().any(|l| l.starts_with('┌')), "{out:?}");
    }

    // Verifies: FR-UI-8 - unpaired inline markers and math render literally.
    #[test]
    fn unpaired_inline_markers_and_math_stay_literal() {
        let out = render_inline(
            "a **b and $x^2$ and `",
            &plain(),
            &MarkdownOptions::default(),
        );
        let stripped = crate::engine::text::strip_terminal_sequences(&out);
        assert!(stripped.contains("**b"), "{stripped}");
        assert!(stripped.contains("$x^2$"), "{stripped}");
        assert!(stripped.ends_with('`'), "{stripped}");
    }

    // Verifies: FR-UI-8 / D10 - the code-block border caps at the content
    // width, not the terminal width.
    #[test]
    fn a_code_block_border_caps_at_the_content_width() {
        let out = strip(&render_markdown(
            "```\nx\n```",
            80,
            &plain(),
            &MarkdownOptions::default(),
        ));
        let top = visible_width(&out[0]);
        let bottom = visible_width(out.last().unwrap());
        assert_eq!(top, bottom);
        assert!(top < 80, "capped at the content width, got {top}");
    }

    // Verifies: D10/R16 - a code block is a full frame, not a half one.
    #[test]
    fn a_code_block_is_a_full_frame() {
        let out = strip(&render_markdown(
            "```python\nx = 1\n```",
            80,
            &plain(),
            &MarkdownOptions::default(),
        ));
        assert!(
            out[0].starts_with('╭') && out[0].ends_with('╮'),
            "{:?}",
            out[0]
        );
        assert!(out[0].contains("python"), "{:?}", out[0]);
        assert!(
            out[1].starts_with('│') && out[1].ends_with('│'),
            "{:?}",
            out[1]
        );
        let bottom = out.last().expect("a bottom");
        assert!(
            bottom.starts_with('╰') && bottom.ends_with('╯'),
            "{bottom:?}"
        );
    }
}
