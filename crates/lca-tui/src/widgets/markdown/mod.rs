//! Streaming-tolerant terminal markdown, ported from pi's
//! `packages/tui/src/components/markdown.ts`
//! (`pi-tui-re/src_re/tui-widgets/markdown.md`).
//!
//! This is the answer to owner issue #6 ("the chat history is hard to
//! read"): headings, lists, tables rendered as aligned columns, framed code
//! blocks, blockquotes, and inline styles.
//!
//! **Math ships with TUI-10 (M3):** inline `$…$`/`\(…\)` and display
//! `$$…$$`/`\[…\]` render through the `latex` widget, with pi's
//! pending-raw-until-closed streaming rule and its fail-soft contract
//! (unsupported input prints as source). **Mermaid ships with TUI-10
//! (M4):** a top-level ```` ```mermaid ```` fence renders as Unicode art
//! from the `mermaid` widget - the flowchart and sequence subset, since
//! `grok-mermaid` itself is an npm package this tree cannot take - with
//! pi's width guard and warning note. **Syntax highlighting closed in
//! cycle 9 (R3):**
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

mod math;

/// A styling function.
pub type StyleFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The syntax-highlighting hook (pi's `theme.highlightCode`).
pub type HighlightFn = Arc<dyn Fn(&str, &str) -> Option<Vec<String>> + Send + Sync>;

mod options;
mod transform;

pub use options::MarkdownOptions;
pub use transform::{
    MarkdownMessageType, MarkdownTransformContext, MarkdownTransformer, apply_transformers,
};

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
    /// Mermaid diagram borders (pi's `borderMuted`).
    pub mermaid_border: StyleFn,
    /// Mermaid node text (pi's `text`).
    pub mermaid_text: StyleFn,
    /// Mermaid arrows and connectors (pi's `accent`).
    pub mermaid_edge: StyleFn,
    /// Mermaid edge labels (pi's `muted`).
    pub mermaid_edge_label: StyleFn,
    /// Mermaid diagram title (pi's `accent` + bold).
    pub mermaid_title: StyleFn,
    /// The warning note under an unrendered diagram (pi's `warning`).
    pub warning: StyleFn,
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
            mermaid_border: Arc::new(identity),
            mermaid_text: Arc::new(identity),
            mermaid_edge: Arc::new(identity),
            mermaid_edge_label: Arc::new(identity),
            mermaid_title: Arc::new(identity),
            warning: Arc::new(identity),
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

/// How a fenced code block is framed (gh #32): the shipped four-sided
/// frame, bars only so a terminal selection copies the code clean, or
/// nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeBlockBorder {
    /// The four-sided frame with side pipes - the shipped look.
    #[default]
    Full,
    /// Top and bottom bars only: no side pipes to clean up after a
    /// copy-paste (the reason issue #32 exists).
    Horizontal,
    /// No bars, no pipes: the code exactly as the fence wrote it.
    None,
}

impl std::str::FromStr for CodeBlockBorder {
    type Err = String;

    /// Parse a configured shape (`markdown.codeblock_border`).
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "full" => Ok(Self::Full),
            "horizontal" => Ok(Self::Horizontal),
            "none" => Ok(Self::None),
            other => Err(format!("expected full, horizontal, or none, got `{other}`")),
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
    // gh #12: the registered transforms run first, over the raw source
    // in registration order; each sees the previous one's output, and a
    // transform that panics behaves as identity (pi's try/catch).
    let context = MarkdownTransformContext {
        message_type: options.message_type,
        is_streaming: options.streaming,
        available_width: width,
    };
    let transformed = apply_transformers(text, &context, &options.transformers);
    let inner_width = width.saturating_sub(options.padding_x * 2).max(1);
    let expanded = transformed.replace('\t', "   ");
    let normalized = expanded.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();

    let mut out: Vec<String> = Vec::new();
    for _ in 0..options.padding_y {
        out.push(String::new());
    }

    render_blocks(&lines, inner_width, theme, options, &mut out);

