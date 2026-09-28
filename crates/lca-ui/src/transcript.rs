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
use lca_tui::widgets::markdown::{MarkdownOptions, render_markdown};

use crate::theme::Theme;

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

/// The transcript: an ordered list of entries.
#[derive(Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    /// Whether tool cards show their result (pi's `app.tools.expand`,
    /// Ctrl+O). Collapsed by default: a card is one line.
    tools_expanded: bool,
    /// Whether thinking runs show their text (pi's `hideThinkingBlock`,
    /// Ctrl+T). Collapsed by default: one dim line (R8).
    thinking_expanded: bool,
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

    /// Toggle thinking-run expansion (Ctrl+T; pi's `hideThinkingBlock`).
    pub fn toggle_thinking_expanded(&mut self) {
        self.thinking_expanded = !self.thinking_expanded;
        self.cache.borrow_mut().clear();
    }

    /// Drop every cached render (a mutation that is not a streaming append).
    fn invalidate_cache(&mut self) {
        self.cache.borrow_mut().clear();
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

    /// Replace the whole transcript (R3's session switch): the new session's
    /// display lines, each shown verbatim.
    pub fn replace(&mut self, lines: Vec<String>) {
        self.invalidate_cache();
        self.entries.clear();
        for line in lines {
            self.entries.push(Entry::Raw(line));
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
                self.thinking_expanded,
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
                self.thinking_expanded,
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

fn render_entry(
    entry: &Entry,
    width: u16,
    theme: &Theme,
    tools_expanded: bool,
    thinking_expanded: bool,
    out: &mut Vec<String>,
) {
    match entry {
        Entry::User(text) => render_user(text, width, theme, out),
        Entry::Assistant {
            text,
            reasoning,
            streaming,
        } => render_assistant(
            text,
            reasoning,
            *streaming,
            thinking_expanded,
            width,
            theme,
            out,
        ),
        Entry::Tool { .. } => render_tool(entry, tools_expanded, width, theme, out),
        Entry::Notice(text) => {
            out.extend(wrap_text_with_ansi(&(theme.dim)(text), width as usize));
        }
        Entry::Error(text) => {
            out.extend(wrap_text_with_ansi(&(theme.error)(text), width as usize));
        }
        Entry::Raw(text) => {
            out.extend(wrap_text_with_ansi(text, width as usize));
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

fn render_user(text: &str, width: u16, theme: &Theme, out: &mut Vec<String>) {
    // A distinct band: the prompt on a styled, indented row (#5).
    let prefix = (theme.user)("› ");
    let inner_width = (width as usize).saturating_sub(2).max(1);
    let wrapped = wrap_text_with_ansi(text, inner_width);
    for (i, line) in wrapped.iter().enumerate() {
        if i == 0 {
            out.push(format!("{prefix}{}", (theme.user)(line)));
        } else {
            out.push(format!("  {}", (theme.user)(line)));
        }
    }
}

fn render_assistant(
    text: &str,
    reasoning: &str,
    streaming: bool,
    thinking_expanded: bool,
    width: u16,
    theme: &Theme,
    out: &mut Vec<String>,
) {
    if !reasoning.is_empty() {
        if thinking_expanded {
            for line in wrap_text_with_ansi(reasoning, (width as usize).saturating_sub(2).max(1)) {
                out.push(format!(
                    "{} {}",
                    (theme.reasoning)("∴"),
                    (theme.reasoning)(&line)
                ));
            }
        } else {
            // pi hides the run behind one dim line (`messages.md` §3).
            out.push(format!(
                "{} {}",
                (theme.reasoning)("∴"),
                (theme.reasoning)("Thinking… (ctrl+t to expand)")
            ));
        }
    }
    if !text.is_empty() {
        let md = render_markdown(
            text,
            width as usize,
            &theme.markdown(),
            &MarkdownOptions::default(),
        );
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
        "read" | "write" | "edit" | "ls" => json_string_field(args, "path"),
        "bash" => json_string_field(args, "command"),
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
    let header = format!(
        "{} {}{} {}",
        (theme.tool)(">"),
        (theme.tool)(name),
        args,
        status_text
    );
    out.push(truncate_to_width(&header, width as usize, "…", false));
    // pi collapses a tool card to its one line and expands on demand
    // (`app.tools.expand`, Ctrl+O). The hint names the key when there is
    // more to see.
    if let Some(result) = result {
        if expanded {
            for line in wrap_text_with_ansi(result, (width as usize).saturating_sub(2).max(1)) {
                out.push(format!("  {}", (theme.dim)(&line)));
            }
        } else if result.lines().count() > 1 {
            out.push(format!("  {}", (theme.dim)("… (ctrl+o to expand)")));
        }
    }
    let _ = visible_width("");
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
        assert_eq!(out[0], "› hello there");
    }

    #[test]
    fn assistant_markdown_renders() {
        let mut t = Transcript::new();
        t.begin_assistant();
        t.append_text("# Title\n\n- a\n- b");
        t.finish_assistant();
        let out = strip(&t.render(40, &plain()));
        assert!(out.iter().any(|l| l == "Title"));
        assert!(out.iter().any(|l| l.starts_with("• a")));
    }

    #[test]
    fn reasoning_is_collapsed_by_default_and_expands() {
        let mut t = Transcript::new();
        t.append_reasoning("thinking hard");
        t.append_text("the answer");
        t.finish_assistant();
        let collapsed = strip(&t.render(40, &plain()));
        assert!(collapsed[0].contains("ctrl+t to expand"), "{collapsed:?}");
        assert!(!collapsed.iter().any(|l| l.contains("thinking hard")));
        t.toggle_thinking_expanded();
        let expanded = strip(&t.render(40, &plain()));
        assert!(expanded[0].starts_with("∴ thinking hard"));
        assert!(expanded.iter().any(|l| l.contains("the answer")));
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
        assert!(out[0].starts_with("> read"));
        assert!(out[0].ends_with("ok"));
        assert!(!out[0].contains("call_"));
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
        assert_eq!(collapsed[0], "> read a.rs ok");
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
        assert_eq!(format_tool_args("other", "raw"), "raw");
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
