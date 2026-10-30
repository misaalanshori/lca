//! The transcript's renderer: entries to styled lines (`messages.md`).
//!
//! Split from the module's state so both halves stay under the workspace's
//! 1,200-line file ceiling; the state (`Transcript`, `Entry`) keeps the
//! module doc. Only the two functions the state and the tests reach -
//! `render_entry` and `format_tool_args` - cross the seam.

use lca_tui::engine::text::{truncate_to_width, visible_width, wrap_text_with_ansi};
use lca_tui::widgets::image::render_image;
use lca_tui::widgets::markdown::{
    CodeBlockBorder, LinkMode, MarkdownMessageType, MarkdownOptions, MarkdownTransformer,
    render_markdown,
};

use crate::theme::{Role, StyleFn, Theme};

use super::{Entry, ThinkingVisibility, ToolStatus};

/// Markdown options for the terminal: the link mode follows the terminal's
/// OSC 8 capability, so a URL never vanishes on a terminal that swallows
/// the hyperlink (pi's `markdown.md` §6).
fn markdown_options(
    streaming: bool,
    codeblock_border: CodeBlockBorder,
    message_type: MarkdownMessageType,
    transformers: Vec<MarkdownTransformer>,
    pad: usize,
    mermaid: lca_tui::widgets::markdown::MermaidMode,
    indent: &str,
) -> MarkdownOptions {
    MarkdownOptions {
        // Gh #82: pi's `outputPad` (assistant margin); the user bubble
        // keeps its own zero below.
        padding_x: pad,
        link_mode: if lca_tui::engine::terminal::supports_hyperlinks() {
            LinkMode::Hyperlink
        } else {
            LinkMode::Inline
        },
        // While the answer streams, pi suppresses mermaid's warning note
        // and shows it once the message settles.
        streaming,
        codeblock_border,
        message_type,
        transformers,
        mermaid,
        code_indent: indent.to_string(),
        ..Default::default()
    }
}

/// The startup key-hint line (gh #23): pi prints
/// `escape interrupt · ctrl+c/ctrl+d clear/exit · / commands` at startup; this is
/// our wording on pi's shape, resolved from the **live** keybindings so a rebind
/// reaches it (gh #410).
pub(crate) fn startup_hint(
    keybindings: &lca_tui::engine::keybindings::KeybindingsManager,
) -> String {
    format!(
        "{} interrupt · {} clear/exit · / commands",
        keybindings.display_keys("app.interrupt"),
        keybindings.display_keys("app.clear"),
    )
}

/// A key hint in the theme's own roles: the key **dim**, the description
/// **muted** (gh #410, pi's `keyHint`).
///
/// There is deliberately **one** form and it always takes the live manager. An
/// overload that skipped the manager and used the default table would be exactly
/// the shape this cycle removes: a helper whose name promises a live label and
/// hands back a default. A caller with no interface around it passes
/// `&KeybindingsManager::new()` and gets the shipped defaults, explicitly.
///
/// Returns **empty** when the action is unbound, so a caller can omit the hint
/// rather than render a blank key press. `description` is optional because some
/// hints are just the key.
pub(crate) fn render_key_hint(
    theme: &Theme,
    keybindings: &lca_tui::engine::keybindings::KeybindingsManager,
    action: &str,
    description: Option<&str>,
) -> String {
    let keys = keybindings.display_keys(action);
    if keys.is_empty() {
        return String::new();
    }
    let mut hint = (theme.role(Role::Dim))(&keys);
    if let Some(description) = description {
        hint.push_str(&(theme.role(Role::Muted))(&format!(" {description}")));
    }
    hint
}

/// [`render_key_hint`] in the parenthesised form these sentences use:
/// `" (ctrl+o to expand)"`, or **nothing at all** when the action is unbound.
///
/// The empty form is the point (gh #410 design point 5): a user who unbinds
/// `app.tools.expand` gets `… +12 lines`, not `… +12 lines ()` and not a blank
/// key press. The parentheses live here, so a caller cannot accidentally leave
/// half a clause behind.
fn paren_hint(
    theme: &Theme,
    keybindings: &lca_tui::engine::keybindings::KeybindingsManager,
    action: &str,
    description: &str,
) -> String {
    let hint = render_key_hint(theme, keybindings, action, Some(description));
    if hint.is_empty() {
        String::new()
    } else {
        format!(" ({hint})")
    }
}

