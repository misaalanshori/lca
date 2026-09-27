//! Streaming-tolerant terminal markdown, ported from pi's
//! `packages/tui/src/components/markdown.ts`
//! (`pi-tui-re/src_re/tui-widgets/markdown.md`).
//!
//! This is the answer to owner issue #6 ("the chat history is hard to
//! read"): headings, lists, tables rendered as aligned columns, framed code
//! blocks, blockquotes, and inline styles.
//!
//! Not ported (documented skips): LaTeX math (the brief says skip it),
//! mermaid (pi shells out), and syntax highlighting (a theme hook in pi;
//! LCA's default is none). HTML is rendered as literal text by construction
//! — no markup reaches the terminal (the hostile-input stance).
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
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
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
            let styled = match level {
                1 => (theme.heading)(&(theme.bold)(&content)),
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

        if let Some((marker, content, indent)) = list_item(trimmed) {
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

fn list_item(line: &str) -> Option<(String, String, usize)> {
    let indent = line.len() - line.trim_start().len();
    let rest = line.trim_start();
    for bullet in ["- ", "* ", "+ "] {
        if let Some(content) = rest.strip_prefix(bullet) {
            let task = content
                .strip_prefix("[x] ")
                .or_else(|| content.strip_prefix("[X] "))
                .map(|c| format!("☑ {c}"))
                .or_else(|| content.strip_prefix("[ ] ").map(|c| format!("☐ {c}")));
            let content = task.unwrap_or_else(|| content.to_string());
            return Some(("• ".to_string(), content, indent));
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
            let marker = format!(
                "{}{} ",
                digits,
                if after.starts_with('.') { "." } else { ")" }
            );
            return Some((marker, content.to_string(), indent));
        }
    }
    None
}

fn is_table_start(lines: &[&str], i: usize) -> bool {
    if i + 1 >= lines.len() {
        return false;
    }
    lines[i].trim_start().starts_with('|')
        && lines[i + 1].contains('-')
        && lines[i + 1].trim_start().starts_with('|')
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
    while i < lines.len() && lines[i].trim_start().starts_with('|') {
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
    let inner = width.saturating_sub(4).max(1);
    let title = if lang.is_empty() {
        String::new()
    } else {
        format!(" {lang} ")
    };
    let border = (theme.code_block_border)(&format!(
        "╭─{title}{}",
        "─".repeat(
            inner
                .saturating_sub(visible_width(&title))
                .saturating_add(2)
        )
    ));
    out.push(truncate_to_width(&border, width, "", false));
    for line in body {
        let styled = (theme.code_block)(line);
        out.push(format!("│ {styled}"));
    }
    out.push((theme.code_block_border)(&format!(
        "╰{}",
        "─".repeat(width.saturating_sub(1))
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
        let cells: Vec<String> = (0..cols)
            .map(|c| {
                let raw = row.get(c).map(String::as_str).unwrap_or("");
                let rendered = render_inline(raw, theme, &MarkdownOptions::default());
                pad_or_truncate(&rendered, widths[c])
            })
            .collect();
        out.push(format!("│ {} │", cells.join(" │ ")));
        if r == 0 {
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

    #[test]
    fn lists_get_bullets_and_continuation_indent() {
        let out = strip(&render_markdown(
            "- first item\n- second item that is quite long and wraps around the width",
            20,
            &plain(),
            &MarkdownOptions::default(),
        ));
        assert!(out[0].starts_with("• first"));
        assert!(out[1].starts_with("• second"));
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
}
