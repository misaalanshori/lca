//! The transcript, ported from pi's
//! `coding-agent/src/modes/interactive/components/messages.ts`
//! (`pi-tui-re/src_re/agent-components/messages.md`).
//!
//! Owner issues #5 (user prompts in history), #6 (markdown, message
//! separation) and #4 (streaming) live here: the transcript is a list of
//! entries rendered to styled lines, updated incrementally as a turn
//! streams, and the renderer repaints only what changed.

use lca_tui::engine::text::{truncate_to_width, visible_width, wrap_text_with_ansi};
use lca_tui::widgets::image::{ImageInfo, render_image};
use lca_tui::widgets::markdown::{LinkMode, MarkdownOptions, render_markdown};

use crate::theme::{Role, StyleFn, Theme};

/// A tool call's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// In flight.
    Running,
    /// Finished successfully.
    Ok,
    /// Finished with an error.
    Error,
    /// Denied by permission or a hook.
    Denied,
    /// Timed out.
    Timeout,
}

impl ToolStatus {
    fn label(self) -> &'static str {
        match self {
            ToolStatus::Running => "…",
            ToolStatus::Ok => "ok",
            ToolStatus::Error => "error",
            ToolStatus::Denied => "denied",
            ToolStatus::Timeout => "timeout",
        }
    }
}

/// One transcript entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// A user prompt.
    User(String),
    /// An assistant message (answer + optional reasoning).
    Assistant {
        /// The answer text (may still be streaming).
        text: String,
        /// Reasoning text, if any.
        reasoning: String,
        /// Whether the message is still streaming.
        streaming: bool,
        /// This run's thinking visibility, when the user toggled it
        /// (R6); `None` follows [`Transcript`]'s configured default.
        thinking_override: Option<bool>,
    },
    /// A tool call and its result.
    Tool {
        /// Tool name.
        name: String,
        /// A short argument summary.
        args: String,
        /// Status.
        status: ToolStatus,
        /// Result preview, once finished.
        result: Option<String>,
    },
    /// A transient notice (command output, info).
    Notice(String),
    /// An error.
    Error(String),
    /// A pre-rendered line from a resumed session (displayed verbatim).
    Raw(String),
    /// An image the terminal renders through the graphics ladder, or as a
    /// placeholder when it has no graphics support (FR-UI-13, R5).
    Image {
        /// What is known about the image.
        info: ImageInfo,
        /// The raw bytes, for the graphics protocols.
        bytes: std::sync::Arc<Vec<u8>>,
    },
}

/// One entry's cached render: the width it was rendered at and its styled
/// lines. `None` means the entry must be re-rendered.
type CachedRender = Option<(u16, Vec<String>)>;

/// How a thinking run renders (R6, pi's `thinkingVisibility`): a per-run
/// override on top of this default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThinkingVisibility {
    /// The first few non-empty lines, then a `… +N lines` marker. The
    /// default: the owner asked to *see* the thinking, not to read all of
    /// it (R6).
    #[default]
    Snippet,
    /// The whole run, rendered as it streams.
    Full,
    /// One dim line, pi's original `hideThinkingBlock` behavior.
    Hidden,
}

impl ThinkingVisibility {
    /// The config spelling (`ui.thinking`).
    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingVisibility::Snippet => "snippet",
            ThinkingVisibility::Full => "full",
            ThinkingVisibility::Hidden => "hidden",
        }
    }

    /// Parse the config spelling.
    pub fn parse(text: &str) -> Option<ThinkingVisibility> {
        match text {
            "snippet" => Some(ThinkingVisibility::Snippet),
            "full" => Some(ThinkingVisibility::Full),
            "hidden" => Some(ThinkingVisibility::Hidden),
            _ => None,
        }
    }
}

/// The transcript: an ordered list of entries.
#[derive(Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    /// Whether tool cards show their result (pi's `app.tools.expand`,
    /// Ctrl+O). Collapsed by default: a card is one line.
    tools_expanded: bool,
    /// How a thinking run renders unless its entry overrides it (R6).
    thinking: ThinkingVisibility,
    /// Per-entry render cache (R15): `None` means the entry must be
    /// rendered; a streaming append invalidates only the last entry, so a
    /// long transcript is not re-rendered from scratch on every delta.
    cache: std::cell::RefCell<Vec<CachedRender>>,
}

impl Transcript {
    /// An empty transcript.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether tool cards are expanded (Ctrl+O).
    pub fn tools_expanded(&self) -> bool {
        self.tools_expanded
    }

    /// Toggle tool-card expansion (Ctrl+O; pi's `app.tools.expand`).
    pub fn toggle_tools_expanded(&mut self) {
        self.tools_expanded = !self.tools_expanded;
        self.cache.borrow_mut().clear();
    }