/// Gh #393: the compaction card lives in its own module (`compaction_card.rs`);
/// this is the one call site.
use crate::transcript::compaction_card::render_card as render_compaction_card;

// Eight rendering inputs plus the sink; like `render_assistant` below,
// a context struct is the upgrade if a ninth arrives.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_entry(
    entry: &Entry,
    width: u16,
    theme: &Theme,
    tools_expanded: bool,
    thinking: ThinkingVisibility,
    codeblock_border: CodeBlockBorder,
    transformers: &[MarkdownTransformer],
    tuning: &crate::state::DisplayTuning,
    keybindings: &lca_tui::engine::keybindings::KeybindingsManager,
    out: &mut Vec<String>,
) -> usize {
    match entry {
        Entry::User(text) => {
            render_user(
                text,
                width,
                theme,
                codeblock_border,
                transformers,
                tuning,
                out,
            );
            0
        }
        Entry::Assistant {
            text,
            reasoning,
            streaming,
            thinking_override,
        } => render_assistant(
            text,
            reasoning,
            *streaming,
            // R6: the run's own cycle wins over the configured default.
            thinking_override.unwrap_or(thinking),
            width,
            theme,
            codeblock_border,
            transformers,
            tuning,
            keybindings,
            out,
        ),
        Entry::Tool { .. } => {
            render_tool(entry, tools_expanded, width, theme, keybindings, out);
            0
        }
        Entry::Notice(text) => {
            render_custom(text, width, theme, out);
            0
        }
        // Gh #393: the collapsed card. `expanded` is the per-entry override over
        // the global default, the same rule a tool card follows.
        Entry::Compaction { card, expanded } => {
            render_compaction_card(
                card,
                expanded.unwrap_or(tools_expanded),
                width,
                theme,
                keybindings,
                out,
            );
            0
        }
        Entry::Error(text) => {
            out.extend(wrap_text_with_ansi(&(theme.error)(text), width as usize));
            0
        }
        Entry::StartupHint => {
            // pi's startup shape, our wording; the keys come from the live
            // manager so a rebind reaches the first line a user reads (gh #410).
            // Dim, as the line was before it became a live entry - the first
            // version of this dropped the styling, which a real pane caught.
            out.push((theme.role(Role::Dim))(&startup_hint(keybindings)));
            0
        }
        Entry::StartupResources {
            compact,
            expanded,
            expanded_state,
        } => {
            // Gh #400 gap 2: the compact line, or the table behind `ctrl+o`. The
            // collision notices are already inside `compact`/`expanded` (they ride
            // both states), so this arm is a pure choice between two prepared sets
            // - the header does not re-decide anything at paint time.
            let rows = if *expanded_state { expanded } else { compact };
            for row in rows {
                out.extend(wrap_text_with_ansi(row, width as usize));
            }
            0
        }
        Entry::StartupHeader { beside_resources } => {
            // Gh #399: the owner's logo block, resolving keys and width here so a
            // rebind and a resize both reach it. The rows come back unwrapped; the
            // text rows wrap through `wrap_text_with_ansi` the way every other
            // block does, while the mark's rows are single glyph cells that always
            // fit their column by construction.
            let version = crate::startup_header::VersionInfo::new(
                tuning.startup_version.0.clone(),
                tuning.startup_version.1.clone(),
            );
            // The rung is decided **here**, so a `/settings` cycle or a quiet
            // startup moves it without a restart. `Hidden` draws no rows at all
            // (`quietStartup = true` is the setting's whole point), `VersionOnly`
            // keeps the pre-#399 middle rung, and `Full` is the owner's block.
            let rows = match tuning.startup_header {
                crate::state::StartupHeaderRung::Hidden => Vec::new(),
                crate::state::StartupHeaderRung::VersionOnly => {
                    vec![(theme.dim)(&version.label())]
                }
                crate::state::StartupHeaderRung::Full => crate::startup_header::compose(
                    theme,
                    keybindings,
                    &version,
                    width as usize,
                    lca_tui::engine::terminal::is_apple_terminal(),
                ),
            };
            // When the resource line beneath already names the build, drop this
            // block's own version row and its `press …` hint: the resource line *is*
            // the version-bearing, expandable row, so printing either twice reads as
            // a duplicated startup. The mark, the key hints and the nudge stay.
            let rows: Vec<String> = if *beside_resources {
                rows.into_iter()
                    .map(|row| drop_version_text(&row))
                    .collect()
            } else {
                rows
            };
            for row in rows {
                out.extend(wrap_text_with_ansi(&row, width as usize));
            }
            0
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
            0
        }
        Entry::Image { info, bytes } => {
            // Gh #82: `terminal.show_images` gates display; the width
            // cap (`terminal.image_width_cells`) rides the caller width.
            if tuning.show_images {
                out.extend(render_image(
                    info,
                    bytes,
                    lca_tui::widgets::image::detect_image_protocol(),
                    (width as usize).min(tuning.image_width_cells.max(1) as usize),
                ));
            }
            0
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

/// Remove this block's version text from one row, keeping the row.
///
/// Gh #399 + gh #400: when the expandable resource line beneath already names the
/// build, printing `v0.6.0 (abi 0.6)` above it reads as a duplicated startup. The
/// row must **stay** - it carries the mark's first line, and the nudge shares row 3
/// - so this strips the label and leaves the styling and the glyphs alone.
fn drop_version_text(row: &str) -> String {
    // The label is `v<version> (abi <abi>)`; it is the only ` (abi ` on the row.
    let Some(open) = row.find(" (abi ") else {
        return row.to_string();
    };
    // Walk back to the `v` that opens the label, which sits after the mark's
    // column and a space.
    let Some(version_at) = row[..open].rfind('v') else {
        return row.to_string();
    };
    let Some(close) = row[open..].find(')') else {
        return row.to_string();
    };
    let end = open + close + 1;
    format!("{}{}", &row[..version_at], &row[end..])
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
    transformers: &[MarkdownTransformer],
    tuning: &crate::state::DisplayTuning,
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
        message_type: MarkdownMessageType::User,
        transformers: transformers.to_vec(),
        mermaid: lca_tui::widgets::markdown::MermaidMode::parse(&tuning.mermaid),
        code_indent: tuning.code_block_indent.clone(),
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
// is the eighth, the transform pipeline (gh #12) the ninth. Bundling them
// into a context struct is the upgrade if a tenth arrives.
#[allow(clippy::too_many_arguments)]
fn render_assistant(
    text: &str,
    reasoning: &str,
    streaming: bool,
    thinking: ThinkingVisibility,
    width: u16,
    theme: &Theme,
    codeblock_border: CodeBlockBorder,
    transformers: &[MarkdownTransformer],
    tuning: &crate::state::DisplayTuning,
    keybindings: &lca_tui::engine::keybindings::KeybindingsManager,
    out: &mut Vec<String>,
) -> usize {
    let start = out.len();
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
                            "  {}{}",
                            (theme.reasoning)(&format!("… +{} lines", lines.len() - SNIPPET_LINES)),
                            paren_hint(theme, keybindings, "app.thinking.toggle", "to expand")
                        ));
                    }
                }
            }
            ThinkingVisibility::Hidden => {
                // pi's original: one dim line (`messages.md` §3).
                out.push(format!(
                    "  {}{}",
                    (theme.reasoning)("Thinking…"),
                    paren_hint(theme, keybindings, "app.thinking.toggle", "to expand")
                ));
            }
        }
    }
    // gh #11's hit map: how many leading rows belong to the reasoning
    // run (measured before the answer renders, so answer rows never
    // count as run rows).
    let thinking_rows = out.len() - start;
    if !text.is_empty() {
        let md = render_markdown(
            text,
            width as usize,
            &theme.markdown(),
            &markdown_options(
                streaming,
                codeblock_border,
                MarkdownMessageType::Assistant,
                transformers.to_vec(),
                tuning.output_pad as usize,
                lca_tui::widgets::markdown::MermaidMode::parse(&tuning.mermaid),
                &tuning.code_block_indent,
            ),
        );
        out.extend(md);
    }
    if streaming {
        out.push((theme.dim)("▍"));
    }
    thinking_rows
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

