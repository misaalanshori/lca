//! The transcript, ported from pi's `coding-agent/…/components/messages.ts`
//! (`pi-tui-re/src_re/agent-components/messages.md`).
//!
//! Owner issues #5 (user prompts in history), #6 (markdown, message separation) and #4
//! (streaming) live here: a list of entries rendered to styled lines, updated
//! incrementally as a turn streams.

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
        /// Active execution time in milliseconds (gh #214); `None`
        /// for refusals and old replays that never recorded one.
        duration_ms: Option<u64>,
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
        /// When the call started (gh #391 gap 2): the live `Elapsed` timer
        /// reads it while the card runs. `None` for a replayed historical
        /// card, which must show its recorded `Took` and never animate.
        started_at: Option<std::time::Instant>,
        /// The editor's `!!` run (gh #391 gap 3): output withheld from the
        /// model's context. Draws quiet where `!` draws in `bashMode`.
        excluded: bool,
        /// The calls this one nested (gh #404 gap 1). Rendered as a checklist
        /// **inside** this card, never as sibling cards: a tool that greps
        /// twenty times is one card with twenty rows, not twenty-one cards.
        ///
        /// The data arrives on `ToolResult.nested` (gh #77), which the turn
        /// fills where the child finishes - nested calls never become their own
        /// records, so this is the only place they exist.
        children: Vec<ChildCall>,
        /// The parent link the call itself carried, kept so an **orphan** - a
        /// child whose parent card is absent - is still drawn as a top-level
        /// card rather than silently dropped (gh #404's fallback).
        parent_link: Option<String>,
        /// The call's own id, when it has one. It is what lets a child's start
        /// and finish be matched to the card (or to the parent that suppressed
        /// it), because the events arrive in pairs and the transcript cannot
        /// assume the most recent running card is the one being answered.
        call_id: Option<String>,
    },
    /// A compaction summary, collapsed to one self-describing line (gh #393).
    ///
    /// It was an `Entry::Raw` string - the fully expanded summary, every time -
    /// so a long session's transcript was mostly summary the reader had already
    /// watched happen. The card keeps the *fact* visible (strategy, record range,
    /// counts) and hides the prose until asked.
    ///
    /// `expanded` follows the tool card's per-entry override over the global
    /// default (gh #166's lifecycle), so there is one collapse mechanism in the
    /// transcript rather than a second one for this entry.
    Compaction {
        /// What the card knows.
        card: Card,
        /// This card's expansion, when the reader toggled it; `None` follows
        /// [`Transcript::tools_expanded`].
        expanded: Option<bool>,
    },
    /// A transient notice (command output, info).
    Notice(String),
    /// An error.
    Error(String),
    /// A pre-rendered line from a resumed session (displayed verbatim).
    Raw(String),
    /// The startup key-hint line (gh #23), rendered from the **live**
    /// keybindings each frame so a rebind reaches it (gh #410).
    ///
    /// It was a `Raw` string and therefore baked: `/reload` rebound the keys and
    /// the first line a user reads kept naming the old ones. Keeping the *shape*
    /// as an entry rather than a string is the whole fix - the keys are resolved
    /// at render time, like every other hint.
    StartupHint,
    /// The startup header (gh #400 gaps 2 and 3): the compact line, or the
    /// expanded table when the reader presses `ctrl+o`. A live entry so the expand
    /// key reaches it, the collision notices ride it, and it outlives `/new` - see
    /// `startup_header.rs` for why each of those needs an entry rather than `Raw`.
    StartupResources {
        /// The compact one-line header (empty when `quietStartup` hides it).
        compact: Vec<String>,
        /// The long-form table behind the expand.
        expanded: Vec<String>,
        /// Whether the reader has expanded it this session.
        expanded_state: bool,
    },
    /// The startup header's logo block (gh #399): the owner's mark beside the
    /// version, the live key hints, and the onboarding nudge.
    ///
    /// A live entry rather than a pre-rendered `Raw` string for the same reason
    /// [`Entry::StartupHint`] is: the key hints resolve at render time, so a
    /// `/reload` rebind reaches the first thing a user reads, and the **width**
    /// decides whether the mark draws beside the text, stacks above it, or gives
    /// way to the version alone. Baking it would fix both at startup.
    StartupHeader {
        /// Whether the resource line (gh #400's `Entry::StartupResources`) is
        /// printed beneath this block and therefore already names the version.
        ///
        /// Set when the host supplied one. Without it the two headers both print
        /// `v0.6.0 (abi 0.6)` and `lca 0.6.0 (abi 0.6)`, which reads as a
        /// duplicated startup - the mark then keeps only the hints and the nudge,
        /// and the resource line becomes the one row that names the build (it is
        /// also the expandable one, so the version and the expand hint stay
        /// together).
        beside_resources: bool,
    },
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
/// One entry's cached frame: the width it was rendered at, its rows, how many of
/// them are reasoning, and the input epoch they belong to (gh #410 - the key
/// hints inside those rows depend on the live keybindings, which are not a
/// function of the width).
type CachedRender = Option<(u16, u64, Vec<String>, usize)>;

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
    /// A row inside an expanded tool card's output (by entry index,
    /// gh #247): clicks here toggle like header clicks.
    ToolBody(usize),
    /// Any row of a compaction card (by entry index, gh #393): the whole card
    /// is the target, because the card is the thing being opened.
    Compaction(usize),
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
    /// The live keybindings, so a hint names the key the interface will accept
    /// (gh #410). Always present: a transcript with no interface around it (a unit
    /// test, an export) holds a default manager, so there is exactly one code path
    /// and no per-hint fallback to keep in step.
    keybindings: std::sync::Arc<lca_tui::engine::keybindings::KeybindingsManager>,
    /// Bumped whenever a render input that is not the width changes, so a cached
    /// frame cannot outlive a rebind or a settings change.
    ///
    /// The cache key used to be the width alone. That was sound while every input
    /// was either immutable or width-dependent; the live keybindings are neither,
    /// so a `/reload` rebind would have repainted from stale rows and the
    /// acceptance row for gh #410 would have passed only while the cache was
    /// cold. Cheap to keep honest: an integer compare per entry.
    cache_epoch: u64,
    /// Which kitty images this terminal already holds (gh #324 gap 1).
    ///
    /// A repaint of an image already in here emits a **placement**, not a
    /// transmission: the base64 is the expensive part of the frame and the
    /// terminal already has it. Keyed by the derived id, which is stable for the
    /// same payload at the same scale, so the key is exactly "the same upload".
    ///
    /// A `RefCell` because `render` takes `&self` - the cache is a rendering
    /// memo, not state anyone reads.
    uploaded_images: std::cell::RefCell<crate::placements::PlacementCache>,
    /// Nested call ids that were suppressed into a parent's checklist (gh #404
    /// gap 1), mapped to the child's name.
    ///
    /// A nested call emits both a start and a finish, and the finish must settle
    /// the checklist row rather than create a card. The id is the only exact key:
    /// "the most recent running card" is the *parent*, which is still running.
    suppressed: std::collections::BTreeMap<Option<String>, String>,
    /// Per-entry render cache (R15): `None` means the entry must be
    /// rendered; a streaming append invalidates only the last entry, so a
    /// long transcript is not re-rendered from scratch on every delta.
    cache: std::cell::RefCell<Vec<CachedRender>>,
    /// How many times the whole transcript was *replaced* (gh #403).
    ///
    /// Only [`Transcript::clear`] bumps this - appends never do. The run loop
    /// compares it against its own last-seen value to decide whether a
    /// selection still addresses text that exists; an append leaves the
    /// earlier rows exactly where they were, so a selection over them is
    /// still valid and must survive.
    generation: u64,
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
        self.invalidate_cache();
    }

    /// Install the interface's live keybindings, so every hint this transcript
    /// paints names the key the interface will actually accept (gh #410).
    ///
    /// Called once at construction and again on `/reload`, which is the
    /// acceptance case: the next frame shows the rebound key. The cache is
    /// dropped because a hint's text is part of a cached frame.
    pub fn set_keybindings(
        &mut self,
        keybindings: std::sync::Arc<lca_tui::engine::keybindings::KeybindingsManager>,
    ) {
        self.keybindings = keybindings;
        self.invalidate_cache();
    }

    /// The live keybindings a hint renders from.
    pub fn keybindings(&self) -> &std::sync::Arc<lca_tui::engine::keybindings::KeybindingsManager> {
        &self.keybindings
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
    /// Gh #245: a run no longer than the snippet itself (three lines)
    /// renders snippet and full identically, so it skips the redundant
    /// state instead - snippet jumps straight to hidden, and hidden
    /// returns straight to full.
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
        // The snippet shows the first three non-blank lines (render's
        // `SNIPPET_LINES` rule): a run that short has no snippet worth
        // collapsing to, so it cycles hidden <-> full (gh #245).
        let short = reasoning
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
            <= 3;
        *thinking_override = Some(match effective {
            ThinkingVisibility::Snippet if short => ThinkingVisibility::Hidden,
            ThinkingVisibility::Snippet => ThinkingVisibility::Full,
            ThinkingVisibility::Full => ThinkingVisibility::Hidden,
            ThinkingVisibility::Hidden if short => ThinkingVisibility::Full,
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

    /// Drop every cached render (a mutation that is not a streaming append, or a
    /// theme change, so every cached line carries the old palette's bytes).
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
            duration_ms: None,
            // gh #391 gap 2: a live card times itself from here.
            started_at: Some(std::time::Instant::now()),
            excluded: false,
            children: Vec::new(),
            parent_link: None,
            call_id: None,
        });
    }

    /// Record a `!`/`!!` run from the editor (FR-UI-14): the same card,
    /// with pi's `bashMode` title instead of `toolTitle`.
    pub fn start_manual_tool(&mut self, name: impl Into<String>, args: impl Into<String>) {
        self.start_manual_tool_excluding(name, args, false);
    }

    /// Record a `!`/`!!` run, saying whether its output is withheld from the
    /// model (gh #391 gap 3). `!!` draws quiet, `!` in `bashMode`.
    pub fn start_manual_tool_excluding(
        &mut self,
        name: impl Into<String>,
        args: impl Into<String>,
        excluded: bool,
    ) {
        self.entries.push(Entry::Tool {
            name: name.into(),
            args: args.into(),
            status: ToolStatus::Running,
            result: None,
            diff: None,
            manual: true,
            expanded: None,
            duration_ms: None,
            started_at: Some(std::time::Instant::now()),
            excluded,
            children: Vec::new(),
            parent_link: None,
            call_id: None,
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
        self.finish_tool_full(status, result, diff, None);
    }

    /// Finish with the recorded execution time (gh #214): the card
    /// renders it as `Took …`; `None` renders no timing line.
    pub fn finish_tool_full(
        &mut self,
        status: ToolStatus,
        result: Option<String>,
        diff: Option<String>,
        duration_ms: Option<u64>,
    ) {
        self.invalidate_cache();
        for entry in self.entries.iter_mut().rev() {
            if let Entry::Tool {
                status: s,
                result: r,
                diff: d,
                duration_ms: ms,
                ..
            } = entry
                && *s == ToolStatus::Running
            {
                *s = status;
                *r = result;
                *d = diff;
                *ms = duration_ms;
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

    /// Every notice line, oldest first (gh #462: the tab-move refusals are
    /// notices, and a callerasserting one should not have to parse the frame).
    pub fn notices(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Notice(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Append an error.
    pub fn push_error(&mut self, text: impl Into<String>) {
        self.entries.push(Entry::Error(text.into()));
    }

    /// Append a pre-rendered resume line, displayed verbatim.
    pub fn push_raw(&mut self, text: impl Into<String>) {
        self.entries.push(Entry::Raw(text.into()));
    }

    /// Append the startup key-hint line (gh #23). Its keys resolve at render time,
    /// so a `/reload` rebind reaches it (gh #410).
    pub fn push_startup_hint(&mut self) {
        self.entries.push(Entry::StartupHint);
        self.invalidate_cache();
    }

    /// Append the startup header block (gh #399). Like [`Self::push_startup_hint`]
    /// its keys and its width rung resolve at render time, so this records the
    /// *intent* rather than a rendered string.
    ///
    /// `beside_resources` says whether a resource line will print under it and
    /// already name the version; see [`Entry::StartupHeader`].
    pub fn push_startup_header(&mut self, beside_resources: bool) {
        self.entries.push(Entry::StartupHeader { beside_resources });
        self.invalidate_cache();
    }

    /// Drop every entry (R3's session switch): the new session starts its
    /// own transcript.
    ///
    /// Bumps [`Transcript::generation`], which is what tells the run loop a
    /// selection no longer addresses live rows (gh #403 gap 1).
    pub fn clear(&mut self) {
        self.invalidate_cache();
        self.entries.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// How many whole-transcript replacements have happened (gh #403).
    ///
    /// Distinguishes "the user's selection still addresses these rows" from
    /// "these rows are different text now". Appends, streaming deltas, and
    /// resizes leave it alone; only [`Transcript::clear`] moves it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Rewind a card's clock (gh #391 gap 2 tests).
    ///
    /// The timer derives its text from `started_at`, so a row can move the
    /// clock instead of sleeping four seconds - the assertion stays about the
    /// derivation rather than about wall time.
    #[cfg(test)]
    pub(super) fn set_started_at_for_test(&mut self, index: usize, at: std::time::Instant) {
        if let Some(Entry::Tool { started_at, .. }) = self.entries.get_mut(index) {
            *started_at = Some(at);
        }
    }

    /// Push a card the way a **replay** would (gh #391 gap 2): a recorded
    /// duration and no live clock, so it must render its exact `Took` and
    /// never animate.
    #[cfg(test)]
    pub(super) fn push_historical_tool_for_test(
        &mut self,
        name: &str,
        args: &str,
        result: &str,
        duration_ms: u64,
    ) {
        self.entries.push(Entry::Tool {
            name: name.to_string(),
            args: args.to_string(),
            status: ToolStatus::Ok,
            result: Some(result.to_string()),
            diff: None,
            manual: false,
            expanded: None,
            duration_ms: Some(duration_ms),
            started_at: None,
            excluded: false,
            children: Vec::new(),
            parent_link: None,
            call_id: None,
        });
    }

    /// Whether any tool card is still running (gh #391 gap 2).
    ///
    /// The interface's tick uses this to decide whether the `Elapsed` line
    /// needs a repaint: a transcript with no running card must not be
    /// invalidated once a second forever, and a running one must be, or the
    /// timer freezes at whatever the first frame said.
    pub fn has_running_tool(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(
                entry,
                Entry::Tool {
                    status: ToolStatus::Running,
                    ..
                }
            )
        })
    }

    /// Drop the cached render of every running tool card (gh #391 gap 2), so
    /// the next frame re-derives `Elapsed` from its `started_at`. Called from
    /// the interface tick; a settled card keeps its cache and therefore its
    /// frozen `Took`.
    pub fn invalidate_running_tools(&mut self) {
        let mut cache = self.cache.borrow_mut();
        if cache.len() != self.entries.len() {
            cache.resize(self.entries.len(), None);
        }
        for (index, entry) in self.entries.iter().enumerate() {
            if matches!(
                entry,
                Entry::Tool {
                    status: ToolStatus::Running,
                    ..
                }
            ) {
                cache[index] = None;
            }
        }
    }

    /// Render every entry to styled lines at `width`, for the detected terminal.
    pub fn render(&self, width: u16, theme: &Theme) -> Vec<String> {
        self.render_for_protocol(
            width,
            theme,
            lca_tui::widgets::image::detect_image_protocol(),
        )
    }

    /// Render every entry to styled lines at `width` for an explicit image
    /// protocol (gh #324 gap 1).
    ///
    /// The protocol is a parameter rather than a call to the detector because
    /// the kitty path is the one under test on a box whose tmux has no
    /// graphics: a row that could only ask the environment could not assert the
    /// image shapes at all.
    pub fn render_for_protocol(
        &self,
        width: u16,
        theme: &Theme,
        protocol: lca_tui::widgets::image::ImageProtocol,
    ) -> Vec<String> {
        self.render_for_protocol_at(width, theme, protocol, RowStage::Final)
    }

    /// [`Self::render_for_protocol`] stopped at a chosen stage (gh #324 gap 3).
    ///
    ///
    /// The placement cache rewrites a kitty header into a no-payload `a=p` form (gap 1),
    /// and a **cropped** header is itself a placement, so the crop runs before the cache:
    /// afterwards there is no `a=T` block left to parse.
    pub fn render_for_protocol_at(
        &self,
        width: u16,
        theme: &Theme,
        protocol: lca_tui::widgets::image::ImageProtocol,
        stage: RowStage,
    ) -> Vec<String> {
        let mut cache = self.cache.borrow_mut();
        if cache.len() != self.entries.len() {
            cache.resize(self.entries.len(), None);
        }
        let mut out = Vec::new();
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                out.push(String::new());
            }
            if let Some((cached_width, cached_epoch, lines, _)) = &cache[i]
                && *cached_width == width
                && *cached_epoch == self.cache_epoch
            {
                out.extend(lines.iter().cloned());
                continue;
            }
            let mut lines = Vec::new();
            let thinking_rows = render_entry(
                entry,
                width,
                theme,
                protocol,
                self.tools_expanded,
                self.thinking,
                self.codeblock_border,
                &self.markdown_transformers,
                &self.tuning,
                self.keybindings.as_ref(),
                &mut lines,
            );
            out.extend(lines.iter().cloned());
            cache[i] = Some((width, self.cache_epoch, lines, thinking_rows));
        }
        if stage == RowStage::Final && protocol == lca_tui::widgets::image::ImageProtocol::Kitty {
            self.uploaded_images.borrow_mut().substitute(&mut out);
        }
        // Gh #375 gap 3: **the marker is stripped at paint time, and only at the final
        // stage.** It is in-band so a jump can find its row after any reflow, but it is
        // a control sequence: leaving it in the frame empties a cell the text wanted.
        //
        // The final stage specifically, because `BeforePlacements` is what the image crop
        // parses - and the crop reads columns, so a row still carrying a marker would be
        // measured one sequence long there. Stripping here means the marker never
        // reaches a renderer, a copy destination, or a scrollback write, while the cached
        // rows keep it for the next jump scan.
        if stage == RowStage::Final {
            for line in out.iter_mut() {
                if line.contains("\x1b]133;") {
                    *line = crate::prompt_marker::strip_markers(line).into_owned();
                }
            }
        }
        out
    }
}

mod compaction_card;
mod image_blocks;
pub use image_blocks::RowStage;
mod measure;
pub use compaction_card::Card;
pub use measure::image_label;
mod nested;
mod startup_header;
pub use RowStage as TranscriptStage;
pub use nested::{COLLAPSED_CHILDREN, ChildCall, ChildStatus};
mod preview;
mod render;
// Gh #574: the replay path and its appenders.
mod replay_records;
use render::*;
// gh #410: the hint formatter crosses for a sibling module; the rest stays private.
#[cfg(test)]
pub(crate) use render::render_key_hint;

#[cfg(test)]
#[path = "compaction_card_tests.rs"]
mod compaction_card_tests;

#[cfg(test)]
#[path = "image_tests.rs"]
mod image_tests;
#[cfg(test)]
#[path = "nested_tests.rs"]
mod nested_tests;
#[cfg(test)]
#[path = "preview_tests.rs"]
mod preview_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
#[path = "timer_tests.rs"]
mod timer_tests;
