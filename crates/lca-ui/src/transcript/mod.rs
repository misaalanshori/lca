//! The transcript, ported from pi's
//! `coding-agent/src/modes/interactive/components/messages.ts`
//! (`pi-tui-re/src_re/agent-components/messages.md`).
//!
//! Owner issues #5 (user prompts in history), #6 (markdown, message
//! separation) and #4 (streaming) live here: the transcript is a list of
//! entries rendered to styled lines, updated incrementally as a turn
//! streams, and the renderer repaints only what changed.

use lca_tui::widgets::image::ImageInfo;

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
        /// This run's thinking visibility, when a click or Ctrl+T
        /// cycled it (gh #166, R6); `None` follows [`Transcript`]'s
        /// configured default.
        thinking_override: Option<ThinkingVisibility>,
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
        /// The structured diff the tool returned in `extras["diff"]`
        /// (gh #9): `Some` renders the card as pi's diff card - its own
        /// line kinds in `toolDiffAdded`/`toolDiffRemoved` - instead of
        /// an ordinary result preview. Carried, never re-parsed out of
        /// display text (the DNA box for this cycle).
        diff: Option<String>,
        /// The call came from the editor's `!`/`!!` line rather than from
        /// the model. pi splits these into two components
        /// (`BashExecutionComponent` in `bashMode`, the tool card in
        /// `toolTitle`); LCA has one card, so the flag carries which
        /// pi treatment applies to its title.
        manual: bool,
        /// This card's expansion, when a click toggled it (gh #166);
        /// `None` follows the global Ctrl+O default.
        expanded: Option<bool>,
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

/// One entry's cached render: the width it was rendered at, its styled
/// lines, and how many leading lines belong to its reasoning run (gh
/// #11's hit map - `None` means the entry must be re-rendered).
type CachedRender = Option<(u16, Vec<String>, usize)>;

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