fn render_tool(
    entry: &Entry,
    expanded: bool,
    width: u16,
    theme: &Theme,
    keybindings: &lca_tui::engine::keybindings::KeybindingsManager,
    out: &mut Vec<String>,
) {
    let Entry::Tool {
        name,
        args,
        status,
        result,
        diff,
        manual,
        expanded: card_expanded,
        duration_ms,
        started_at,
        excluded,
        children,
        parent_link: _,
        call_id: _,
    } = entry
    else {
        return;
    };
    let expanded = card_expanded.unwrap_or(expanded);
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
    // Gh #391 gap 3: `!` (context-included) keeps pi's `bashMode`; `!!`
    // (eyes-only) draws quiet. A **mode**, not a decoration - so the plain
    // theme needs a textual cue too, which is why the title carries the
    // doubled marker below (NFR-28: colour is never the only signal).
    let name_style = theme.role_bold(match (manual, excluded) {
        (true, true) => Role::Dim,
        (true, false) => Role::BashMode,
        (false, _) => Role::ToolTitle,
    });
    let shell = matches!(name.as_str(), "bash" | "shell");
    let path_tool = matches!(
        name.as_str(),
        "read" | "write" | "edit" | "list" | "ls" | "glob" | "grep"
    );
    // Gh #391 gap 3: the editor's own runs wear the marker the user typed, so
    // `!` and `!!` stay distinguishable with no colour at all (NFR-28). A
    // model-requested shell keeps its plain name.
    let head = if *manual {
        if *excluded { "!!" } else { "!" }
    } else {
        ""
    };
    let name = &format!("{head}{name}");
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
    // Gh #214 + gh #391 gap 2: the timing line.
    //
    // A **settled** card shows its recorded duration and never moves again -
    // including a replayed historical card, whose `started_at` is `None` and
    // which must not resume a count that already finished. A **running**
    // card shows `Elapsed …`, re-derived from `started_at` every frame; the
    // interface's existing tick invalidates the cache once a second while a
    // tool runs, so no second clock is introduced (the #369 lesson: two
    // clocks is how they disagree).
    if let Some(ms) = duration_ms {
        rows.push(format!(
            "  {}",
            (theme.role(Role::Muted))(&format!("Took {}", format_duration_ms(*ms)))
        ));
    } else if *status == ToolStatus::Running
        && let Some(started) = started_at
    {
        let elapsed = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        rows.push(format!(
            "  {}",
            (theme.role(Role::Muted))(&format!("Elapsed {}", format_duration_ms(elapsed)))
        ));
    }
    // gh #9 / EFG-016: a structured diff renders as pi's diff card - the diff's own
    // line kinds in the two diff roles, bounded like every other preview, inside the
    // same state band. The diff came from the tool's own before/after (EFG-014); nothing
    // here parses display text back into a change.
    //
    // Gh #328: **diff and general output are one choice, not two blocks.** This used to
    // be `if let Some(diff) { … }` followed by `if let Some(result) { … }` with no
    // `else`, so a result carrying a diff drew the diff and then the general block over
    // the same `result` - an `edit` printed its summary twice and a tool whose result
    // *was* the diff printed every line twice. The predicate is [`DiffBody`]: either the
    // result is rendered as a diff, or as general output, and the value that chose is
    // what the second half obeys.
    let body = Body::classify(diff.as_deref(), result.as_deref(), name);
    match &body {
        Body::Diff {
            summary,
            diff,
            note,
        } => {
            // The summary is the card's one-line claim and the diff is its evidence, so
            // it leads. `note` is anything in the result that the diff does not already
            // contain - and it goes *after* the diff, where the trailing prose a tool
            // appends (a warning, a count of skipped files) is read in the order the tool
            // wrote it.
            if let Some(summary) = summary {
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
                let key = render_key_hint(theme, keybindings, "app.tools.expand", None);
                let shape = if key.is_empty() {
                    format!("… ({} more lines)", lines.len() - preview)
                } else {
                    format!("… ({} more lines, {key} to expand)", lines.len() - preview)
                };
                rows.push(format!("  {}", (theme.role(Role::Muted))(&shape)));
            }
            if let Some(note) = note
                && expanded
            {
                for line in wrap_text_with_ansi(note, inner.saturating_sub(2)) {
                    rows.push(format!("  {}", (theme.role(Role::ToolOutput))(&line)));
                }
            }
        }
        Body::Output => {
            // The general command-output block, unchanged: everything the card ever did
            // with a result that is not a diff. Extracted only so the two paths are
            // visibly exclusive - see [`render_tool_output`].
            render_tool_output(
                &mut rows,
                result.as_deref(),
                name,
                expanded,
                inner,
                theme,
                keybindings,
            );
        }
    }
    // gh #404 gap 1: this card's nested calls, as a checklist. Drawn after the
    // parent's own result, so the card reads "what it did, then what it did it
    // with" - and expanding the card expands the checklist with it.
    super::nested::render_checklist(children, expanded, inner, theme, keybindings, &mut rows);

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

/// The general command-output block of a tool card, unchanged (gh #328).
///
/// Extracted from `render_tool` only so the diff and output paths are **visibly
/// exclusive**: the card calls [`Body::classify`] once and then runs exactly one of these.
/// Before, this block was a second `if let Some(result)` that ran after the diff arm as
/// well, which is what printed an expanded diff twice.
///
/// Every behaviour below is the pre-#328 behaviour, kept verbatim: the preview length
/// per tool (`preview::preview_lines`), `\r` collapsing on expand, the head/tail split
/// for shells, the omission marker's side, and the single-row "`…` to expand" hint.
#[allow(clippy::too_many_arguments)]
fn render_tool_output(
    rows: &mut Vec<String>,
    result: Option<&str>,
    name: &str,
    expanded: bool,
    inner: usize,
    theme: &Theme,
    keybindings: &lca_tui::engine::keybindings::KeybindingsManager,
) {
    // pi shows a bounded preview of command output (`bash.ts`
    // `BASH_PREVIEW_LINES` = 5, `ls.ts` 20, `grep.ts` 15) and a one-line
    // card for the rest; Ctrl+O expands to the full result.
    let Some(result) = result else {
        return;
    };
    // gh #391 gap 4: the LLM-facing spillover footer is bookkeeping, not
    // output. It is stripped before anything else so it can never spend
    // one of the five tail rows, and the card's own truncation warning
    // below is the single place that fact is shown.
    let body = super::preview::strip_truncation_footer(result);
    // gh #404 gap 1 / design point 7: a codemode parent leads with its own
    // content, not with the wrapper's banner. Narrow by construction - see
    // `nested::is_wrapper_boilerplate` for why only the wrapper's exact
    // shapes are cut and author output that mentions them survives.
    let stripped;
    let body = if name.contains("codemode") {
        stripped = super::nested::strip_wrapper_boilerplate(body);
        stripped.as_str()
    } else {
        body
    };
    let preview = super::preview::preview_lines(name);
    let total = body.lines().count();
    if expanded {
        // Expanded shows everything, but still collapses `\r` progress
        // repaints: the terminal kept only the last segment, and an
        // expanded card printing every repaint is unreadable.
        let collapsed = super::preview::collapse_carriage_returns(body);
        for line in wrap_text_with_ansi(&collapsed, inner.saturating_sub(2)) {
            rows.push(format!("  {}", (theme.role(Role::ToolOutput))(&line)));
        }
    } else if preview > 0 && total > 0 {
        // gh #391 gap 1: wrap first, slice **visual** rows, and keep the
        // tail for shells - `preview.rs` carries the why.
        let built = super::preview::build_preview(name, body, inner.saturating_sub(2));
        let marker = |omitted: usize| {
            let key = render_key_hint(theme, keybindings, "app.tools.expand", None);
            let shape = if key.is_empty() {
                format!("… ({omitted} earlier lines)")
            } else {
                format!("… ({omitted} earlier lines, {key} to expand)")
            };
            format!("  {}", (theme.role(Role::Muted))(&shape))
        };
        // The omission marker names what precedes a tail, so it sits
        // above it; a head preview names what follows, so below.
        if built.omitted > 0 && built.marker_above {
            rows.push(marker(built.omitted));
        }
        for line in &built.rows {
            rows.push(format!("  {}", (theme.role(Role::ToolOutput))(line)));
        }
        if built.omitted > 0 && !built.marker_above {
            rows.push(marker(built.omitted));
        }
    } else if total > 1 {
        rows.push(format!(
            "  {}{}",
            (theme.role(Role::Muted))("…"),
            paren_hint(theme, keybindings, "app.tools.expand", "to expand")
        ));
    }
}

/// What a tool card's body is (gh #328): **one** choice between a diff and general
/// output, so the two can never both render.
///
/// The defect this type replaces was two independent `if let`s in a row - a diff arm
/// followed by an output arm - which meant a result carrying a diff was rendered by
/// both. A type rather than an `else` because the diff arm needs three pieces the output
/// arm must not see, and because "which one is it" is exactly the question a reader of
/// this code asks.
enum Body<'a> {
    /// The result is shown as a diff: the one-line summary, the change, and any note the
    /// result carried that the diff does not already contain.
    Diff {
        /// The card's claim, from the result's leading line(s).
        summary: Option<String>,
        /// The change itself.
        diff: &'a str,
        /// Result text the diff does not contain. `None` when the result is entirely
        /// accounted for - which is the common case, and the reason a `Some(diff)` card
        /// usually shows no extra block.
        note: Option<String>,
    },
    /// The result is shown as general command output, exactly as it always was.
    Output,
}

