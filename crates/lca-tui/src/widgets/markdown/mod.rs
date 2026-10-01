//! Streaming-tolerant terminal markdown, ported from pi's
//! `packages/tui/src/components/markdown.ts`
//! (`pi-tui-re/src_re/tui-widgets/markdown.md`).
//!
//! This is the answer to owner issue #6 ("the chat history is hard to
//! read"): headings, lists, tables rendered as aligned columns, framed code
//! blocks, blockquotes, and inline styles.
//!
//! Not ported (documented skips): LaTeX math (the brief says skip it) and
//! mermaid (pi shells out). **Syntax highlighting closed in cycle 9 (R3):**
//! pi's markdown calls `theme.highlightCode` (`markdown.ts` §523), so a
//! fenced block now paints the theme's nine `Syntax*` roles through the
//! [`MarkdownTheme::highlight`] hook - the same seam pi puts it on, because
//! highlighting belongs to the theme, not to this widget. A fence with no
//! language, or one the port does not know, keeps `code_block`, which is
//! pi's own unknown-language fallback; the grammars are hand-written in
//! `lca-ui`'s `theme/highlight.rs`, so ADR-0036's no-new-dependency rule
//! still holds. HTML is rendered as literal text by construction - no
//! markup reaches the terminal (the hostile-input stance).
//!
//! The tokenizer is hand-written and line-based rather than `pulldown-cmark`
//! (ADR-0036 adds no such dependency). It covers the block and inline shapes
//! a coding transcript uses; `ponytail:` upgrade to `pulldown-cmark` if a
//! real document needs constructs this misses.

use std::sync::Arc;

use crate::engine::text::{truncate_to_width, visible_width, wrap_text_with_ansi};

/// A styling function.
pub type StyleFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The syntax-highlighting hook (pi's `theme.highlightCode`).
pub type HighlightFn = Arc<dyn Fn(&str, &str) -> Option<Vec<String>> + Send + Sync>;

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
    /// Syntax highlighting for a fenced block (pi's `theme.highlightCode`):
    /// `(code, language)` to styled lines, or `None` when the language is
    /// one it does not know - the caller then paints every line with
    /// [`Self::code_block`], which is pi's own unknown-language fallback.
    pub highlight: Option<HighlightFn>,
    /// Link style.
    pub link: StyleFn,
    /// The link's URL when it prints inline (pi's `linkUrl`).
    pub link_url: StyleFn,
    /// List bullet style (pi's `listBullet`).
    pub list_bullet: StyleFn,
    /// Blockquote style (pi's `quote`).
    pub quote: StyleFn,
    /// Blockquote border style (pi's `quoteBorder`).
    pub quote_border: StyleFn,
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
            highlight: None,
            link: Arc::new(identity),
            link_url: Arc::new(identity),
            list_bullet: Arc::new(identity),
            quote: Arc::new(identity),
            quote_border: Arc::new(identity),
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
            // pi (markdown.md §4): a heading runs its *inline* tokens
            // inside the heading style - h1 is heading(bold(underline(…))),
            // h2 heading(bold(…)), and h3+ prepends a styled literal prefix.
            // The style is re-armed after every nested reset (§3's
            // style-prefix trick), so an inline code span in a heading
            // cannot desaturate the rest of the line.
            let inner = rearm(
                &render_inline(&content, theme, options),
                &style_prefix(&theme.heading),
            );
            let decorate = |text: &str| match level {
                1 => (theme.heading)(&(theme.bold)(&(theme.underline)(text))),
                _ => (theme.heading)(&(theme.bold)(text)),
            };
            let mut styled = decorate(&inner);
            if level >= 3 {
                styled = format!("{}{}", decorate(&format!("{} ", "#".repeat(level))), styled);
            }
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
                // pi (markdown.md §4): the quote's *text* is quote+italic
                // with the style re-armed after every nested reset; only
                // the border character takes `mdQuoteBorder`.
                let rendered = render_inline(line, theme, options);
                let quote_style = |text: &str| (theme.quote)(&(theme.italic)(text));
                let prefix = format!(
                    "{}{}",
                    style_prefix(&theme.quote),
                    style_prefix(&theme.italic)
                );
                let styled = quote_style(&rearm(&rendered, &prefix));
                let border = (theme.quote_border)("│ ");
                for wrapped in wrap_text_with_ansi(&styled, inner_width.saturating_sub(2)) {
                    out.push(format!("{border}{wrapped}"));
                }
            }
            continue;
        }

        if let Some((kind, content, indent)) = list_item(trimmed) {
            let mut marker = match kind {
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
            // pi colors the bullet with `mdListBullet`; an ordered marker
            // is a number, not a bullet, and keeps the default.
            if matches!(kind, ListMarker::Bullet { .. }) {
                marker = (theme.list_bullet)(&marker);
            }
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
    // pi's order: highlight when the theme can (and the fence named a
    // language it knows), otherwise paint the whole block `mdCodeBlock`.
    let fallback = || {
        body.iter()
            .map(|line| (theme.code_block)(line))
            .collect::<Vec<String>>()
    };
    let rendered = theme
        .highlight
        .as_ref()
        .and_then(|highlight| highlight(&body.join("\n"), lang))
        .unwrap_or_else(fallback);
    for line in rendered {
        let pad = inner.saturating_sub(visible_width(&line));
        out.push(format!("│ {line}{} │", " ".repeat(pad)));
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
            // pi bolds the header cells and only the header cells
            // (`markdown.ts` renderTable: `theme.bold(padded)` per cell) -
            // the border characters stay unstyled.
            let joined = if r == 0 {
                parts
                    .iter()
                    .map(|part| (theme.bold)(part))
                    .collect::<Vec<String>>()
                    .join(" │ ")
            } else {
                parts.join(" │ ")
            };
            out.push(format!("│ {joined} │"));
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

/// The pure open prefix a style function emits: render a sentinel and keep
/// everything before it (pi's `getStylePrefix`, markdown.md §3) - what lets
/// a nested style re-arm the one around it.
fn style_prefix(style: &StyleFn) -> String {
    const SENTINEL: &str = "\u{0}";
    style(SENTINEL)
        .split(SENTINEL)
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Re-arm an enclosing style after every nested reset that would kill it:
/// a nested color closes with `39` (or a full `0m`), which would otherwise
/// leave the rest of the line in the body color (markdown.md §3).
fn rearm(text: &str, prefix: &str) -> String {
    if prefix.is_empty() || text.is_empty() {
        return text.to_string();
    }
    text.replace("\x1b[0m", &format!("\x1b[0m{prefix}"))
        .replace("\x1b[39m", &format!("\x1b[39m{prefix}"))
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
                format!("{styled} {}", (theme.link_url)(&format!("({url})")))
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
mod tests;
