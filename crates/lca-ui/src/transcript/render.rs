//! The transcript's renderer: entries to styled lines (`messages.md`).
//!
//! Split from the module's state so both halves stay under the workspace's
//! 1,200-line file ceiling; the state (`Transcript`, `Entry`) keeps the
//! module doc. Only the two functions the state and the tests reach -
//! `render_entry` and `format_tool_args` - cross the seam.

use lca_tui::engine::text::{truncate_to_width, visible_width, wrap_text_with_ansi};
use lca_tui::widgets::image::render_image;
use lca_tui::widgets::markdown::{CodeBlockBorder, LinkMode, MarkdownOptions, render_markdown};

use crate::theme::{Role, StyleFn, Theme};

use super::{Entry, ThinkingVisibility, ToolStatus};

/// Markdown options for the terminal: the link mode follows the terminal's
/// OSC 8 capability, so a URL never vanishes on a terminal that swallows
/// the hyperlink (pi's `markdown.md` §6).
fn markdown_options(streaming: bool, codeblock_border: CodeBlockBorder) -> MarkdownOptions {
    MarkdownOptions {
        // pi renders assistant markdown with `outputPad = 1`
        // (`assistant-message.ts`), so every line carries a one-space left
        // margin and the text block is inset from the transcript edge.
        padding_x: 1,
        link_mode: if lca_tui::engine::terminal::supports_hyperlinks() {
            LinkMode::Hyperlink
        } else {
            LinkMode::Inline
        },
        // While the answer streams, pi suppresses mermaid's warning note
        // and shows it once the message settles.
        streaming,
        codeblock_border,
        ..Default::default()
    }
}

pub(super) fn render_entry(
    entry: &Entry,
    width: u16,
    theme: &Theme,
    tools_expanded: bool,
    thinking: ThinkingVisibility,
    codeblock_border: CodeBlockBorder,
    out: &mut Vec<String>,
) {
    match entry {
        Entry::User(text) => render_user(text, width, theme, codeblock_border, out),
        Entry::Assistant {
            text,
            reasoning,
            streaming,
            thinking_override,
        } => render_assistant(
            text,
            reasoning,
            *streaming,
            // R6: the run's own toggle wins over the configured default.
            match thinking_override {
                Some(true) => ThinkingVisibility::Full,
                Some(false) => ThinkingVisibility::Hidden,
                None => thinking,
            },
            width,
            theme,
            codeblock_border,
            out,
        ),
        Entry::Tool { .. } => render_tool(entry, tools_expanded, width, theme, out),
        Entry::Notice(text) => render_custom(text, width, theme, out),
        Entry::Error(text) => {
            out.extend(wrap_text_with_ansi(&(theme.error)(text), width as usize));
        }
        Entry::Raw(text) => {
            // A bracketed header (`[compaction] …`, `[session in …]`) is
            // pi's custom-message shape: a `customMessageBg` band with the
            // type in `customMessageLabel`. Anything else is verbatim.
            if text.starts_with('[') && text.contains(']') {
                render_custom(text, width, theme, out);
            } else {
                out.extend(wrap_text_with_ansi(text, width as usize));
            }
        }
        Entry::Image { info, bytes } => {
            out.extend(render_image(
                info,
                bytes,
                lca_tui::widgets::image::detect_image_protocol(),
                width as usize,
            ));
        }
    }
}

/// One row of a background band: the row padded to the full width, then
/// wrapped in the role's background (pi's `Box::applyBg`, which pads and
/// then paints - `messages.md` §2). The background closes its own channel
/// only, so a styled run inside the row keeps its own color.
fn band_row(row: &str, width: usize, bg: &StyleFn) -> String {
    let pad = width.saturating_sub(visible_width(row));
    bg(&format!("{row}{}", " ".repeat(pad)))
}