    // Apply horizontal padding.
    if options.padding_x > 0 {
        let pad = " ".repeat(options.padding_x);
        out = out.into_iter().map(|l| format!("{pad}{l}")).collect();
    }
    for _ in 0..options.padding_y {
        out.push(String::new());
    }
    // RC-C (issue #5): Close all active SGR styles and OSC 8 links at every non-empty line end
    // when formatting was present, so open formatting never leaks into subsequent rows or terminal history.
    for line in out.iter_mut() {
        if line.contains('\x1b') {
            line.push_str(FULL_LINE_RESET);
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// One pass over a block sequence: every top-level branch of the
/// renderer. A blockquote re-invokes this on its children, which is how
/// pi renders quote bodies as blocks (`markdown.md` §4, `renderToken`'s
/// `blockquote` case) rather than as inline text.
fn render_blocks(
    lines: &[&str],
    inner_width: usize,
    theme: &MarkdownTheme,
    options: &MarkdownOptions,
    out: &mut Vec<String>,
) {
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
            // Mermaid: Unicode art behind the fence (TUI-10 M4). pi runs
            // this through a markdown *transformer* that re-encodes rows as
            // code spans for a second parse pass; LCA renders the art
            // directly, so the fence-length trick that protects diagram
            // rows from markdown has nothing to do here. The fence scan
            // above has already consumed the block, so `i` is past it.
            if lang.eq_ignore_ascii_case("mermaid")
                && let Some(art) = crate::widgets::mermaid::render(&body.join("\n"))
                && art.width <= inner_width
            {
                if !art.warnings.is_empty() && !options.streaming {
                    // pi keeps the raw source and appends a styled warning
                    // note (only outside streaming).
                    render_code_block(
                        &body,
                        &lang,
                        inner_width,
                        theme,
                        options.codeblock_border,
                        out,
                    );
                    out.push((theme.warning)(&format!(
                        "Mermaid diagram not rendered: {}",
                        art.warnings[0]
                    )));
                    if art.warnings.len() > 1 {
                        out.push((theme.warning)(&format!(
                            "(+{} more)",
                            art.warnings.len() - 1
                        )));
                    }
                } else {
                    for row in &art.lines {
                        out.push(style_mermaid_row(row, theme));
                    }
                }
                if out.last().is_some_and(|line| !line.is_empty()) {
                    out.push(String::new());
                }
                continue;
            }
            render_code_block(
                &body,
                &lang,
                inner_width,
                theme,
                options.codeblock_border,
                out,
            );
            continue;
        }

        // Display math: `$$…$$` / `\[…\]`, pending forms printing the
        // raw source until a closer arrives (pi's `latexBlock` token).
        if let Some(block) = math::block_math(lines, i) {
            ordered_next = None;
            let raw: Vec<String> = lines[i..i + block.consumed]
                .iter()
                .map(|line| (*line).to_string())
                .collect();
            let body = if block.pending || !options.render_latex {
                raw
            } else {
                crate::widgets::latex::render_latex(&block.content, true)
                    .map(|rendered| {
                        rendered
                            .split('\n')
                            .map(str::to_string)
                            .collect::<Vec<String>>()
                    })
                    .unwrap_or(raw)
            };
            out.extend(body);
            i += block.consumed;
            if out.last().is_some_and(|line| !line.is_empty()) {
                out.push(String::new());
            }
            continue;
        }

        if is_table_start(lines, i) {
            let (table, consumed) = collect_table(lines, i);
            render_table(&table, &lines[i..i + consumed], inner_width, theme, out);
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
            // One `>` level per pass: the children re-enter as blocks, so a
            // list, heading, or code fence inside a quote renders as what it
            // is (pi's recursive blockquote case in `renderToken`), and `>>`
            // nests a second level instead of flattening.
            let mut body: Vec<&str> = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let rest = lines[i].trim_start().strip_prefix('>').unwrap_or(" ");
                body.push(rest.strip_prefix(' ').unwrap_or(rest));
                i += 1;
            }
            let mut children = Vec::new();
            render_blocks(
                &body,
                inner_width.saturating_sub(2),
                theme,
                options,
                &mut children,
            );
            // pi drops the trailing blank lines before the quote's own
            // spacing rule runs.
            while children.last().is_some_and(|line| line.is_empty()) {
                children.pop();
            }
            // pi (markdown.md §4): every child line is dressed in quote+italic
            // with the style re-armed after nested resets; only the border
            // character takes `mdQuoteBorder`.
            let quote_style = |text: &str| (theme.quote)(&(theme.italic)(text));
            let prefix = format!(
                "{}{}",
                style_prefix(&theme.quote),
                style_prefix(&theme.italic)
            );
            let border = (theme.quote_border)("│ ");
            for line in children {
                let styled = quote_style(&rearm(&line, &prefix));
                for wrapped in wrap_text_with_ansi(&styled, inner_width.saturating_sub(2)) {
                    out.push(format!("{border}{wrapped}"));
                }
            }
            continue;
        }

        if let Some((kind, content, indent)) = list_item(trimmed) {
            // pi's two modes: the default renumbers an ordered run from its
            // start and uses `- ` for every unordered bullet; with
            // `preserveOrderedListMarkers` the authored marker (`1.` vs `1)`,
            // `-`/`+`/`*`) is what prints (`markdown.ts` renderList).
            let mut marker = match kind {
                ListMarker::Bullet { task, sym } => {
                    let bullet = if options.preserve_ordered_list_markers {
                        format!("{sym} ")
                    } else {
                        "- ".to_string()
                    };
                    match task {
                        Some(true) => format!("{bullet}[x] "),
                        Some(false) => format!("{bullet}[ ] "),
                        None => bullet,
                    }
                }
                ListMarker::Ordered { start, delim } => {
                    if options.preserve_ordered_list_markers {
                        format!("{start}{delim} ")
                    } else {
                        let n = *ordered_next.get_or_insert(start);
                        ordered_next = Some(n + 1);
                        format!("{n}. ")
                    }
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

        // pi keeps counting an ordered run across the blank lines that make
        // it a *loose* list; only a real non-list line ends the run.
        if trimmed.is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            i += 1;
            continue;
        }

        ordered_next = None;

        // Paragraph.
        let rendered = render_inline(trimmed, theme, options);
        out.extend(wrap_text_with_ansi(&rendered, inner_width));
        i += 1;
    }
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
    /// An unordered item: its task state and the authored bullet char.
    Bullet { task: Option<bool>, sym: char },
    /// An ordered item: the number the run starts at and the authored
    /// delimiter (`.` or `)`), which `preserveOrderedListMarkers` keeps.
    Ordered { start: u64, delim: char },
}

fn list_item(line: &str) -> Option<(ListMarker, String, usize)> {
    let indent = line.len() - line.trim_start().len();
    let rest = line.trim_start();
    for (sym, bullet) in [('-', "- "), ('*', "* "), ('+', "+ ")] {
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
            return Some((ListMarker::Bullet { task, sym }, content, indent));
        }
    }
    // Ordered: N. or N)
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &rest[digits.len()..];
        let delim = after.chars().next()?;
        if (delim == '.' || delim == ')')
            && let Some(content) = after[1..].strip_prefix(' ')
        {
            let start = digits.parse::<u64>().unwrap_or(1);
            return Some((
                ListMarker::Ordered { start, delim },
                content.to_string(),
                indent,
            ));
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
    border: CodeBlockBorder,
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
    if border == CodeBlockBorder::None {
        out.extend(rendered(body, lang, theme));
        return;
    }
    if border == CodeBlockBorder::Horizontal {
        // gh #32: bars top and bottom, code lines bare - a terminal
        // selection copies exactly what the fence wrote.
        let top_fill = frame.saturating_sub(visible_width(&title) + 2);
        out.push((theme.code_block_border)(&format!(
            "──{title}{}",
            "─".repeat(top_fill)
        )));
        out.extend(rendered(body, lang, theme));
        out.push((theme.code_block_border)(&"─".repeat(frame)));
        return;
    }
    let top_fill = frame.saturating_sub(visible_width(&title) + 3);
    out.push((theme.code_block_border)(&format!(
        "╭─{title}{}╮",
        "─".repeat(top_fill)
    )));
    for line in rendered(body, lang, theme) {
        let pad = inner.saturating_sub(visible_width(&line));
        out.push(format!("│ {line}{} │", " ".repeat(pad)));
    }
    out.push((theme.code_block_border)(&format!(
        "╰{}╯",
        "─".repeat(frame.saturating_sub(2))
    )));
}

/// The code block's lines, styled: pi's order - highlight when the theme
/// can (and the fence named a language it knows), otherwise paint every
/// line `mdCodeBlock`. Shared by the three frame shapes so they can only
/// ever differ in what surrounds the code.
fn rendered(body: &[String], lang: &str, theme: &MarkdownTheme) -> Vec<String> {
    let fallback = || {
        body.iter()
            .map(|line| (theme.code_block)(line))
            .collect::<Vec<String>>()
    };
    theme
        .highlight
        .as_ref()
        .and_then(|highlight| highlight(&body.join("\n"), lang))
        .unwrap_or_else(fallback)
}

fn render_table(
    rows: &[Vec<String>],
    raw: &[&str],
    width: usize,
    theme: &MarkdownTheme,
    out: &mut Vec<String>,
) {
    #[allow(clippy::needless_range_loop)] // columns index parallel width vectors
    if rows.is_empty() {
        return;
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    // Border overhead is 3n+1; too narrow to be stable -> pi falls back to
    // the raw markdown source, wrapped.
    if width < cols * 3 + 1 {
        for line in raw {
            out.extend(wrap_text_with_ansi(line, width));
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
                // pi's wrapCellText: the narrow styles reset after every
                // non-final fragment, so a style inside a multi-line cell
                // cannot bleed into the next fragment or the padding.
                let fragments = lines.len();
                for line in lines.iter_mut().take(fragments.saturating_sub(1)) {
                    line.push_str("\x1b[22;23;24;25;27;28;29;39m");
                }
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

/// Full line-end reset: resets all text styles and foreground color,
/// plus OSC 8 hyperlink close, so unclosed spans never leak across line ends or repaints.
pub const FULL_LINE_RESET: &str = "\x1b[22;23;24;25;27;28;29;39m\x1b]8;;\x07";

/// Re-arm an enclosing style after every nested reset that would kill it:
/// a nested span or reset closes with `39`, `22`, `23`, `24`, `27`, `29`, or a full `0m`,
/// which would otherwise leave the rest of the line unstyled (markdown.md §3).
fn rearm(text: &str, prefix: &str) -> String {
    if prefix.is_empty() || text.is_empty() {
        return text.to_string();
    }
    let mut s = text
        .replace("\x1b[0m", &format!("\x1b[0m{prefix}"))
        .replace("\x1b[39m", &format!("\x1b[39m{prefix}"))
        .replace(FULL_LINE_RESET, &format!("{FULL_LINE_RESET}{prefix}"));
    // Also re-arm after the narrow multi-style reset if present
    s = s.replace(
        "\x1b[22;23;24;25;27;28;29;39m",
        &format!("\x1b[22;23;24;25;27;28;29;39m{prefix}"),
    );
    s
}

/// Dress one mermaid row in the theme's diagram roles (pi's `styleSpan`).
fn style_mermaid_row(row: &[crate::widgets::mermaid::Span], theme: &MarkdownTheme) -> String {
    use crate::widgets::mermaid::Class;
    row.iter()
        .map(|span| match span.class {
            Class::Border => (theme.mermaid_border)(&span.text),
            Class::Text => (theme.mermaid_text)(&span.text),
            Class::Edge => (theme.mermaid_edge)(&span.text),
            Class::EdgeLabel => (theme.mermaid_edge_label)(&span.text),
            Class::Title => (theme.mermaid_title)(&span.text),
            Class::None => span.text.clone(),
        })
        .collect()
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
        // pi's strict strikethrough: only a well-formed `~~text~~` strikes.
        if chars[i] == '~'
            && i + 1 < chars.len()
            && chars[i + 1] == '~'
            && let Some(end) = find_strict_strike(&chars, i + 2)
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
        // pi: an `image` token falls to the default inline case and prints
        // its alt text only - the URL never renders (the image itself is a
        // transcript entry, `widgets::image`).
        if chars[i] == '!'
            && chars.get(i + 1) == Some(&'[')
            && let Some(close) = find_char(&chars, i + 2, ']')
            && chars.get(close + 1) == Some(&'(')
            && let Some(paren) = find_char(&chars, close + 2, ')')
        {
            let alt: String = chars[i + 2..close].iter().collect();
            out.push_str(&alt);
            i = paren + 1;
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
        // Inline math: `$…$`, `\(…\)`, `\[…\]` with pi's guards, and
        // pending-raw behavior until the closer streams in (M3).
        if matches!(chars[i], '$' | '\\')
            && let Some(expr) = math::inline_math(&chars, i)
        {
            let raw: String = chars[i..i + expr.len].iter().collect();
            if expr.pending || !options.render_latex {
                out.push_str(&raw);
            } else {
                match crate::widgets::latex::render_math(
                    &expr.content,
                    crate::widgets::latex::MathMode::Inline,
                ) {
                    Some(rendered) => out.push_str(&rendered),
                    None => out.push_str(&raw),
                }
            }
            i += expr.len;
            continue;
        }
        // pi's `escape` token: the escaped character is what prints, unless
        // the caller asked to preserve the source form.
        if chars[i] == '\\'
            && let Some(&next) = chars.get(i + 1)
            && next.is_ascii_punctuation()
        {
            if options.preserve_backslash_escapes {
                out.push('\\');
            }
            out.push(next);
            i += 2;
            continue;
        }
        // GFM autolink literal: bare URLs and emails become links.
        if let Some((shown, href, len)) = autolink_at(&chars, i) {
            out.push_str(&render_link(&shown, &href, theme, options));
            i += len;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn render_link(label: &str, url: &str, theme: &MarkdownTheme, options: &MarkdownOptions) -> String {
    // pi: the label is `link(underline(text))`, OSC 8 closes with ST
    // (`\x1b\\`), and the fallback prints ` (url)` *inside* the linkUrl
    // style (`markdown.ts` §6).
    let styled = (theme.link)(&(theme.underline)(label));
    match options.link_mode {
        LinkMode::Hyperlink => format!("\x1b]8;;{url}\x1b\\{styled}\x1b]8;;\x1b\\"),
        LinkMode::Inline => {
            if label == url || url.strip_prefix("mailto:") == Some(label) {
                styled
            } else {
                format!("{styled}{}", (theme.link_url)(&format!(" ({url})")))
            }
        }
    }
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&j| chars[j] == target && !is_escaped(chars, j))
}

fn find_str(chars: &[char], from: usize, target: &[char; 2]) -> Option<usize> {
    (from..chars.len().saturating_sub(1))
        .find(|&j| chars[j] == target[0] && chars[j + 1] == target[1] && !is_escaped(chars, j))
}

/// Whether the character at `index` is backslash-escaped (pi's `isEscaped`:
/// an odd run of backslashes in front), which is what keeps `\*` from
/// opening an emphasis.
fn is_escaped(chars: &[char], index: usize) -> bool {
    let mut backslashes = 0;
    let mut j = index;
    while j > 0 && chars[j - 1] == '\\' {
        backslashes += 1;
        j -= 1;
    }
    backslashes % 2 == 1
}

/// pi's strict strikethrough (`STRICT_STRIKETHROUGH_REGEX`): `~~text~~`
/// only when the content neither starts nor ends with whitespace or a tilde,
/// escapes count as content, and the closing run is exactly two tildes -
/// so mid-prose tildes do not strike. Returns the closer's index.
fn find_strict_strike(chars: &[char], from: usize) -> Option<usize> {
    if from >= chars.len() || chars[from].is_whitespace() || chars[from] == '~' {
        return None;
    }
    let mut j = from;
    while j < chars.len() {
        if chars[j] == '\\' && j + 1 < chars.len() {
            j += 2;
            continue;
        }
        if chars[j] == '~' && chars.get(j + 1) == Some(&'~') {
            if chars.get(j + 2) == Some(&'~') {
                return None; // `~~~`: not a closer
            }
            let prev = chars.get(j.checked_sub(1)?)?;
            return if prev.is_whitespace() || *prev == '~' {
                None
            } else {
                Some(j)
            };
        }
        j += 1;
    }
    None
}

/// A GFM autolink literal (marked's default lexer behavior, so pi gets
/// these for free): a bare URL, a `www.` address, or an email becomes a
/// link. Returns (shown text, href, consumed length).
fn autolink_at(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    if i > 0 {
        let prev = chars[i - 1];
        if prev.is_alphanumeric() || matches!(prev, '.' | '/' | '@') {
            return None;
        }
    }
    let starts_with = |word: &str| {
        word.chars()
            .enumerate()
            .all(|(k, c)| chars.get(i + k) == Some(&c))
    };
    let is_url = starts_with("http://") || starts_with("https://");
    let is_www = !is_url && starts_with("www.");
    if is_url || is_www {
        let mut j = i;
        while j < chars.len()
            && !chars[j].is_whitespace()
            && !matches!(chars[j], '<' | '>' | '"' | '\'')
        {
            j += 1;
        }
        // Trailing punctuation marked would not include in the URL.
        while j > i && matches!(chars[j - 1], '.' | ',' | ';' | ':' | '!' | '?') {
            j -= 1;
        }
        while j > i
            && chars[j - 1] == ')'
            && chars[i..j].iter().filter(|&&c| c == '(').count()
                < chars[i..j].iter().filter(|&&c| c == ')').count()
        {
            j -= 1;
        }
        if j == i {
            return None;
        }
        let text: String = chars[i..j].iter().collect();
        let href = if is_www {
            format!("http://{text}")
        } else {
            text.clone()
        };
        return Some((text, href, j - i));
    }
    // Bare email: local part, `@`, a dotted domain.
    let mut j = i;
    while j < chars.len()
        && (chars[j].is_ascii_alphanumeric()
            || matches!(chars[j], '.' | '_' | '%' | '+' | '-' | '@'))
    {
        j += 1;
    }
    if j > i {
        let text: String = chars[i..j].iter().collect();
        if let Some(at) = text.find('@')
            && at > 0
            && at + 1 < text.len()
            && text[at + 1..].contains('.')
            && !text.contains("..")
            && !text.ends_with('.')
            && !text.ends_with('-')
        {
            return Some((text.clone(), format!("mailto:{text}"), j - i));
        }
    }
    None
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