    /// The configured default for thinking runs (R6).
    pub fn thinking_visibility(&self) -> ThinkingVisibility {
        self.thinking
    }

    /// Set the default for thinking runs (R6's `ui.thinking`).
    pub fn set_thinking_visibility(&mut self, visibility: ThinkingVisibility) {
        self.thinking = visibility;
        self.invalidate_cache();
    }

    /// Toggle the *most recent* thinking run's visibility (Ctrl+T; pi's
    /// per-run `thinkingVisibilityOverrides`, `assistant-message.ts`). Runs
    /// already rendered keep the configured default, so expanding the one
    /// being read does not rewrite the transcript behind it.
    pub fn toggle_thinking_expanded(&mut self) {
        let default_expanded = matches!(self.thinking, ThinkingVisibility::Full);
        let Some(Entry::Assistant {
            reasoning,
            thinking_override,
            ..
        }) = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| matches!(entry, Entry::Assistant { .. }))
        else {
            return;
        };
        if reasoning.trim().is_empty() {
            return;
        }
        let visible = thinking_override.unwrap_or(default_expanded);
        *thinking_override = Some(!visible);
        self.invalidate_cache();
    }

    /// Drop every cached render (a mutation that is not a streaming append).
    fn invalidate_cache(&mut self) {
        self.cache.borrow_mut().clear();
    }

    /// Drop the render cache from outside (the theme changed, so every
    /// cached line carries the old palette's bytes).
    pub fn invalidate(&mut self) {
        self.invalidate_cache();
    }

    /// Drop the last entry's cached render (the streaming hot path).
    fn invalidate_last(&mut self) {
        if let Some(last) = self.cache.borrow_mut().last_mut() {
            *last = None;
        }
    }

    /// The entries.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Append a user prompt.
    pub fn push_user(&mut self, text: impl Into<String>) {
        // A user message ends any assistant message before it - the
        // steering boundary - so it never leaves a streaming marker behind.
        self.finish_assistant();
        self.entries.push(Entry::User(text.into()));
    }

    /// Begin an assistant message.
    pub fn begin_assistant(&mut self) {
        self.entries.push(Entry::Assistant {
            text: String::new(),
            reasoning: String::new(),
            streaming: true,
            thinking_override: None,
        });
    }

    /// Append answer text to the current assistant message, starting one if
    /// needed.
    pub fn append_text(&mut self, delta: &str) {
        self.invalidate_last();
        if let Some(Entry::Assistant { text, .. }) = self.entries.last_mut() {
            text.push_str(delta);
        } else {
            self.entries.push(Entry::Assistant {
                text: delta.to_string(),
                reasoning: String::new(),
                streaming: true,
                thinking_override: None,
            });
        }
    }

    /// Append reasoning text to the current assistant message.
    pub fn append_reasoning(&mut self, delta: &str) {
        self.invalidate_last();
        if let Some(Entry::Assistant { reasoning, .. }) = self.entries.last_mut() {
            reasoning.push_str(delta);
        } else {
            self.entries.push(Entry::Assistant {
                text: String::new(),
                reasoning: delta.to_string(),
                streaming: true,
                thinking_override: None,
            });
        }
    }

    /// Mark the current assistant message complete.
    pub fn finish_assistant(&mut self) {
        self.invalidate_cache();
        // Finish every streaming assistant, not just the last: a steered
        // user message splits a turn into more than one assistant entry.
        for entry in &mut self.entries {
            if let Entry::Assistant { streaming, .. } = entry {
                *streaming = false;
            }
        }
    }

    /// Record a tool call.
    pub fn start_tool(&mut self, name: impl Into<String>, args: impl Into<String>) {
        self.entries.push(Entry::Tool {
            name: name.into(),
            args: args.into(),
            status: ToolStatus::Running,
            result: None,
        });
    }

    /// Finish the most recent running tool call.
    pub fn finish_tool(&mut self, status: ToolStatus, result: Option<String>) {
        self.invalidate_cache();
        for entry in self.entries.iter_mut().rev() {
            if let Entry::Tool {
                status: s,
                result: r,
                ..
            } = entry
                && *s == ToolStatus::Running
            {
                *s = status;
                *r = result;
                return;
            }
        }
    }

    /// Finish the most recent running tool call without touching its result
    /// (R4: a streamed `!`/`!!` card keeps what it already appended).
    pub fn finish_tool_status(&mut self, status: ToolStatus) {
        self.invalidate_cache();
        for entry in self.entries.iter_mut().rev() {
            if let Entry::Tool { status: s, .. } = entry
                && *s == ToolStatus::Running
            {
                *s = status;
                return;
            }
        }
    }

    /// Append a notice.
    pub fn push_notice(&mut self, text: impl Into<String>) {
        self.entries.push(Entry::Notice(text.into()));
    }

    /// Append an error.
    pub fn push_error(&mut self, text: impl Into<String>) {
        self.entries.push(Entry::Error(text.into()));
    }

    /// Append a pre-rendered resume line, displayed verbatim.
    pub fn push_raw(&mut self, text: impl Into<String>) {
        self.entries.push(Entry::Raw(text.into()));
    }

    /// Drop every entry (R3's session switch): the new session starts its
    /// own transcript.
    pub fn clear(&mut self) {
        self.invalidate_cache();
        self.entries.clear();
    }

    /// Append a replay of persisted records with the rendering the live
    /// path produced (FR-UI-7): the user band, markdown assistant text,
    /// tool cards, and image labels. The plain `user:`/`assistant:` dump
    /// this replaced could not show a table after a resume - the table had
    /// been flattened to one line per record (manual side-by-side against
    /// pi, 2026-10-01).
    ///
    /// `attachment` resolves a user-attachment hash to media type and bytes
    /// (FR-UI-13); a hash with no file replays as text alone.
    pub fn replay_records(
        &mut self,
        records: &[lca_protocol::Record],
        attachment: Option<&crate::state::LoadAttachment>,
    ) {
        use lca_protocol::{ContentBlock, Record};
        for record in records {
            match record {
                Record::SessionStart { working_dir, .. } => {
                    self.push_raw(format!("[session in {}]", crate::display_path(working_dir)))
                }
                Record::User {
                    content,
                    attachments,
                    ..
                } => {
                    // Render each resolvable attachment as the image card the
                    // live transcript showed; a hash with no file keeps the
                    // record's own stub line, which then names the missing
                    // image (FR-UI-13's named placeholder).
                    let mut resolved: Vec<&String> = Vec::new();
                    for hash in attachments {
                        if let Some((media, bytes)) = attachment.and_then(|load| load(hash)) {
                            let info = ImageInfo::new(media, &bytes);
                            self.push_image(info, bytes);
                            resolved.push(hash);
                        }
                    }
                    // The record's text carries a stub naming each
                    // attachment (assemble.rs: "the message text gains a stub
                    // naming it"); once the card is on screen that stub is a
                    // second copy of the same image. The live transcript
                    // pushed the typed text and never showed it.
                    let text = if resolved.is_empty() {
                        content.clone()
                    } else {
                        content
                            .lines()
                            .filter(|line| {
                                let stub = line.starts_with("[image attachment ");
                                !(stub
                                    && resolved
                                        .iter()
                                        .any(|hash| line.contains(&hash[..8.min(hash.len())])))
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    if !text.trim().is_empty() {
                        self.push_user(text);
                    }
                }
                Record::Assistant {
                    content, reasoning, ..
                } => {
                    let reasoning = reasoning.as_deref().filter(|text| !text.is_empty());
                    if content.is_empty() && reasoning.is_none() {
                        continue;
                    }
                    let mut index = 0;
                    let mut reasoning_done = false;
                    while index < content.len() {
                        // An image is its own entry, as it renders live.
                        if let ContentBlock::Image { media_type, bytes } = &content[index] {
                            let info = ImageInfo::new(media_type, bytes);
                            self.push_image(info, bytes.clone());
                            index += 1;
                            continue;
                        }
                        self.begin_assistant();
                        if !reasoning_done {
                            if let Some(reasoning) = reasoning {
                                self.append_reasoning(reasoning);
                            }
                            reasoning_done = true;
                        }
                        while index < content.len()
                            && !matches!(&content[index], ContentBlock::Image { .. })
                        {
                            match &content[index] {
                                // The same call is persisted as the ToolCall
                                // and ToolResult records that follow, and
                                // the card comes from those - rendering the
                                // block too would show every call twice.
                                ContentBlock::ToolCall { .. } => {}
                                // The record's own reasoning field carries it.
                                ContentBlock::Reasoning { .. } => {}
                                ContentBlock::Text { text } => self.append_text(text),
                                ContentBlock::Image { .. } => {}
                            }
                            index += 1;
                        }
                        self.finish_assistant();
                    }
                    // A reasoning-only answer still earns its entry.
                    if !reasoning_done && let Some(reasoning) = reasoning {
                        self.begin_assistant();
                        self.append_reasoning(reasoning);
                        self.finish_assistant();
                    }
                }
                Record::ToolCall {
                    name, arguments, ..
                } => self.start_tool(name.clone(), arguments.clone()),
                Record::ToolResult {
                    status, content, ..
                } => {
                    let status = match status {
                        lca_protocol::ToolResultStatus::Ok => ToolStatus::Ok,
                        lca_protocol::ToolResultStatus::Error => ToolStatus::Error,
                        lca_protocol::ToolResultStatus::Denied => ToolStatus::Denied,
                        lca_protocol::ToolResultStatus::Timeout => ToolStatus::Timeout,
                    };
                    self.finish_tool(status, content.clone());
                }
                Record::Compaction {
                    summary, strategy, ..
                } => {
                    self.push_raw(format!("[compaction] {summary} (via {strategy})"));
                }
                // Permission decisions, extension events, fork points, and
                // the end marker are transient or structural: the live path
                // shows them as the bottom notice, not as transcript lines.
                _ => {}
            }
        }
    }

    /// Append an image (FR-UI-13, R5): rendered through the graphics ladder.
    pub fn push_image(&mut self, info: ImageInfo, bytes: Vec<u8>) {
        self.entries.push(Entry::Image {
            info,
            bytes: std::sync::Arc::new(bytes),
        });
    }

    /// Append streamed tool output to the most recent running tool card.
    pub fn append_tool_output(&mut self, chunk: &str) {
        self.invalidate_last();
        for entry in self.entries.iter_mut().rev() {
            if let Entry::Tool { result, .. } = entry {
                result.get_or_insert_with(String::new).push_str(chunk);
                return;
            }
        }
        self.entries.push(Entry::Tool {
            name: "tool".into(),
            args: String::new(),
            status: ToolStatus::Running,
            result: Some(chunk.to_string()),
        });
    }

    /// Render every entry to styled lines at `width`.
    pub fn render(&self, width: u16, theme: &Theme) -> Vec<String> {
        let mut cache = self.cache.borrow_mut();
        if cache.len() != self.entries.len() {
            cache.resize(self.entries.len(), None);
        }
        let mut out = Vec::new();
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                out.push(String::new());
            }
            if let Some((cached_width, lines)) = &cache[i]
                && *cached_width == width
            {
                out.extend(lines.iter().cloned());
                continue;
            }
            let mut lines = Vec::new();
            render_entry(
                entry,
                width,
                theme,
                self.tools_expanded,
                self.thinking,
                &mut lines,
            );
            out.extend(lines.iter().cloned());
            cache[i] = Some((width, lines));
        }
        out
    }

    /// The document line index where each user message starts, at `width`
    /// (FR-UI-11's prompt jump).
    pub fn user_offsets(&self, width: u16, theme: &Theme) -> Vec<usize> {
        let mut offsets = Vec::new();
        let mut line = 0usize;
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                line += 1;
            }
            if matches!(entry, Entry::User(_)) {
                offsets.push(line);
            }
            let mut tmp = Vec::new();
            render_entry(
                entry,
                width,
                theme,
                self.tools_expanded,
                self.thinking,
                &mut tmp,
            );
            line += tmp.len();
        }
        offsets
    }

    /// The height (line count) at a width.
    pub fn height(&self, width: u16, theme: &Theme) -> usize {
        self.render(width, theme).len()
    }
}