/// pi's custom message (`custom-message.ts`): a `customMessageBg` box with
/// the `[type]` label in `customMessageLabel` and the body in
/// `customMessageText`. LCA's `[compaction]` and `[session in …]` headers
/// are the same shape, so they take the same treatment.
fn render_custom(text: &str, width: u16, theme: &Theme, out: &mut Vec<String>) {
    let width = width as usize;
    let (label, body) = match (text.starts_with('['), text.find(']')) {
        (true, Some(close)) => (&text[..=close], text[close + 1..].trim_start()),
        _ => ("", text),
    };
    let content = width.saturating_sub(3).max(1);
    let styled = if label.is_empty() {
        (theme.role(Role::CustomMessageText))(body)
    } else {
        format!(
            "{} {}",
            (theme.role(Role::CustomMessageLabel))(label),
            (theme.role(Role::CustomMessageText))(body)
        )
    };
    let bg = theme.bg(Role::CustomMessageBg);
    out.push(band_row("", width, &bg));
    for line in wrap_text_with_ansi(&styled, content) {
        out.push(band_row(&format!(" {line}"), width, &bg));
    }
    out.push(band_row("", width, &bg));
}

fn render_user(
    text: &str,
    width: u16,
    theme: &Theme,
    codeblock_border: CodeBlockBorder,
    out: &mut Vec<String>,
) {
    // pi's user bubble: `Box(outputPad = 1, 1, theme.bg("userMessageBg"))`
    // around the user's own *markdown* in `userMessageText` - a full-width
    // band, content padded one column inside it, one blank band row above
    // and below. pi passes `preserveOrderedListMarkers` and
    // `preserveBackslashEscapes` for the user's own text (`user-message.ts`),
    // so `1)` stays `1)` and `\*` stays `\*`. The `› ` marker stays:
    // color is never the only signal (NFR-28).
    let width = width as usize;
    let bg = theme.bg(Role::UserMessageBg);
    let content = width.saturating_sub(3).max(1);
    let options = MarkdownOptions {
        padding_x: 0,
        padding_y: 0,
        link_mode: if lca_tui::engine::terminal::supports_hyperlinks() {
            LinkMode::Hyperlink
        } else {
            LinkMode::Inline
        },
        preserve_ordered_list_markers: true,
        preserve_backslash_escapes: true,
        render_latex: true,
        streaming: false,
        codeblock_border,
    };
    let wrapped = render_markdown(text, content, &theme.markdown(), &options);
    out.push(band_row("", width, &bg));
    for (i, line) in wrapped.iter().enumerate() {
        let prefix = if i == 0 { "› " } else { "  " };
        out.push(band_row(
            &format!(" {}{}", prefix, (theme.user)(line)),
            width,
            &bg,
        ));
    }
    out.push(band_row("", width, &bg));
}

// Seven rendering inputs plus the sink already; the frame shape (gh #32)
// is the eighth. Bundling them into a context struct is the upgrade if a
// ninth arrives.
#[allow(clippy::too_many_arguments)]
fn render_assistant(
    text: &str,
    reasoning: &str,
    streaming: bool,
    thinking: ThinkingVisibility,
    width: u16,
    theme: &Theme,
    codeblock_border: CodeBlockBorder,
    out: &mut Vec<String>,
) {
    if !reasoning.is_empty() {
        match thinking {
            ThinkingVisibility::Full => {
                for line in
                    wrap_text_with_ansi(reasoning, (width as usize).saturating_sub(2).max(1))
                {
                    // A blank line inside the run stays blank.
                    if line.trim().is_empty() {
                        out.push(String::new());
                        continue;
                    }
                    out.push(format!("  {}", (theme.reasoning)(&line)));
                }
            }
            // R6's default: enough thinking to see where the model is
            // going, then a count of what is left.
            // W3: When streaming, follow the tail (the latest lines are what the user watches).
            ThinkingVisibility::Snippet => {
                const SNIPPET_LINES: usize = 3;
                let lines: Vec<&str> = reasoning
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .collect();
                if streaming && lines.len() > SNIPPET_LINES {
                    // Tailing view during streaming: show header count of previous lines, then the last 3 lines
                    out.push(format!(
                        "  {}",
                        (theme.reasoning)(&format!(
                            "Thinking… ({} earlier lines)",
                            lines.len() - SNIPPET_LINES
                        ))
                    ));
                    for line in lines.iter().skip(lines.len() - SNIPPET_LINES) {
                        for wrapped in
                            wrap_text_with_ansi(line, (width as usize).saturating_sub(2).max(1))
                        {
                            out.push(format!("  {}", (theme.reasoning)(&wrapped)));
                        }
                    }
                } else {
                    for line in lines.iter().take(SNIPPET_LINES) {
                        for wrapped in
                            wrap_text_with_ansi(line, (width as usize).saturating_sub(2).max(1))
                        {
                            out.push(format!("  {}", (theme.reasoning)(&wrapped)));
                        }
                    }
                    if lines.len() > SNIPPET_LINES {
                        out.push(format!(
                            "  {}",
                            (theme.reasoning)(&format!(
                                "… +{} lines ({} to expand)",
                                lines.len() - SNIPPET_LINES,
                                lca_tui::engine::keybindings::key_text("app.thinking.toggle")
                            ))
                        ));
                    }
                }
            }
            ThinkingVisibility::Hidden => {
                // pi's original: one dim line (`messages.md` §3).
                out.push(format!(
                    "  {}",
                    (theme.reasoning)(&format!(
                        "Thinking… ({} to expand)",
                        lca_tui::engine::keybindings::key_text("app.thinking.toggle")
                    ))
                ));
            }
        }
    }
    if !text.is_empty() {
        let md = render_markdown(
            text,
            width as usize,
            &theme.markdown(),
            &markdown_options(streaming, codeblock_border),
        );
        out.extend(md);
    }
    if streaming {
        out.push((theme.dim)("▍"));
    }
}