impl<'a> Body<'a> {
    /// The predicate (gh #328 design point 1): a tool card renders as a diff **or** as
    /// output, decided here and nowhere else.
    ///
    /// * No diff → output. Unchanged behaviour for every non-diff tool.
    /// * A diff → **diff**, with the result split into a summary and whatever else it
    ///   carried. The split is what keeps the fix non-lossy: dropping the general block
    ///   wholesale would have deleted a trailing note or a truncation marker with it.
    fn classify(diff: Option<&'a str>, result: Option<&str>, name: &str) -> Body<'a> {
        let Some(diff) = diff.filter(|text| !text.is_empty()) else {
            return Body::Output;
        };
        // A codemode parent's result leads with the wrapper's own banner (gh #404 gap 1);
        // stripping it here keeps the split honest, because the banner is not the card's
        // claim and would otherwise read as the summary.
        let result = result.map(|text| {
            let body = super::preview::strip_truncation_footer(text);
            if name.contains("codemode") {
                super::nested::strip_wrapper_boilerplate(body)
            } else {
                body.to_string()
            }
        });
        let Some(result) = result.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
            return Body::Diff {
                summary: None,
                diff,
                note: None,
            };
        };
        let (summary, note) = split_summary_and_note(result, diff);
        Body::Diff {
            summary,
            diff,
            note,
        }
    }
}

