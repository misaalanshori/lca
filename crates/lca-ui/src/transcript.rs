//! The transcript, ported from pi's
//! `coding-agent/src/modes/interactive/components/messages.ts`
//! (`pi-tui-re/src_re/agent-components/messages.md`).
//!
//! Owner issues #5 (user prompts in history), #6 (markdown, message
//! separation) and #4 (streaming) live here: the transcript is a list of
//! entries rendered to styled lines, updated incrementally as a turn
//! streams, and the renderer repaints only what changed.

use lca_tui::engine::text::{truncate_to_width, visible_width, wrap_text_with_ansi};
use lca_tui::widgets::image::{ImageInfo, render_image_placeholder};
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
    /// An image the terminal shows as a placeholder (FR-UI-13).
    Image(ImageInfo),
}

/// The transcript: an ordered list of entries.
#[derive(Default)]
pub struct Transcript {
    entries: Vec<Entry>,
}

impl Transcript {
    /// An empty transcript.
    pub fn new() -> Self {
        Self::default()
    }

    /// The entries.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Append a user prompt.
    pub fn push_user(&mut self, text: impl Into<String>) {
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
        if let Some(Entry::Assistant { streaming, .. }) = self.entries.last_mut() {
            *streaming = false;
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

    /// Append an image placeholder (FR-UI-13).
    pub fn push_image(&mut self, info: ImageInfo) {
        self.entries.push(Entry::Image(info));
    }

    /// Append streamed tool output to the most recent running tool card.
    pub fn append_tool_output(&mut self, chunk: &str) {
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
        let mut out = Vec::new();
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                out.push(String::new()); // separate messages (#6)
            }
            match entry {
                Entry::User(text) => render_user(text, width, theme, &mut out),
                Entry::Assistant {
                    text,
                    reasoning,
                    streaming,
                } => render_assistant(text, reasoning, *streaming, width, theme, &mut out),
                Entry::Tool {
                    name,
                    args,
                    status,
                    result,
                } => render_tool(
                    name,
                    args,
                    *status,
                    result.as_deref(),
                    width,
                    theme,
                    &mut out,
                ),
                Entry::Notice(text) => {
                    out.extend(wrap_text_with_ansi(&(theme.dim)(text), width as usize));
                }
                Entry::Error(text) => {
                    out.extend(wrap_text_with_ansi(&(theme.error)(text), width as usize));
                }
                Entry::Raw(text) => {
                    out.extend(wrap_text_with_ansi(text, width as usize));
                }
                Entry::Image(info) => {
                    out.extend(render_image_placeholder(info, width as usize));
                }
            }
        }
        out
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
    width: u16,
    theme: &Theme,
    out: &mut Vec<String>,
) {
    if !reasoning.is_empty() {
        for line in wrap_text_with_ansi(reasoning, (width as usize).saturating_sub(2).max(1)) {
            out.push(format!(
                "{} {}",
                (theme.reasoning)("∴"),
                (theme.reasoning)(&line)
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

fn render_tool(
    name: &str,
    args: &str,
    status: ToolStatus,
    result: Option<&str>,
    width: u16,
    theme: &Theme,
    out: &mut Vec<String>,
) {
    let args = if args.is_empty() {
        String::new()
    } else {
        format!(" {args}")
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
    if let Some(result) = result {
        let preview: String = result.lines().take(6).collect::<Vec<_>>().join("\n");
        for line in wrap_text_with_ansi(&preview, (width as usize).saturating_sub(2).max(1)) {
            out.push(format!("  {}", (theme.dim)(&line)));
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
    fn reasoning_is_separated_from_the_answer() {
        let mut t = Transcript::new();
        t.append_reasoning("thinking hard");
        t.append_text("the answer");
        t.finish_assistant();
        let out = strip(&t.render(40, &plain()));
        assert!(out[0].starts_with("∴ thinking hard"));
        assert!(out.iter().any(|l| l.contains("the answer")));
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

    // Verifies: FR-UI-13 - an image renders with its media type and
    // dimensions, never silently dropped.
    #[test]
    fn image_entries_render_a_placeholder() {
        let mut t = Transcript::new();
        t.push_image(ImageInfo {
            media_type: "image/png".into(),
            bytes: 1024,
            width: Some(10),
            height: Some(20),
            alt: Some("a chart".into()),
        });
        let out = strip(&t.render(60, &plain()));
        assert!(
            out.iter()
                .any(|l| l.contains("image/png") && l.contains("10×20") && l.contains("a chart")),
            "{out:?}"
        );
    }
}