/// A one-line argument summary for a tool card (pi's per-tool formats,
/// `messages.md` §6). Unknown tools fall back to the raw arguments.
pub(super) fn format_tool_args(name: &str, args: &str) -> String {
    if args.trim().is_empty() {
        return String::new();
    }
    let summary = match name {
        "read" | "write" | "edit" | "list" | "ls" | "glob" => {
            json_string_field(args, "path").or_else(|| json_string_field(args, "file_path"))
        }
        "bash" | "shell" => json_string_field(args, "command"),
        "grep" => json_string_field(args, "pattern").map(|pattern| format!("/{pattern}/")),
        _ => None,
    };
    summary.unwrap_or_else(|| args.to_string())
}

/// Pull a string field out of a flat JSON object without a parser. Tool
/// arguments are always a flat object the model emitted; a value that is
/// not a plain string yields `None` and the caller keeps the raw args.
fn json_string_field(args: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = args.find(&needle)? + needle.len();
    let rest = args[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let mut chars = rest.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'u' => {
                    for _ in 0..4 {
                        chars.next()?;
                    }
                    out.push('\u{fffd}');
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

fn render_tool(entry: &Entry, expanded: bool, width: u16, theme: &Theme, out: &mut Vec<String>) {
    let Entry::Tool {
        name,
        args,
        status,
        result,
        diff,
        manual,
    } = entry
    else {
        return;
    };
    let width = width as usize;
    let inner = width.saturating_sub(3).max(1);
    let summary = format_tool_args(name, args);
    let status_text = match status {
        ToolStatus::Ok => (theme.success)(status.label()),
        ToolStatus::Error | ToolStatus::Denied => (theme.error)(status.label()),
        ToolStatus::Timeout => (theme.warn)(status.label()),
        ToolStatus::Running => (theme.dim)(status.label()),
    };
    // pi's tool-card roles, read off its renderers (`renderers/*.ts` and
    // `tool-execution.ts`): the name is `toolTitle` **plus bold** -
    // `bashMode` for the editor's own `!` runs, which pi draws as its own
    // `BashExecutionComponent`; a path or pattern argument is `accent`;
    // a shell command rides inside the bold title (`formatShellCall`);
    // an unknown tool's JSON arguments stay plain (the card fallback).
    let name_style = theme.role_bold(if *manual {
        Role::BashMode
    } else {
        Role::ToolTitle
    });
    let shell = matches!(name.as_str(), "bash" | "shell");
    let path_tool = matches!(
        name.as_str(),
        "read" | "write" | "edit" | "list" | "ls" | "glob" | "grep"
    );
    let (title, args_span) = if summary.is_empty() {
        (name_style(name), String::new())
    } else if shell {
        (name_style(&format!("{name} {summary}")), String::new())
    } else if path_tool {
        (
            name_style(name),
            format!(" {}", (theme.role(Role::Accent))(&summary)),
        )
    } else {
        (name_style(name), format!(" {summary}"))
    };
    let header = format!("{} {title}{args_span} {status_text}", (theme.tool)(">"));
    let mut rows = vec![truncate_to_width(&header, inner, "…", false)];
    // gh #9 / EFG-016: a structured diff renders as pi's diff card -
    // the diff's own line kinds in the two diff roles, bounded like
    // every other preview, inside the same state band. The diff came
    // from the tool's own before/after (EFG-014); nothing here parses
    // display text back into a change.
    if let Some(diff) = diff {
        if let Some(summary) = result {
            rows.push(format!("  {}", (theme.role(Role::ToolOutput))(summary)));
        }
        let lines: Vec<&str> = diff.lines().collect();
        let preview = if expanded {
            lines.len()
        } else {
            DIFF_PREVIEW_LINES
        };
        for line in lines.iter().take(preview) {
            let clipped = truncate_to_width(line, inner.saturating_sub(2), "…", false);
            rows.push(format!("  {}", (theme.role(diff_role(line)))(&clipped)));
        }
        if lines.len() > preview {
            rows.push(format!(
                "  {}",
                (theme.role(Role::Muted))(&format!(
                    "… ({} more lines, {} to expand)",
                    lines.len() - preview,
                    lca_tui::engine::keybindings::key_text("app.tools.expand")
                ))
            ));
        }
    }
    // pi shows a bounded preview of command output (`bash.ts`
    // `BASH_PREVIEW_LINES` = 5, `ls.ts` 20, `grep.ts` 15) and a one-line
    // card for the rest; Ctrl+O expands to the full result.
    if let Some(result) = result {
        let preview = preview_lines(name);
        let total = result.lines().count();
        if expanded {
            for line in wrap_text_with_ansi(result, inner.saturating_sub(2)) {
                rows.push(format!("  {}", (theme.role(Role::ToolOutput))(&line)));
            }
        } else if preview > 0 && total > 0 {
            let shown: String = result.lines().take(preview).collect::<Vec<_>>().join("\n");
            for line in wrap_text_with_ansi(&shown, inner.saturating_sub(2)) {
                rows.push(format!("  {}", (theme.role(Role::ToolOutput))(&line)));
            }
            if total > preview {
                rows.push(format!(
                    "  {}",
                    (theme.role(Role::Muted))(&format!(
                        "… ({} more lines, {} to expand)",
                        total - preview,
                        lca_tui::engine::keybindings::key_text("app.tools.expand")
                    ))
                ));
            }
        } else if total > 1 {
            rows.push(format!(
                "  {}",
                (theme.role(Role::Muted))(&format!(
                    "… ({} to expand)",
                    lca_tui::engine::keybindings::key_text("app.tools.expand")
                ))
            ));
        }
    }

    // The card's background is the call's state (pi's `updateDisplay`):
    // `toolPendingBg` while the call is in flight or its output is still
    // streaming, the quiet success tint when it settled, the error tint
    // when it did not (`messages.md` §6).
    let bg = theme.bg(match status {
        ToolStatus::Running => Role::ToolPendingBg,
        ToolStatus::Ok => Role::ToolSuccessBg,
        ToolStatus::Error | ToolStatus::Denied | ToolStatus::Timeout => Role::ToolErrorBg,
    });
    out.push(band_row("", width, &bg));
    for row in &rows {
        out.push(band_row(&format!(" {row}"), width, &bg));
    }
    out.push(band_row("", width, &bg));
}

/// The collapsed preview length for a structured diff (gh #9): enough
/// rows to show the change, bounded like every other tool preview.
const DIFF_PREVIEW_LINES: usize = 10;

/// One diff line's role, decided by its unified-diff marker - pi's
/// `renderDiff` parses the same shape: `+` is added, `-` is removed, and
/// everything else (context lines, the `@@`/`---`/`+++` headers) is
/// context.
fn diff_role(line: &str) -> Role {
    if line.starts_with('+') && !line.starts_with("+++") {
        Role::ToolDiffAdded
    } else if line.starts_with('-') && !line.starts_with("---") {
        Role::ToolDiffRemoved
    } else {
        Role::ToolDiffContext
    }
}

/// The collapsed preview line count for a tool, from pi's per-tool
/// renderers: `bash` 5, `ls` 20, `grep` 15; the rest show one line.
fn preview_lines(name: &str) -> usize {
    match name {
        "bash" | "shell" => 5,
        "list" | "ls" => 20,
        "grep" => 15,
        _ => 0,
    }
}