/// What a transcript document row belongs to (gh #166's click and
/// hover map): thinking rows cycle, tool headers expand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryHit {
    /// A reasoning row of an assistant entry (by entry index).
    Thinking(usize),
    /// The first row of a tool card (by entry index).
    ToolHeader(usize),
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
    /// How fenced code blocks are framed (gh #32), read by every render.
    codeblock_border: lca_tui::widgets::markdown::CodeBlockBorder,
    /// Pre-parse markdown transforms in registration order (gh #12):
    /// pi's `registerMarkdownTransformer`. Empty by default - no
    /// consumer, no rewriting.
    markdown_transformers: Vec<lca_tui::widgets::markdown::MarkdownTransformer>,
    /// The live display tunables (gh #82): margin, image display,
    /// mermaid mode, and code indent, synced from the config hook.
    tuning: crate::state::DisplayTuning,
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

    /// How fenced code blocks are framed (gh #32): set once from the
    /// options when the chat is built, like the thinking visibility.
    pub fn set_codeblock_border(&mut self, border: lca_tui::widgets::markdown::CodeBlockBorder) {
        self.codeblock_border = border;
    }

    /// Sync the display tunables from the config hook (gh #82) and drop
    /// the render cache they key: a margin or mode change re-renders.
    pub fn apply_display(&mut self, tuning: &crate::state::DisplayTuning) {
        // The tick syncs every frame: only a real change drops the
        // render cache (R15), or scrolling would re-render always.
        if self.tuning != *tuning {
            self.tuning = tuning.clone();
            self.invalidate();
        }
    }

    /// Set the default for thinking runs (R6's `ui.thinking`).
    pub fn set_thinking_visibility(&mut self, visibility: ThinkingVisibility) {
        self.thinking = visibility;
        self.invalidate_cache();
    }

    /// Cycle the *most recent* thinking run's visibility (Ctrl+T; pi's
    /// per-run `thinkingVisibilityOverrides`, `assistant-message.ts`). Runs
    /// already rendered keep the configured default, so expanding the one
    /// being read does not rewrite the transcript behind it.
    pub fn toggle_thinking_expanded(&mut self) {
        // FR-UI-22: the key overrides *the run it is pressed on*. With no
        // pointer, that is the newest run that actually has reasoning - the
        // newest assistant message is often a tool report with none, and a
        // dead key while a `ctrl+t to expand` marker is on screen is a
        // broken promise.
        let Some(index) = self.entries.iter().rposition(|entry| {
            matches!(entry, Entry::Assistant { reasoning, .. } if !reasoning.trim().is_empty())
        }) else {
            return;
        };
        self.toggle_entry_thinking(index);
    }

    /// Cycle one thinking run by entry index (gh #166's click path; the
    /// keybinding above is `toggle_thinking_expanded`): snippet → full
    /// → hidden → snippet, starting from the run's effective state.
    /// Only an assistant entry carrying reasoning cycles; anything else
    /// reports `false` and moves nothing.
    pub fn toggle_entry_thinking(&mut self, index: usize) -> bool {
        let current = self.thinking;
        let Some(Entry::Assistant {
            reasoning,
            thinking_override,
            ..
        }) = self.entries.get_mut(index)
        else {
            return false;
        };
        if reasoning.trim().is_empty() {
            return false;
        }
        let effective = thinking_override.unwrap_or(current);
        *thinking_override = Some(match effective {
            ThinkingVisibility::Snippet => ThinkingVisibility::Full,
            ThinkingVisibility::Full => ThinkingVisibility::Hidden,
            ThinkingVisibility::Hidden => ThinkingVisibility::Snippet,
        });
        self.invalidate_cache();
        true
    }

    /// A tool card's effective expansion (gh #210): `None` for
    /// anything that is not a showable card.
    pub fn tool_card_expanded(&self, index: usize) -> Option<bool> {
        let Entry::Tool {
            result,
            diff,
            expanded,
            ..
        } = self.entries.get(index)?
        else {
            return None;
        };
        let showable = result.as_ref().is_some_and(|text| !text.is_empty())
            || diff.as_ref().is_some_and(|text| !text.is_empty());
        showable.then(|| expanded.unwrap_or(self.tools_expanded))
    }

    /// Toggle one tool card's expansion by entry index (gh #166's click
    /// path): the card's own override wins over the global Ctrl+O
    /// default. Only a tool entry with output to show toggles; a bare
    /// running card reports `false` and arms nothing.
    pub fn toggle_entry_tool(&mut self, index: usize) -> bool {
        let global = self.tools_expanded;
        let Some(Entry::Tool {
            result,
            diff,
            expanded,
            ..
        }) = self.entries.get_mut(index)
        else {
            return false;
        };
        let showable = result.as_ref().is_some_and(|text| !text.is_empty())
            || diff.as_ref().is_some_and(|text| !text.is_empty());
        if !showable {
            return false;
        }
        let effective = expanded.unwrap_or(global);
        *expanded = Some(!effective);
        self.invalidate_cache();
        true
    }

    /// Register a pre-parse markdown transform (gh #12): pi's
    /// `registerMarkdownTransformer`. Transforms run in registration
    /// order over the raw source before parsing; registering drops the
    /// render cache, so the new transform reaches every entry.
    pub fn register_markdown_transformer(
        &mut self,
        transformer: lca_tui::widgets::markdown::MarkdownTransformer,
    ) {
        self.markdown_transformers.push(transformer);
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

    /// The last assistant message's source text, when it has any: what
    /// `app.message.copy` copies with nothing selected (gh #9). The
    /// stored source, never the rendered rows - plain text by
    /// construction, then stripped of any escape the model wrote.
    pub fn last_assistant_text(&self) -> Option<&str> {
        self.entries.iter().rev().find_map(|entry| match entry {
            Entry::Assistant { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
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

    /// Record a tool call the model asked for.
    pub fn start_tool(&mut self, name: impl Into<String>, args: impl Into<String>) {
        self.entries.push(Entry::Tool {
            name: name.into(),
            args: args.into(),
            status: ToolStatus::Running,
            result: None,
            diff: None,
            manual: false,
            expanded: None,
        });
    }

    /// Record a `!`/`!!` run from the editor (FR-UI-14): the same card,
    /// with pi's `bashMode` title instead of `toolTitle`.
    pub fn start_manual_tool(&mut self, name: impl Into<String>, args: impl Into<String>) {
        self.entries.push(Entry::Tool {
            name: name.into(),
            args: args.into(),
            status: ToolStatus::Running,
            result: None,
            diff: None,
            manual: true,
            expanded: None,
        });
    }

    /// Finish the most recent running tool call.
    pub fn finish_tool(&mut self, status: ToolStatus, result: Option<String>) {
        self.finish_tool_with_diff(status, result, None);
    }

    /// Finish the most recent running tool call, carrying the structured
    /// diff the tool returned in `extras["diff"]` (gh #9, EFG-014): the
    /// card then renders it as pi's diff card. A result with no diff
    /// takes the ordinary path - `finish_tool` is this with `None`.
    pub fn finish_tool_with_diff(
        &mut self,
        status: ToolStatus,
        result: Option<String>,
        diff: Option<String>,
    ) {
        self.invalidate_cache();
        for entry in self.entries.iter_mut().rev() {
            if let Entry::Tool {
                status: s,
                result: r,
                diff: d,
                ..
            } = entry
                && *s == ToolStatus::Running
            {
                *s = status;
                *r = result;
                *d = diff;
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
            diff: None,
            manual: false,
            expanded: None,
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
            if let Some((cached_width, lines, _)) = &cache[i]
                && *cached_width == width
            {
                out.extend(lines.iter().cloned());
                continue;
            }
            let mut lines = Vec::new();
            let thinking_rows = render_entry(
                entry,
                width,
                theme,
                self.tools_expanded,
                self.thinking,
                self.codeblock_border,
                &self.markdown_transformers,
                &self.tuning,
                &mut lines,
            );
            out.extend(lines.iter().cloned());
            cache[i] = Some((width, lines, thinking_rows));
        }
        out
    }

    /// The entry owning a document row, if any, and whether the row
    /// belongs to its reasoning run (gh #11's hit map): blank separator
    /// rows belong to no entry. The render cache carries each entry's
    /// line count, so this walks lengths, never re-renders.
    /// What a transcript document row belongs to (gh #166): a
    /// thinking row cycles its run, a tool card's first row toggles its
    /// expansion, anything else is not clickable.
    pub fn entry_at_row(&self, width: u16, theme: &Theme, row: usize) -> Option<EntryHit> {
        let _ = self.render(width, theme);
        let cache = self.cache.borrow();
        let mut line = 0usize;
        for (i, slot) in cache.iter().enumerate() {
            if i > 0 {
                if line == row {
                    return None;
                }
                line += 1;
            }
            let Some((_, lines, thinking_rows)) = slot.as_ref() else {
                continue;
            };
            if row >= line && row < line + lines.len() {
                if row - line < *thinking_rows {
                    return Some(EntryHit::Thinking(i));
                }
                // A tool card paints as a state band: a blank band row,
                // then the header, then the output (`render_tool` owns
                // this shape, so the header is always row 1).
                if row - line == 1 && matches!(self.entries.get(i), Some(Entry::Tool { .. })) {
                    return Some(EntryHit::ToolHeader(i));
                }
                return None;
            }
            line += lines.len();
        }
        None
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
            let _ = render_entry(
                entry,
                width,
                theme,
                self.tools_expanded,
                self.thinking,
                self.codeblock_border,
                &self.markdown_transformers,
                &self.tuning,
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

mod render;
use render::*;

#[cfg(test)]
mod tests;