/// A one-line image label (media type, dimensions, size) for resume lines.
pub fn image_label(media_type: &str, bytes: &[u8]) -> String {
    let info = ImageInfo::new(media_type, bytes);
    let dimensions = match (info.width, info.height) {
        (Some(w), Some(h)) => format!("{w}×{h}"),
        _ => "unknown size".to_string(),
    };
    format!("[image {media_type}, {dimensions}, {} bytes]", bytes.len())
}

/// Markdown options for the terminal: the link mode follows the terminal's
/// OSC 8 capability, so a URL never vanishes on a terminal that swallows
/// the hyperlink (pi's `markdown.md` §6).
fn markdown_options() -> MarkdownOptions {
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
        ..Default::default()
    }
}

fn render_entry(
    entry: &Entry,
    width: u16,
    theme: &Theme,
    tools_expanded: bool,
    thinking: ThinkingVisibility,
    out: &mut Vec<String>,
) {
    match entry {
        Entry::User(text) => render_user(text, width, theme, out),
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

fn render_user(text: &str, width: u16, theme: &Theme, out: &mut Vec<String>) {
    // pi's user bubble: `Box(outputPad = 1, 1, theme.bg("userMessageBg"))`
    // around the user's own markdown in `userMessageText` - a full-width
    // band, content padded one column inside it, one blank band row above
    // and below. The `› ` marker stays: color is never the only signal
    // (NFR-28).
    let width = width as usize;
    let bg = theme.bg(Role::UserMessageBg);
    let content = width.saturating_sub(3).max(1);
    let wrapped = wrap_text_with_ansi(text, content);
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

fn render_assistant(
    text: &str,
    reasoning: &str,
    streaming: bool,
    thinking: ThinkingVisibility,
    width: u16,
    theme: &Theme,
    out: &mut Vec<String>,
) {
    if !reasoning.is_empty() {
        match thinking {
            ThinkingVisibility::Full => {
                for line in
                    wrap_text_with_ansi(reasoning, (width as usize).saturating_sub(2).max(1))
                {
                    out.push(format!(
                        "{} {}",
                        (theme.reasoning)("∴"),
                        (theme.reasoning)(&line)
                    ));
                }
            }
            // R6's default: enough thinking to see where the model is
            // going, then a count of what is left.
            ThinkingVisibility::Snippet => {
                const SNIPPET_LINES: usize = 3;
                let lines: Vec<&str> = reasoning
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .collect();
                for line in lines.iter().take(SNIPPET_LINES) {
                    for wrapped in
                        wrap_text_with_ansi(line, (width as usize).saturating_sub(2).max(1))
                    {
                        out.push(format!(
                            "{} {}",
                            (theme.reasoning)("∴"),
                            (theme.reasoning)(&wrapped)
                        ));
                    }
                }
                if lines.len() > SNIPPET_LINES {
                    out.push(format!(
                        "{} {}",
                        (theme.reasoning)("∴"),
                        (theme.reasoning)(&format!(
                            "… +{} lines ({} to expand)",
                            lines.len() - SNIPPET_LINES,
                            lca_tui::engine::keybindings::key_text("app.thinking.toggle")
                        ))
                    ));
                }
            }
            ThinkingVisibility::Hidden => {
                // pi's original: one dim line (`messages.md` §3).
                out.push(format!(
                    "{} {}",
                    (theme.reasoning)("∴"),
                    (theme.reasoning)(&format!(
                        "Thinking… ({} to expand)",
                        lca_tui::engine::keybindings::key_text("app.thinking.toggle")
                    ))
                ));
            }
        }
    }
    if !text.is_empty() {
        let md = render_markdown(text, width as usize, &theme.markdown(), &markdown_options());
        out.extend(md);
    }
    if streaming {
        out.push((theme.dim)("▍"));
    }
}

/// A one-line argument summary for a tool card (pi's per-tool formats,
/// `messages.md` §6). Unknown tools fall back to the raw arguments.
fn format_tool_args(name: &str, args: &str) -> String {
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
    } = entry
    else {
        return;
    };
    let width = width as usize;
    let inner = width.saturating_sub(3).max(1);
    let summary = format_tool_args(name, args);
    let args = if summary.is_empty() {
        String::new()
    } else {
        format!(" {summary}")
    };
    let status_text = match status {
        ToolStatus::Ok => (theme.success)(status.label()),
        ToolStatus::Error | ToolStatus::Denied => (theme.error)(status.label()),
        ToolStatus::Timeout => (theme.warn)(status.label()),
        ToolStatus::Running => (theme.dim)(status.label()),
    };
    // pi paints a shell card's title in `bashMode` (`bash-execution.ts`:
    // `fg("bashMode", bold("$ cmd"))`); every other tool keeps `toolTitle`.
    let title_style = if matches!(name.as_str(), "bash" | "shell") {
        theme.role(Role::BashMode)
    } else {
        theme.tool.clone()
    };
    let header = format!(
        "{} {}{} {}",
        (theme.tool)(">"),
        title_style(name),
        args,
        status_text
    );
    let mut rows = vec![truncate_to_width(&header, inner, "…", false)];
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

#[cfg(test)]
mod tests {
    use super::*;
    use lca_tui::engine::text::strip_terminal_sequences;

    fn plain() -> Theme {
        Theme::plain()
    }

    fn strip(lines: &[String]) -> Vec<String> {
        lines.iter().map(|l| strip_terminal_sequences(l)).collect()
    }

    #[test]
    fn user_prompts_render_with_a_marker() {
        let mut t = Transcript::new();
        t.push_user("hello there");
        let out = strip(&t.render(40, &plain()));
        let row = out
            .iter()
            .find(|l| l.contains("› "))
            .expect("the marker row");
        assert!(row.contains("hello there"), "{out:?}");
    }

    // Verifies: R1 - the user prompt renders as a full-width
    // `userMessageBg` band: every row painted on the background role, each
    // exactly the render width, with the marker row inside it.
    #[test]
    fn the_user_message_is_a_full_width_band() {
        let theme = Theme::colored();
        let width = 40usize;
        let mut t = Transcript::new();
        t.push_user("hello there");
        let rows = t.render(width as u16, &theme);
        let bg = "\x1b[48;2;52;53;65m"; // #343541, pi's dark `userMessageBg`
        assert!(
            rows.iter().all(|r| r.contains(bg)),
            "every band row is on the user background: {rows:?}"
        );
        for row in &rows {
            assert_eq!(
                lca_tui::engine::text::visible_width(row),
                width,
                "the band fills the row: {row:?}"
            );
        }
        assert!(
            rows.iter()
                .any(|r| r.contains("› ") && r.contains("hello there")),
            "the marker sits inside the band: {rows:?}"
        );
        assert!(
            rows.iter().all(|r| r.ends_with("\x1b[49m")),
            "the band closes its own channel, so the row beside it is clean"
        );
        // Assistant text stays on the default background (pi's choice -
        // the contrast is the point).
        let mut t = Transcript::new();
        t.begin_assistant();
        t.append_text("an answer");
        t.finish_assistant();
        assert!(
            t.render(width as u16, &theme)
                .iter()
                .all(|r| !r.contains(bg)),
            "assistant text is not banded"
        );
    }

    // Verifies: R1 - a tool card's background is its state: pending while
    // the call is in flight, the quiet success tint when it settled, the
    // error tint when it did not (pi's `updateDisplay`).
    #[test]
    fn tool_cards_carry_the_states_background() {
        let theme = Theme::colored();
        let paint = |status: ToolStatus, result: Option<&str>| {
            let mut t = Transcript::new();
            t.start_tool("read", r#"{"path":"a.rs"}"#);
            t.finish_tool(status, result.map(str::to_string));
            t.render(60, &theme)
        };
        let pending = paint(ToolStatus::Running, Some("partial"));
        let ok = paint(ToolStatus::Ok, Some("done"));
        let failed = paint(ToolStatus::Error, Some("boom"));
        // #282832, #283228, #3c2828
        for (rows, expected) in [
            (&pending, "\x1b[48;2;40;40;50m"),
            (&ok, "\x1b[48;2;40;50;40m"),
            (&failed, "\x1b[48;2;60;40;40m"),
        ] {
            assert!(
                rows.iter().all(|r| r.contains(expected)),
                "every card row carries {expected}: {rows:?}"
            );
            assert!(
                rows.iter()
                    .all(|r| { lca_tui::engine::text::visible_width(r) == 60 }),
                "the card fills each row: {rows:?}"
            );
        }
        assert!(
            pending.iter().any(|r| r.contains("…")),
            "the pending state carries its own symbol, not just a color (NFR-28): {pending:?}"
        );
        assert!(ok.iter().any(|r| r.contains("ok")), "{ok:?}");
        assert!(failed.iter().any(|r| r.contains("error")), "{failed:?}");
    }

    // Verifies: FR-UI-5 - the plain theme paints no background at all, so
    // an 80-column colorless terminal reads the same bands as text.
    #[test]
    fn the_plain_theme_paints_no_band() {
        let mut t = Transcript::new();
        t.push_user("hi");
        t.start_tool("read", r#"{"path":"a.rs"}"#);
        t.finish_tool(ToolStatus::Ok, Some("ok".into()));
        let rows = strip(&t.render(50, &plain()));
        assert!(rows.iter().any(|r| r.contains("› hi")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("> read")), "{rows:?}");
        let raw = t.render(50, &plain());
        assert!(
            raw.iter().all(|r| !r.contains('\x1b')),
            "the plain theme emits no SGR at all: {raw:?}"
        );
    }

    #[test]
    fn assistant_markdown_renders() {
        let mut t = Transcript::new();
        t.begin_assistant();
        t.append_text("# Title\n\n- a\n- b");
        t.finish_assistant();
        let out = strip(&t.render(40, &plain()));
        assert!(out.iter().any(|l| l.trim() == "Title"), "{out:?}");
        assert!(
            out.iter().any(|l| l.trim_start().starts_with("- a")),
            "{out:?}"
        );
    }

    // Verifies: FR-UI-22 (R6) - a thinking run shows a short snippet by
    // default: the first few non-empty lines, then a count of the rest.
    #[test]
    fn reasoning_shows_a_snippet_by_default() {
        let mut t = Transcript::new();
        t.append_reasoning("one\ntwo\nthree\nfour\nfive");
        t.append_text("the answer");
        t.finish_assistant();
        let lines = strip(&t.render(40, &plain()));
        let shown: Vec<&String> = lines.iter().filter(|l| l.starts_with('∴')).collect();
        assert_eq!(shown.len(), 4, "three lines plus the marker: {lines:?}");
        assert!(shown[0].contains("one"), "{lines:?}");
        assert!(shown[2].contains("three"), "{lines:?}");
        assert!(
            shown[3].contains("… +2 lines") && shown[3].contains("ctrl+t to expand"),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains("four")), "{lines:?}");
    }

    // Verifies: FR-UI-22 (R6) - the toggle expands the run it is on, in
    // place, to the full block; the answer below it is untouched.
    #[test]
    fn the_thinking_toggle_expands_the_run_in_place() {
        let mut t = Transcript::new();
        t.append_reasoning("one\ntwo\nthree\nfour\nfive");
        t.append_text("the answer");
        t.finish_assistant();
        t.toggle_thinking_expanded();
        let lines = strip(&t.render(40, &plain()));
        for expected in ["one", "two", "three", "four", "five"] {
            assert!(
                lines.iter().any(|l| l.contains(expected)),
                "{expected} after expanding: {lines:?}"
            );
        }
        assert!(
            !lines.iter().any(|l| l.contains("+2 lines")),
            "no marker once expanded: {lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("the answer")));
    }

    // Verifies: FR-UI-22 (R6) - `full` and `hidden` are settings values:
    // each renders its shape with no per-run toggle involved.
    #[test]
    fn thinking_visibility_full_and_hidden_are_settings() {
        let mut full = Transcript::new();
        full.set_thinking_visibility(ThinkingVisibility::Full);
        full.append_reasoning("alpha\nbeta\ngamma\ndelta");
        full.append_text("answer");
        full.finish_assistant();
        let lines = strip(&full.render(40, &plain()));
        assert!(lines.iter().any(|l| l.contains("delta")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("+1 lines")), "{lines:?}");

        let mut hidden = Transcript::new();
        hidden.set_thinking_visibility(ThinkingVisibility::Hidden);
        hidden.append_reasoning("alpha\nbeta");
        hidden.append_text("answer");
        hidden.finish_assistant();
        let lines = strip(&hidden.render(40, &plain()));
        assert!(lines.iter().any(|l| l.contains("Thinking…")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("alpha")), "{lines:?}");
    }

    // Verifies: FR-UI-22 (R6) - the toggle is per run (pi's
    // thinkingVisibilityOverrides): expanding the latest run leaves the
    // earlier one on the configured default.
    #[test]
    fn the_thinking_toggle_only_changes_the_latest_run() {
        let mut t = Transcript::new();
        t.append_reasoning("first run");
        t.append_text("answer one");
        t.finish_assistant();
        t.append_reasoning("second run");
        t.append_text("answer two");
        t.finish_assistant();
        t.toggle_thinking_expanded();
        let lines = strip(&t.render(40, &plain()));
        // The second run's reasoning is now shown as-is; the first run's
        // still shows as a snippet (one line, so no marker).
        assert_eq!(
            lines.iter().filter(|l| l.contains("second run")).count(),
            1,
            "{lines:?}"
        );
        assert_eq!(
            lines.iter().filter(|l| l.contains("first run")).count(),
            1,
            "{lines:?}"
        );
    }

    // Verifies: R15 - the per-entry render cache never serves stale lines.
    #[test]
    fn the_render_cache_reflects_appends_and_finishes() {
        let mut t = Transcript::new();
        t.begin_assistant();
        t.append_text("first");
        let a = strip(&t.render(40, &plain()));
        assert!(a.iter().any(|l| l.contains("first")));
        t.append_text(" second");
        let b = strip(&t.render(40, &plain()));
        assert!(b.iter().any(|l| l.contains("first second")), "{b:?}");
        t.start_tool("read", r#"{"path":"a"}"#);
        t.finish_tool(ToolStatus::Ok, Some("ok".into()));
        t.toggle_tools_expanded();
        let c = strip(&t.render(40, &plain()));
        assert!(c.iter().any(|l| l.contains("ok")), "{c:?}");
    }

    #[test]
    fn tool_cards_name_the_tool_not_the_call_id() {
        let mut t = Transcript::new();
        t.start_tool("read", r#"{"path":"a.rs"}"#);
        t.finish_tool(ToolStatus::Ok, Some("ok".into()));
        let out = strip(&t.render(60, &plain()));
        let header = out
            .iter()
            .find(|l| l.contains("> read"))
            .expect("the card header row");
        assert!(header.contains("ok"), "{out:?}");
        assert!(!header.contains("call_"));
    }

    // Verifies: R8 - a tool card collapses to one line and expands on demand.
    #[test]
    fn a_tool_card_collapses_and_expands() {
        let mut t = Transcript::new();
        t.start_tool("read", r#"{"path":"a.rs"}"#);
        t.finish_tool(
            ToolStatus::Ok,
            Some("line one\nline two\nline three".into()),
        );
        let collapsed = strip(&t.render(60, &plain()));
        assert!(
            collapsed.iter().any(|l| l.contains("> read a.rs ok")),
            "{collapsed:?}"
        );
        assert!(collapsed.iter().any(|l| l.contains("ctrl+o to expand")));
        assert!(!collapsed.iter().any(|l| l.contains("line two")));
        t.toggle_tools_expanded();
        let expanded = strip(&t.render(60, &plain()));
        assert!(expanded.iter().any(|l| l.contains("line two")));
    }

    // Verifies: R8 - per-tool one-line argument summaries.
    #[test]
    fn tool_arguments_render_as_a_one_line_summary() {
        assert_eq!(format_tool_args("read", r#"{"path":"a.rs"}"#), "a.rs");
        assert_eq!(
            format_tool_args("bash", r#"{"command":"ls -la"}"#),
            "ls -la"
        );
        assert_eq!(
            format_tool_args("grep", r#"{"pattern":"foo","path":"."}"#),
            "/foo/"
        );
        assert_eq!(format_tool_args("list", r#"{"path":"."}"#), ".");
        assert_eq!(format_tool_args("other", "raw"), "raw");
    }

    // Verifies: R8 - a command card shows a bounded preview when collapsed.
    #[test]
    fn a_shell_card_previews_its_output() {
        let mut t = Transcript::new();
        t.start_tool("shell", r#"{"command":"ls"}"#);
        let output: String = (1..=8).map(|i| format!("line {i}\n")).collect();
        t.finish_tool(ToolStatus::Ok, Some(output));
        let out = strip(&t.render(60, &plain()));
        assert!(out.iter().any(|l| l.contains("> shell ls ok")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("line 1")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("line 5")), "{out:?}");
        assert!(!out.iter().any(|l| l.contains("line 6")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("3 more lines")), "{out:?}");
    }

    // Verifies: R8 - a read card stays one line when collapsed.
    #[test]
    fn a_read_card_stays_one_line() {
        let mut t = Transcript::new();
        t.start_tool("read", r#"{"path":"a.rs"}"#);
        t.finish_tool(ToolStatus::Ok, Some("1  fn main() {}\n2  more\n".into()));
        let out = strip(&t.render(60, &plain()));
        assert!(out.iter().any(|l| l.contains("> read a.rs ok")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("ctrl+o to expand")));
        assert!(!out.iter().any(|l| l.contains("fn main")), "{out:?}");
    }

    #[test]
    fn entries_are_separated_by_a_blank_line() {
        let mut t = Transcript::new();
        t.push_user("q");
        t.append_text("a");
        t.finish_assistant();
        let out = t.render(40, &plain());
        assert!(out.iter().any(|l| l.is_empty()));
    }

    #[test]
    fn streaming_marker_disappears_when_finished() {
        let mut t = Transcript::new();
        t.begin_assistant();
        t.append_text("partial");
        let streaming = strip(&t.render(40, &plain()));
        assert!(streaming.iter().any(|l| l.contains("▍")));
        t.finish_assistant();
        let done = strip(&t.render(40, &plain()));
        assert!(!done.iter().any(|l| l.contains("▍")));
    }

    // Verifies: FR-CORE-11 - a steered user message ends the assistant
    // message before it, so no stale streaming marker survives the
    // boundary (a turn split by a steer has more than one assistant entry).
    #[test]
    fn a_steer_finalizes_the_assistant_before_it() {
        let mut t = Transcript::new();
        t.begin_assistant();
        t.append_text("first call");
        t.push_user("steer");
        t.append_text("second call");
        let out = strip(&t.render(60, &plain()));
        let streaming = out.iter().filter(|l| l.contains('▍')).count();
        assert_eq!(streaming, 1, "only the live assistant streams:\n{out:?}");
    }

    // Verifies: FR-UI-13 - an image renders with its media type and
    // dimensions, never silently dropped.
    #[test]
    fn image_entries_render_a_placeholder() {
        let mut t = Transcript::new();
        t.push_image(
            ImageInfo {
                media_type: "image/png".into(),
                bytes: 1024,
                width: Some(10),
                height: Some(20),
                alt: Some("a chart".into()),
            },
            Vec::new(),
        );
        let out = strip(&t.render(60, &plain()));
        assert!(
            out.iter()
                .any(|l| l.contains("image/png") && l.contains("10×20") && l.contains("a chart")),
            "{out:?}"
        );
    }
}