/// Split a result into the card's **summary** and any text the diff does not already
/// contain (gh #328 design point 3).
///
/// Three shapes, all observed:
///
/// * the result repeats the diff (a patch tool that returns the change as prose) → there
///   is nothing left over, so no note and no summary either: the diff *is* the content,
///   and printing its first line as a summary would be a fourth copy of it;
/// * the result is a one-line claim beside the diff (`edit`'s "Successfully replaced …")
///   → that line is the summary, and there is no note;
/// * the result is a claim **plus** other prose (a warning, a count of skipped files) →
///   the first line is the summary and the rest is the note, which the card prints after
///   the diff so it reads in the order the tool wrote it.
///
/// Returns `(summary, note)`.
fn split_summary_and_note(result: &str, diff: &str) -> (Option<String>, Option<String>) {
    let diff_lines: Vec<&str> = diff
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let result_lines: Vec<&str> = result.lines().collect();
    // No separate "the result *is* the diff" early exit: a line the diff already draws is
    // skipped below, so that case falls out as (no summary, no note) without a second rule
    // restating it. The version with an explicit `all_in_diff` check was written first and
    // removed after mutation-testing showed it unreachable - the per-line skip already
    // answered it, and an unpinned branch is how a rule rots (the gh #304 lesson).
    let mut summary = None;
    let mut note_lines: Vec<&str> = Vec::new();
    for line in &result_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // A line the diff already draws is not summarised or repeated: it is the evidence.
        if diff_lines.contains(&trimmed) {
            continue;
        }
        if summary.is_none() {
            summary = Some(trimmed.to_string());
        } else {
            note_lines.push(line);
        }
    }
    if note_lines.is_empty() {
        (summary, None)
    } else {
        (summary, Some(note_lines.join("\n").trim().to_string()))
    }
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

/// pi's `formatToolDuration` tiers (`render-utils.ts:95-170`): `4.2s` under a
/// minute, `1m 30s` under an hour, `2h 3m 4s` above. Sub-second durations keep
/// one decimal so a fast tool does not read as `0s`.
pub(super) fn format_duration_ms(ms: u64) -> String {
    if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else if ms < 3_600_000 {
        let minutes = ms / 60_000;
        let seconds = (ms % 60_000) / 1000;
        format!("{minutes}m {seconds}s")
    } else {
        let hours = ms / 3_600_000;
        let minutes = (ms % 3_600_000) / 60_000;
        let seconds = (ms % 60_000) / 1000;
        format!("{hours}h {minutes}m {seconds}s")
    }
}
