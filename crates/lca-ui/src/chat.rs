//! The interactive chat composition, ported from pi's
//! `coding-agent/src/modes/interactive/chat-viewport.ts`
//! (`pi-tui-re/src_re/agent-components/utilities.md` §1).
//!
//! The product's frame: the transcript above, the editor dock and footer
//! below, overlays composited over the viewport. It owns the engine
//! widgets - `widgets::editor::Editor`, `Transcript`, `Footer`, `Theme` -
//! and routes raw input through the pi pipeline
//! (`parse_key` -> `KeybindingsManager` -> editor actions). There is one
//! editor and one key vocabulary (ADR-0037).

use std::sync::Arc;

use lca_protocol::{Record, TurnEvent, Usage};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::keys;
use lca_tui::widgets::editor::{Editor, EditorEvent};

use crate::chat_commands::{is_turn_boundary_command, provider_for};
use crate::chat_pickers::{
    GrantPicker, ModelPicker, ShellRun, ThemePicker, ThinkingPicker, TreePicker, TrustPicker,
};

use crate::footer::Footer;

use crate::separator::Separator;
use crate::state::{Action, TurnStatusLine, UiOptions, UiState};
// The tests reach `LoginNext` through this module's former import.
#[cfg(test)]
use crate::state::LoginNext;
use crate::theme::Theme;
use crate::transcript::Transcript;

/// One queued message (steering, ADR-0038). Carried by `Chat`; the agent
/// semantics live in `lca-core`. A queued *command* (a slash command
/// typed mid-turn) rides the same queue in order but dispatches as a
/// command at turn end, never as model text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMessage {
    /// The message text (or the command line, when `is_command`).
    pub text: String,
    /// How it was submitted while the turn ran.
    pub mode: lca_protocol::SubmitMode,
    /// A slash command queued mid-turn: it dispatches as a command at
    /// turn end (the composer-polish fold-in), never as model text.
    pub is_command: bool,
}

/// The startup key-hint line (gh #23): pi prints
/// `escape interrupt · ctrl+c/ctrl+d clear/exit · / commands …` at
/// startup; this is our wording on pi's shape. One dim line on the
/// first frame, never a modal. Headless mode never builds a `Chat`, so
/// it never prints this (the scripting contract).
/// The interactive chat.
/// A transient toast (gh #239): a confirmation or host diagnostic
/// that floats top-right and fades after five seconds, never
/// stealing transcript rows or landing in history.
#[derive(Debug, Clone)]
pub struct Toast {
    /// The line to float.
    pub text: String,
    /// When it fades.
    pub until: std::time::Instant,
}

/// How long a toast floats (gh #239, pi's transient-note shape).
pub const TOAST_SECS: u64 = 5;

/// How many toasts stack (older ones drop — a flood of confirmations
/// is one line, not a wall).
pub const TOAST_MAX: usize = 3;

/// The interactive chat.
pub struct Chat {
    /// The transcript.
    pub transcript: Transcript,
    /// The prompt editor.
    pub editor: Editor,
    /// The footer data (cwd, session).
    pub footer: Footer,
    /// The document the last `viewport` composed (gh #374 gap 1): written there,
    /// taken by [`Chat::take_document`]. One `sections` walk serves both the
    /// frame and the fullscreen exit dump; see `take_document` for why it is not
    /// simply recomposed.
    pub(crate) document: std::cell::RefCell<Vec<String>>,
    /// The keybinding registry: app actions are looked up here.
    ///
    /// **Not `pub`, and that is the fix for gh #332.** It used to be `pub`, so an
    /// embedder could assign a new map and silently leave `editor` — which holds its
    /// own clone — on the old one. That is exactly the bug this cycle closed: one fact
    /// with two owners and nothing keeping them equal. Reads stay `pub(crate)` because
    /// a dozen call sites in this crate resolve actions against it; **writes** go
    /// through [`Chat::set_keybindings`], which updates the transcript's hint manager,
    /// the editor and this field together, so replacing one without the others is not
    /// expressible outside this module. [`Chat::keybindings_manager`] is the borrow an
    /// embedder gets.
    pub(crate) keybindings: Arc<KeybindingsManager>,
    /// The theme.
    pub theme: Theme,
    /// Accumulated session usage: the true, single-counted sum of every
    /// completed turn's totals (gh #327). Folded once per turn by
    /// [`Chat::fold_turn_usage`] — never grown by the stream.
    pub usage: Usage,
    /// This turn's usage so far, rebuilt from the stream (gh #327): a
    /// provider may report in several events, so this `absorb`s each one
    /// exactly as core does. It feeds mid-turn display only; the session
    /// total takes it (or core's authoritative `TurnOutcome`) once, at
    /// the turn boundary.
    pub turn_usage: Usage,
    /// The turn's generation-time meter (the footer's tok/s reading). It adds
    /// up the spans `lca-core` stamped where the stream was consumed (gh
    /// #464); it holds no clock of its own.
    ///
    /// `pub(crate)` since gh #462: a background turn's meter travels with its
    /// tab, so the swap in `chat_tabs` moves this field.
    pub(crate) meter: crate::footer::meter::StreamMeter,
    /// Whether a turn is running.
    pub turn_running: bool,
    /// Which tab the running turn belongs to (gh #462). A turn is started by
    /// the visible tab, so this is set in `begin_turn`; it is what routes the
    /// stream into the right transcript once the reader has switched away.
    pub(crate) turn_tab: Option<usize>,
    /// A session id the host must be switched to once the running turn settles
    /// (gh #462).
    ///
    /// The host's `switch_session` rewrites a shared "current session" cell
    /// that the turn worker reads on its own thread, lazily. Two tabs cannot
    /// both be that cell's answer, so a switch during a turn is deferred: the
    /// *display* moves now (each tab carries its own transcript), and the
    /// host's binding follows at settle. Moving it early would append the
    /// running turn's records to a different session's log.
    pub(crate) deferred_host_session: Option<String>,
    /// Whether a background `/compact` is summarizing (the interface stays
    /// responsive while it does; the separator says `Working`).
    pub compacting: bool,
    /// Gh #496: whether the *current* compaction already left its transcript
    /// marker, so a duplicate `CompactionEnded` (a retried attempt) does not
    /// append a second row for one compaction. Cleared by `CompactionStarted`,
    /// which is what makes a genuine second compaction still get its own.
    pub(crate) compaction_marked: bool,
    /// The separator row's state and spinner (chrome.md's
    /// spinner-in-the-border, R2).
    pub separator: Separator,
    /// The status-line turn state.
    pub turn_status: Option<TurnStatusLine>,
    /// The ui-world adapter (options + modals).
    pub world: UiState,
    /// Messages queued while a turn runs (ADR-0038).
    pub pending: Vec<PendingMessage>,
    /// Messages parked while a compaction runs (gh #401, pi's
    /// `compactionQueuedMessages`): a compaction is a model round-trip that
    /// owns the session, so a submit during it cannot start a turn - but
    /// refusing the text loses what the user typed. They flush through
    /// [`Self::flush_compaction_queue`] when it ends.
    ///
    /// Kept apart from [`Self::pending`] so the dock can say *why* a
    /// message is waiting, but every clear/restore path treats the two as
    /// one queue (pi's `getAllQueuedMessages`).
    pub compaction_queued: Vec<PendingMessage>,
    /// How queued follow-ups flush at turn end (gh #76): `OneAtATime`
    /// pops one per turn end (historical); `All` flushes every pending
    /// follow-up as one joined submission. Set from `queue.follow_up_mode`.
    pub follow_up_mode: lca_protocol::QueueMode,
    /// The shared queue the running turn drains at each boundary.
    pub current_steer: Option<lca_protocol::SteerQueue>,
    /// A submitted prompt awaiting the loop's handoff.
    pub submitted: Option<String>,
    /// How that prompt was queued (ADR-0038): `None` for an ordinary
    /// submit, `steer`/`follow-up` when it was queued while a turn ran and
    /// is now flushing as this turn - the record carries the marker
    /// (`docs/session-log-format.md`).
    pub(crate) submitted_queue: Option<lca_protocol::SubmitMode>,
    /// Whether the fullscreen (alt-screen) renderer is active (FR-UI-21).
    pub screen_mode: bool,
    /// Ctrl+X seen, waiting for the chord's second key (external editor).
    pending_ctrl_x: bool,
    /// A pending prompt-jump target (a document line index).
    pub(crate) jump_target: Option<usize>,
    /// When the last lone Escape landed (gh #132): a second one within
    /// pi's 500 ms window acts (tree/fork/none), anything else re-arms.
    pub(crate) last_escape: Option<std::time::Instant>,
    /// Whether the pointer hovers the scrollbar (gh #164): the frame
    /// paints the thumb solid while set. The loop mirrors it into the
    /// renderer and clears it whenever an overlay covers the viewport
    /// (pi's `stopScrollbarHover`).
    pub scrollbar_hover: bool,
    /// An active scrollbar drag's grab offset in rows from the thumb top
    /// (gh #164, pi's `grabOffset`): motion maps pointer Y to scroll
    /// while set, and text selection stays off.
    pub scroll_drag: Option<u16>,
    /// The floating toast stack (gh #239): transient confirmations and
    /// host diagnostics, newest last, fading on `tick`.
    pub toasts: Vec<Toast>,
    /// An in-progress picker press (gh #167, pi's `mousePressedIndex`):
    /// a release on the same item confirms it, anywhere else just moves
    /// the highlight. `None` means no press owns the gesture.
    pub(crate) picker_press: Option<usize>,
    /// The painted picker window (gh #226): the `(start, end)` item
    /// range plus the content height the last frame's rolling viewport
    /// showed. The mouse hit test reads it back, so clicks land on what
    /// is actually drawn — never a recomputed guess that wrapping could
    /// desync. `(0, usize::MAX, 0)` before the first paint (the mouse
    /// falls back to the full list then).
    pub(crate) picker_window: std::cell::Cell<(usize, usize, u16)>,
    /// Whether the pointer hovers the drawer tab (gh #207): the frame
    /// paints it in accent while set.
    pub drawer_hover: bool,
    /// A shown tooltip (gh #210): placed lines the frame paints last.
    pub tooltip: Option<crate::chat_mouse::Tooltip>,
    /// The last hover cell and when it landed (gh #210): the 250 ms
    /// debounce clock. Tests set it directly to fake time.
    pub(crate) hover_at: Option<(u16, u16, std::time::Instant)>,
    /// The transcript's line count at the last frame (gh #35): scroll is
    /// measured from the live bottom, so growth is what tells a new line
    /// from a re-wrap when holding the reader's place.
    pub(super) last_transcript_len: Option<usize>,
    /// The width that count was measured at (gh #35).
    pub(super) last_render_width: u16,
    /// The screen scroll at the last frame (gh #209): the run loop
    /// mirrors it here so a tab switch can save the reader's place.
    pub(crate) last_scroll: u16,
    /// The session tabs (gh #209): one per open conversation.
    pub(crate) tabs: Vec<crate::chat_tabs::SessionTab>,
    /// The active tab's index.
    pub(crate) active_tab: usize,
    /// The previously active tab (a close lands here).
    pub(crate) prev_tab: usize,
    /// The open transcript search query (FR-UI-12), when any.
    pub search: Option<String>,
    /// Document lines matching the search query.
    pub(crate) search_matches: Vec<usize>,
    /// The current match.
    pub(crate) search_index: usize,
    /// The active theme's name (FR-UI-17).
    pub theme_name: String,
    /// The configured theme setting (`ui.theme`, S5).
    theme_setting: String,
    /// Where custom theme files resolve, in order (gh #70).
    theme_extra_dirs: Vec<std::path::PathBuf>,
    /// The picker's theme names (built-ins plus custom files).
    pub theme_names: Vec<String>,
    /// Whether the theme still follows the detected terminal scheme (R10);
    /// an explicit pick clears it. `auto` and `system` both follow (gh
    /// #10); anything else is fixed.
    theme_auto: bool,
    /// The terminal's reported background color, when it has answered
    /// (gh #10): the `system` theme rebuilds from this on every scheme
    /// or background answer.
    terminal_bg: Option<lca_tui::engine::colors::RgbColor>,
    /// The startup colour query's lifecycle (gh #379 gap 1).
    theme_query: crate::theme_query::ThemeQuery,
    /// The open `/theme` picker with live preview, when any.
    pub theme_picker: Option<ThemePicker>,
    /// The `/settings` selector (gh #30): open while it owns the
    /// keyboard; a row's sub-picker replaces it (one picker at a time).
    pub settings_picker: Option<crate::chat_pickers::SettingsPicker>,
    /// The open `/thinking` level picker, when any (R1).
    pub thinking_picker: Option<ThinkingPicker>,
    /// The open `/model` picker, when any (R9).
    pub model_picker: Option<ModelPicker>,
    /// The open `/extensions` hub, when any (#227).
    pub extension_hub: Option<crate::chat_hub::ExtensionHub>,
    /// The open `/ext` manager, when it owns the keyboard (gh #3).
    pub extension_manager: Option<crate::chat_extman::ExtensionManager>,
    /// The open `/mcp` manager, when it owns the keyboard (gh #418 Phase G).
    pub mcp_manager: Option<crate::chat_mcp::McpManager>,
    /// A pending install-consent step (#227): the hub stays behind it.
    pub install_consent: Option<crate::chat_hub::InstallConsent>,
    /// The open `/grants` view, when any (S8).
    pub grants_picker: Option<GrantPicker>,
    /// The open `/tree` branch selector, when any (FR-UI-16).
    pub tree_picker: Option<TreePicker>,
    /// The open `/trust` picker, when any (ADR-0039).
    pub trust_picker: Option<TrustPicker>,
    /// The running `!`/`!!` command, when any (R4).
    pub shell: Option<ShellRun>,
    /// The open `/resume` session picker (R2).
    pub resume_picker: Option<crate::resume::ResumePicker>,
    /// The `/fork` user-message picker while open (gh #203).
    pub fork_picker: Option<crate::chat_pickers::ForkPicker>,
    /// The `/scoped-models` checklist while open (gh #204).
    pub scoped_models_picker: Option<crate::chat_pickers::ScopedModelsPicker>,
    /// The cached model catalog snapshot (gh #233): enumeration is
    /// per-provider WASM work, so dispatch reads this instead of the
    /// hook. `None` means unlisted; login, switch, grant, reload, and
    /// scope events clear it (see `invalidate_models`).
    pub(crate) model_rows_cache: Option<Vec<crate::state::ModelRow>>,
    /// A background catalog discovery's rows (gh #232): the picker
    /// opened instantly and this fills it when the thread lands.
    pub(crate) model_refresh: Option<std::sync::mpsc::Receiver<Vec<crate::state::ModelRow>>>,
    /// Whether this terminal forwards `OSC 8` (gh #398 gap 1): the sign-in
    /// modal only offers a clickable label where a click can actually open
    /// the link. Defaults to the live detection; a test overrides it, because
    /// `lca-tui` caches the answer per process and a rendered frame must be
    /// reproducible.
    pub(crate) hyperlinks: bool,
    /// The startup header's two prepared states (gh #400 gaps 2 and 3), kept on the
    /// chat so the session-switch path can re-push the entry with its expand state
    /// intact - the header must outlive `/new`.
    pub(crate) startup_header: (Vec<String>, Vec<String>),
}
impl Chat {
    /// Build the chat: widgets, theme, and the autocomplete chain.
    pub fn new(options: UiOptions, keybindings: Arc<KeybindingsManager>) -> Chat {
        let mut world = UiState::new(options);
        // gh #76: the follow-up flush mode rides the chat (the options
        // move into the world below, so it is read out first).
        let world_follow_up_mode = world.options.follow_up_mode;
        // The resumed session's records replay with the live rendering and
        // the trailing notices come after them, so both are taken out of the
        // options before `world` moves into the chat (FR-UI-7).
        let initial_records = std::mem::take(&mut world.options.initial_records);
        let initial_tail = std::mem::take(&mut world.options.initial_tail_lines);
        // Gh #400 gaps 2 and 3: taken before `world` moves into the chat, and kept
        // on the chat so a session switch can re-push the header entry.
        let startup_header = std::mem::take(&mut world.options.initial_header);
        let attachment_loader = world.options.hooks.load_attachment.clone();
        let thinking_visibility = world.options.thinking_visibility;
        let codeblock_border = world.options.codeblock_border;
        // S5: the configured theme resolves through the auto-pair grammar
        // (a built-in, a custom file, or `auto` following the detected
        // scheme); an invalid custom file keeps the last-good palette and
        // lands in the notice.
        let theme_setting = if world.options.plain {
            "plain".to_string()
        } else {
            world.options.theme.clone()
        };
        let theme_extra_dirs = world.options.theme_extra_dirs.clone();
        let theme_names = if world.options.themes.is_empty() {
            crate::theme::THEMES
                .iter()
                .map(|name| name.to_string())
                .collect()
        } else {
            world.options.themes.clone()
        };
        let (theme, theme_notice) = crate::theme::load_all(
            &theme_setting,
            crate::theme::detect_scheme(),
            &theme_extra_dirs,
        );
        let theme_name = theme.name.clone();
        let theme_auto =
            !world.options.plain && matches!(theme_setting.as_str(), "" | "auto" | "system");
        let chat_head = theme_notice;
        let mut editor = Editor::new();
        editor.set_keybindings(keybindings.clone());
        editor.set_autocomplete(Arc::new(provider_for(&world.options)));
        // V2: the caret is painted into the row on colored themes; the
        // plain theme renders no escapes at all (FR-UI-5), so there the
        // hardware cursor keeps the job it already had. Gh #371 bug 3:
        // `Chat::paints_caret` is the one answer, and every overlay field
        // paints from it too, so no field can show a caret form the composer
        // would not.
        editor.set_paint_caret(theme.colored);
        let mut transcript = Transcript::new();
        // gh #23: one key-hint line on the first frame (pi's startup shape, our
        // wording). Gh #410: it renders from the **live** keybindings each frame,
        // so a `/reload` rebind reaches the first line a user reads - it used to
        // be a pre-rendered `Raw` string and therefore baked the keys at startup.
        transcript.set_keybindings(keybindings.clone());
        // Gh #399: the owner's logo block replaces the bare hint line.
        //
        // One entry for all three `quietStartup` rungs: the **renderer** decides how
        // much to draw, so `Hidden` prints nothing, `VersionOnly` keeps the version
        // row and `Full` draws the owner's block - and a `/settings` cycle moves the
        // rung without a restart. Pushing a different entry per rung would bake the
        // choice at startup, which is the mistake #410 already taught this file not
        // to make with keys.
        // Gh #400 gaps 2 and 3 (worker 2) own the resource line and the expandable
        // table behind `ctrl+o`. When the host supplied one it is the row that names
        // the build, so the logo block drops its own version row rather than printing
        // a second one.
        let has_resources = !startup_header.0.is_empty() || !startup_header.1.is_empty();
        transcript.push_startup_header(has_resources);
        if has_resources {
            transcript.push_startup_resources(startup_header.0.clone(), startup_header.1.clone());
        }
        for line in &world.options.initial_lines {
            transcript.push_raw(line.clone());
        }
        let mut footer = Footer {
            cwd: world.options.workspace.to_string_lossy().to_string(),
            yolo: world.options.yolo,
            context_window: *world
                .options
                .context_window
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ..Default::default()
        };
        // Gh #389 gap 2: one read here, so the first frame names the branch.
        // Every later read is the tick's, never a frame's.
        footer
            .git
            .prime(&footer.cwd.clone(), std::time::Instant::now(), None);
        let screen_mode = world.options.fullscreen;
        // Pi's folder-trust prompt: open the picker at startup only when the
        // project has something to gate and no decision yet (ADR-0039).
        let trust_prompt = world
            .options
            .hooks
            .trust_needed
            .as_ref()
            .is_some_and(|needed| needed());
        // gh #12: the host's collected pre-parse transforms, cloned out
        // before `world` moves into the chat.
        let markdown_transformers = world.options.hooks.markdown_transformers.clone();
        let mut chat = Chat {
            keybindings,
            transcript,
            editor,
            footer,
            theme,
            usage: Usage::default(),
            turn_usage: Usage::default(),
            turn_running: false,
            turn_tab: None,
            deferred_host_session: None,
            compacting: false,
            compaction_marked: false,
            turn_status: None,
            separator: Separator::new(),
            world,
            pending: Vec::new(),
            compaction_queued: Vec::new(),
            follow_up_mode: world_follow_up_mode,
            current_steer: None,
            submitted: None,
            submitted_queue: None,
            screen_mode,
            pending_ctrl_x: false,
            jump_target: None,
            last_escape: None,
            scrollbar_hover: false,
            scroll_drag: None,
            toasts: Vec::new(),
            picker_press: None,
            picker_window: std::cell::Cell::new((0, usize::MAX, 0)),
            drawer_hover: false,
            tooltip: None,
            hover_at: None,
            last_transcript_len: None,
            last_render_width: 0,
            last_scroll: 0,
            tabs: Vec::new(),
            active_tab: 0,
            prev_tab: 0,
            search: None,
            search_matches: Vec::new(),
            search_index: 0,
            meter: Default::default(),
            theme_name,
            theme_setting,
            theme_extra_dirs,
            theme_names,
            theme_auto,
            terminal_bg: None,
            theme_query: crate::theme_query::ThemeQuery::new(),
            theme_picker: None,
            settings_picker: None,
            thinking_picker: None,
            model_picker: None,
            extension_hub: None,
            extension_manager: None,
            mcp_manager: None,
            install_consent: None,
            grants_picker: None,
            tree_picker: None,
            trust_picker: trust_prompt.then_some(TrustPicker { selected: 0 }),
            shell: None,
            resume_picker: None,
            fork_picker: None,
            scoped_models_picker: None,
            model_rows_cache: None,
            model_refresh: None,
            hyperlinks: lca_tui::engine::terminal::supports_hyperlinks(),
            startup_header,
            document: std::cell::RefCell::new(Vec::new()),
        };
        chat.transcript.set_thinking_visibility(thinking_visibility);
        // gh #12: the host's collected pre-parse transforms ride the
        // transcript's pipeline from the first render.
        for transformer in markdown_transformers {
            chat.transcript.register_markdown_transformer(transformer);
        }
        // gh #32: the code-block frame is config, set once like the
        // visibility - the render cache keys on it only through this.
        chat.transcript.set_codeblock_border(codeblock_border);
        // FR-UI-7: a resumed transcript renders like the live one - user
        // band, markdown, tool cards - instead of `user:`/`assistant:` lines.
        chat.load_records(&initial_records, attachment_loader.as_ref());
        for line in &initial_tail {
            chat.transcript.push_raw(line.clone());
        }
        // gh #209: the first tab seeds here, so every later tab op
        // finds it.
        chat.seed_tabs();
        // S5: an invalid custom theme keeps the last-good palette and
        // Gh #399: the transcript needs the tuning **before** the first frame, not
        // on the first `tick()`. The startup header entry is pushed above and its
        // version and rung come from this, so a caller that renders a fresh `Chat`
        // without ticking first (a test, a screenshot, a resumed session) would
        // otherwise draw the defaults - which is exactly what caught this.
        chat.sync_display_tuning();
        // says so in history, never in editor chrome.
        if let Some(notice) = chat_head {
            chat.say(notice);
        }
        chat
    }

    /// Replay persisted records into the transcript with the rendering the
    /// live path used, then fold their usage into the footer totals so a
    /// resumed session reports its own history (FR-UI-7, FR-UI-20).
    pub fn load_records(
        &mut self,
        records: &[Record],
        attachment: Option<&crate::state::LoadAttachment>,
    ) {
        self.transcript.replay_records(records, attachment);
        for record in records {
            if let Record::Assistant {
                usage: Some(usage), ..
            } = record
            {
                // gh #327: a replayed record's usage is a COMPLETED turn's
                // total, so it folds through the same single-counted path
                // `reap` uses rather than through the stream handler (which
                // now only feeds display). Historical spend still shows on a
                // resumed session — it just cannot be double-counted by
                // construction, because there is one fold.
                self.fold_turn_usage(usage);
            }
        }
    }

    /// Start a turn's usage view (gh #327): called at the turn boundary so
    /// a later turn's stream never inherits a previous one's numbers.
    pub fn begin_turn_usage(&mut self) {
        self.turn_usage = Usage::default();
    }

    /// Note one *tool step's* complete usage report inside a turn (gh
    /// #327): distinct calls sum, which is what core's per-turn
    /// `accumulate_usage` does, so the live turn view matches the total
    /// `reap` later folds.
    ///
    /// This is deliberately distinct from [`Usage::absorb`], which the
    /// stream handler uses: an `absorb` is a *refinement* within one call
    /// (gh #276 — a later non-zero value wins), while a step is a new call
    /// whose numbers add. Conflating them would either double-count a
    /// refinement or lose a step's spend.
    pub fn note_turn_step(&mut self, usage: &Usage) {
        self.turn_usage.input = self.turn_usage.input.saturating_add(usage.input);
        self.turn_usage.output = self.turn_usage.output.saturating_add(usage.output);
        self.turn_usage.cache_read = self.turn_usage.cache_read.saturating_add(usage.cache_read);
        self.turn_usage.cache_write = self
            .turn_usage
            .cache_write
            .saturating_add(usage.cache_write);
        self.turn_usage.cache_write_1h = self
            .turn_usage
            .cache_write_1h
            .saturating_add(usage.cache_write_1h);
        self.turn_usage.cost += usage.cost;
    }

    /// Fold one completed turn's total into the session totals (gh #327).
    ///
    /// This is the ONE implementation of "turn total into session total":
    /// `TurnState::reap` calls it with core's authoritative
    /// [`lca_protocol::TurnOutcome::usage`], and the record-replay path
    /// calls it per historical record. Both add exactly once, so the
    /// footer, `/session` and `/usage` report the true sum.
    pub fn fold_turn_usage(&mut self, usage: &Usage) {
        self.usage.input = self.usage.input.saturating_add(usage.input);
        self.usage.output = self.usage.output.saturating_add(usage.output);
        self.usage.cache_read = self.usage.cache_read.saturating_add(usage.cache_read);
        self.usage.cache_write = self.usage.cache_write.saturating_add(usage.cache_write);
        self.usage.cache_write_1h = self
            .usage
            .cache_write_1h
            .saturating_add(usage.cache_write_1h);
        self.usage.cost += usage.cost;
        for (key, value) in &usage.extras {
            self.usage.extras.insert(key.clone(), value.clone());
        }
    }

    /// Consume a turn event into the transcript and the counters.
    ///
    /// Gh #462: the event belongs to a *tab*, not to the interface. When the
    /// reader has switched away from the tab that owns the running turn, the
    /// event is applied to that tab's own state (`with_tab_state`) - so the
    /// stream keeps landing where it started, and the visible tab stays
    /// untouched. On the owning tab this is a plain call with no swap.
    pub fn on_turn_event(&mut self, event: TurnEvent) {
        let ends_turn = matches!(event, TurnEvent::TurnEnded { .. });
        let owner = self.turn_tab.unwrap_or(self.active_tab);
        // The tab flags are interface-wide facts about a tab, so they are set
        // outside the swap (they live on the tab itself, not on `Chat`).
        if ends_turn && let Some(tab) = self.tabs.get_mut(owner) {
            tab.running = false;
            // Unread only when the settle happened off-screen: a turn the
            // reader is watching needs no reminder.
            tab.unread = owner != self.active_tab;
        }
        self.with_tab_state(owner, |chat| chat.apply_turn_event(event, owner));
    }

    /// Mark a turn as started, with the queue it drains at each boundary.
    pub fn begin_turn(&mut self, steer: lca_protocol::SteerQueue) {
        self.turn_running = true;
        // Gh #462: the turn belongs to the tab that submitted it, for the rest
        // of its life. This single line is what every later event is routed
        // by, and what `Ctrl+C` consults.
        self.turn_tab = Some(self.active_tab);
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.running = true;
            tab.unread = false;
        }
        self.separator.working();
        self.current_steer = Some(steer);
        self.turn_status = Some(TurnStatusLine {
            text: "running...".into(),
        });
        self.transcript.begin_assistant();
    }

    /// Take the submitted prompt the loop should run, if any.
    pub fn take_submitted(&mut self) -> Option<String> {
        self.submitted.take()
    }

    /// Take the queue marker the submitted prompt was handed in, after
    /// [`Self::take_submitted`] (ADR-0038's record marker).
    pub(crate) fn take_submitted_queue(&mut self) -> Option<lca_protocol::SubmitMode> {
        self.submitted_queue.take()
    }

    /// The current model label, resolved from the shared cell.
    pub fn model_label(&self) -> String {
        self.world
            .options
            .model_label
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The session's thinking level, resolved from the shared cell (R1).
    pub fn thinking_level(&self) -> Option<String> {
        self.world
            .options
            .thinking
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Say it in history (gh #239, pi's `showStatus`): an
    /// informational line lands as a transcript notice — visible,
    /// scrollable, and permanent — never in editor chrome.
    pub fn say(&mut self, text: impl Into<String>) {
        self.transcript.push_notice(text);
    }

    /// The latest transcript notice (gh #239: command results land
    /// in history, so readers — including tests — look them up here
    /// instead of in editor chrome, which no longer carries text).
    pub fn last_notice(&self) -> Option<String> {
        self.transcript.entries().iter().rev().find_map(|entry| {
            if let crate::transcript::Entry::Notice(text) = entry {
                Some(text.clone())
            } else {
                None
            }
        })
    }

    /// Advance the separator's spinner. Returns `true` when the frame
    /// moved, so the loop repaints; idle never wakes the renderer.
    pub fn tick(&mut self) -> bool {
        self.sync_display_tuning();
        let separator = self.separator.tick();
        // gh #232: the loading spinner rides the same repaint.
        let loading = match self.model_picker.as_mut() {
            Some(picker) => picker.tick(),
            None => false,
        };
        // Gh #391 gap 2: a running tool card re-derives its `Elapsed` line on
        // this tick. The cache is only dropped while a card is actually
        // running, so an idle transcript never repaints for the timer.
        let ticking = self.transcript.has_running_tool();
        if ticking {
            self.transcript.invalidate_running_tools();
        }
        // Gh #389 gap 2: the git branch is re-read on this tick's cadence, not
        // in `render`. The footer repaints on every streaming delta, so a
        // filesystem read there scaled with `.git` latency on the UI thread;
        // here it costs one read per 2 s at worst, and the value is only
        // dropped from the footer when it actually changed.
        let branch =
            self.footer
                .git
                .refresh_if_stale(&self.footer.cwd, std::time::Instant::now(), None);
        separator || loading || ticking || branch || self.expire_toasts()
    }

    /// Handle one keypress: the modal first, then the pickers, the
    /// bindings, and the editor.
    pub fn handle_key(&mut self, data: &str) -> Action {
        // Gh #210: any key dismisses a showing tooltip with zero
        // latency (and rearms the hover clock).
        self.dismiss_tooltip();
        if let Some(action) = self.handle_modal_key(data) {
            return action;
        }

        let key = keys::parse_key(data);

        // gh #30: a running turn's Ctrl+C reaches the agent even while a
        // picker owns the keyboard - the owner's own words in the row
        // that guards this ("not even ctrl-c is able to close it"). The
        // modal dispatch above keeps its own keys: this is pickers only.
        //
        // Gh #462: only a turn the reader can see is cancellable here. A
        // background tab's turn is not - silently killing work nobody is
        // looking at is how a `cargo test` gets thrown away. Switch to that
        // tab and cancel there.
        if self.cancellable_turn_visible() && self.keybindings.matches(data, "app.clear") {
            return Action::CancelTurn;
        }

        if let Some(action) = self.handle_picker_key(data, key.as_deref()) {
            return action;
        }

        // Escape cancels a running `!`/`!!` command (R4), after the pickers
        // (a picker's Escape closes it instead).
        if key.as_deref() == Some("escape")
            && let Some(shell) = self.shell.as_ref()
        {
            (shell.cancel)();
            self.say(format!("cancelling `{}`", shell.command));
            return Action::Continue;
        }

        // The transcript search owns the keyboard while open (FR-UI-12).
        if self.search.is_some() {
            return self.handle_search_key(data, key.as_deref());
        }
        if self.keybindings.matches(data, "app.tools.expand") {
            // Gh #400 gap 2: the same key expands the startup header. One key for
            // "show me the detail" - the header is the detail a reader reaches for
            // before any card exists, and the brief is explicit that it must not
            // get a second binding.
            //
            // Both toggles run: a session with tool cards *and* a header should
            // expand both on one press, which is what a single "expand" key means.
            let header = self.transcript.toggle_startup_resources();
            self.transcript.toggle_tools_expanded();
            let _ = header;
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.thinking.toggle") {
            self.transcript.toggle_thinking_expanded();
            return Action::Continue;
        }
        // gh #209: session tabs - new, close (empty composer only,
        // so Ctrl+W keeps deleting words with text), cycle, and
        // Alt+digit direct jumps. Pickers own the keyboard while open
        // (above), the editor owns the rest below.
        if self.handle_tab_key(data, key.as_deref()) {
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.search") {
            self.search = Some(String::new());
            self.search_matches.clear();
            self.search_index = 0;
            // The readout renders from state in the dock (gh #239).
            return Action::Continue;
        }
        // gh #8 (EFG-003): pi's model cycle - Ctrl+P forward, Ctrl+Shift+P
        // (Alt+P where a terminal drops the shift) backward. The host does
        // the switch, so a cycle lands everywhere a `/model` pick does.
        if self.keybindings.matches(data, "app.model.cycleForward")
            || self.keybindings.matches(data, "app.model.cycleBackward")
        {
            let forward = self.keybindings.matches(data, "app.model.cycleForward");
            self.say(crate::state::sanitize_block(
                &match &self.world.options.hooks.cycle_model {
                    Some(cycle) => cycle(forward),
                    None => "model cycling is not available in this host".to_string(),
                },
            ));
            return Action::Continue;
        }
        // Ctrl+X Ctrl+E opens the external editor (FR-UI-15).
        if self.pending_ctrl_x {
            self.pending_ctrl_x = false;
            if key.as_deref() == Some("ctrl+e") {
                return Action::ExternalEditor;
            }
        }
        if self.keybindings.matches(data, "app.editor.external") {
            self.pending_ctrl_x = true;
            return Action::Continue;
        }

        // Gh #401 item 3: with a queue waiting, the dequeue key takes the
        // queue back into the editor before anything else claims it -
        // above the *editor*, because `Alt+Up` is also a cursor move there
        // and the editor would swallow it. With no queue the key falls
        // through to its other meanings (jump between prompts). The same
        // restore path as the abort rescue and the dock's hint.
        if (!self.pending.is_empty() || !self.compaction_queued.is_empty())
            && self.keybindings.matches(data, "app.message.dequeue")
        {
            self.restore_pending();
            return Action::Continue;
        }

        // The extension panel toggle is a host binding, not an extension's.
        if self.keybindings.matches(data, "app.panel.toggle") {
            self.world.panel_open = !self.world.panel_open;
            return Action::Continue;
        }

        match self.editor.handle_key(data) {
            EditorEvent::Submitted(text) => self.on_submit(text),
            EditorEvent::Exit => Action::Exit,
            EditorEvent::Changed | EditorEvent::None => self.global_key(data),
        }
    }

    /// Revoke one grant (S8): the store's own write path for an ad hoc
    /// grant, or the manual path the install-consent group names.
    pub(super) fn revoke_grant(&mut self, entry: &crate::state::GrantEntry) -> String {
        // gh #233: a revoked grant (net patterns gate catalog fetches)
        // can shrink what providers offer.
        self.invalidate_models();
        if !entry.revocable {
            return format!(
                "`{}` is {subject}: the extension's approved set - revoke it with `lca ext disable {subject}` (or re-run /login)",
                entry.detail,
                subject = entry.subject
            );
        }
        match self.world.options.hooks.revoke_grant.as_ref() {
            Some(revoke) => revoke(entry),
            None => "revoking grants is not available in this host".to_string(),
        }
    }

    /// The `ui.theme` setting currently in force.
    ///
    /// Exposed for `/reload` (gh #387 gap 5), which must re-apply whatever is
    /// active rather than the picker default.
    pub(crate) fn theme_setting(&self) -> &str {
        &self.theme_setting
    }

    /// Whether the theme currently follows terminal detection.
    ///
    /// A `system` theme is rebuilt from the OSC 11 answer, so re-applying it by
    /// name on reload would drop a background the terminal already reported.
    pub(crate) fn theme_follows_detection(&self) -> bool {
        self.theme_auto
    }

    /// Install a new keybinding manager and keep every hint in step (gh #410).
    ///
    /// Assigning `self.keybindings` alone rebinds what the interface *accepts*
    /// while leaving the labels on the manager they were rendered with, which is
    /// the same lie in the opposite direction. The transcript is told too, and it
    /// drops its cached frames because a hint's text is part of one.
    pub fn set_keybindings(&mut self, keybindings: Arc<KeybindingsManager>) {
        self.transcript.set_keybindings(keybindings.clone());
        self.editor.set_keybindings(keybindings.clone());
        self.keybindings = keybindings;
    }

    /// The key map the interface is accepting keys with.
    ///
    /// The reader half of [`Self::set_keybindings`]: what `/hotkeys` prints and what
    /// the editor does both come from this one object, which is the promise gh #332
    /// broke when the field was public and could be replaced alone.
    pub fn keybindings_manager(&self) -> &Arc<KeybindingsManager> {
        &self.keybindings
    }

    /// Apply a theme by name and remember it (S5: built-in or custom file).
    /// Picking `system` re-arms detection-following (gh #10); any other
    /// explicit pick clears it.
    pub(super) fn set_theme(&mut self, name: &str) {
        let (theme, notice) =
            crate::theme::load_all(name, crate::theme::detect_scheme(), &self.theme_extra_dirs);
        self.theme = theme;
        // The transcript caches styled lines per entry: a new palette
        // invalidates every one of them, or the bands keep the old colors.
        self.transcript.invalidate();
        self.theme_name = name.to_string();
        // gh #10: a `system` pick follows detection from here on, so a
        // later OSC 11 / DSR answer rebuilds it; the setting travels
        // with the name for the same reason.
        self.theme_setting = name.to_string();
        self.theme_auto = name == "system";
        if let Some(notice) = notice {
            self.say(notice);
        }
    }

    /// Apply a terminal color-scheme detection (R10): only while the theme
    /// still follows detection, so an explicit pick wins. A `system`
    /// theme rebuilds from the last reported background (gh #10), so a
    /// scheme-only answer never drops a known background.
    pub fn apply_detected_scheme(&mut self, scheme: lca_tui::engine::colors::ColorScheme) {
        if !self.theme_auto || !self.theme_query.accepts_reply() {
            return;
        }
        let (theme, _) = if self.theme_setting == "system" {
            (
                crate::theme::Theme::system_theme(self.terminal_bg, Some(scheme)),
                None,
            )
        } else {
            crate::theme::load_all(&self.theme_setting, Some(scheme), &self.theme_extra_dirs)
        };
        self.theme_name = theme.name.clone();
        self.theme = theme;
        self.transcript.invalidate();
    }

    /// Whether the startup colour query is still accepting replies (gh #379 gap 1).
    pub fn theme_query_open(&self) -> bool {
        self.theme_query.is_open()
    }

    /// Close the startup colour query on the `DA1` sentinel (gh #379 gap 1).
    pub fn close_theme_query(&mut self) {
        self.theme_query.close();
    }

    /// Apply a terminal background answer (gh #10): the OSC 11 reply's
    /// own color. A `system` theme rebuilds from it and repaints;
    /// anything else takes the reply's scheme through the existing
    /// detection path.
    pub fn apply_terminal_background(&mut self, background: lca_tui::engine::colors::RgbColor) {
        if !self.theme_query.accepts_reply() {
            return;
        }
        self.terminal_bg = Some(background);
        // Gh #387: `theme_auto` covers `""`/`auto`/`system`; testing the literal
        // `"system"` meant the default theme never rebuilt from a probed background.
        if self.theme_auto {
            self.theme = crate::theme::Theme::system_theme(Some(background), None);
            self.theme_name = self.theme.name.clone();
            self.transcript.invalidate();
        } else {
            self.apply_detected_scheme(background.scheme());
        }
    }

    /// Preview a theme without committing it (FR-UI-17).
    pub(super) fn preview_theme(&mut self, index: usize) {
        if let Some(name) = self.theme_names.get(index).cloned() {
            let (theme, _) = crate::theme::load_all(
                &name,
                crate::theme::detect_scheme(),
                &self.theme_extra_dirs,
            );
            self.theme = theme;
            self.transcript.invalidate();
        }
    }

    /// Keys the editor does not claim (the escape ladder, cancel, exit).
    fn global_key(&mut self, data: &str) -> Action {
        // Prompt jump (FR-UI-11): the authoritative matcher, since the
        // arrow-modifier dialects are not all round-tripped by `parse_key`.
        if self.keybindings.matches(data, "app.prompt.previous") {
            self.jump_prompt(-1);
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.prompt.next") {
            self.jump_prompt(1);
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.clear") {
            return if self.cancellable_turn_visible() {
                Action::CancelTurn
            } else if !self.editor.text().is_empty() {
                // gh #225: the hint promises `clear/exit` - text
                // present clears (and stands down an armed exit).
                self.editor.set_text("");
                self.world.ctrl_c_armed = false;
                Action::Continue
            } else if self.world.ctrl_c_armed {
                self.world.ctrl_c_armed = false;
                Action::Exit
            } else {
                self.world.ctrl_c_armed = true;
                Action::Continue
            };
        }
        // Gh #132: pi's double-escape sits ahead of the interrupt arm:
        // a lone Escape with an empty editor re-arms the 500 ms window
        // (the editor consumed its own Escapes on the way here: popup,
        // search, non-empty buffer). A running turn keeps its cancel
        // semantics, and a non-empty editor keeps the disarm below, so
        // this never fires mid-turn or off a clearing key.
        if !self.turn_running
            && keys::parse_key(data).as_deref() == Some("escape")
            && self.editor.text().trim().is_empty()
        {
            // The interrupt arm's disarm rides along: every Escape
            // still stands down an armed Ctrl+C.
            self.world.ctrl_c_armed = false;
            return self.double_escape();
        }
        if self.keybindings.matches(data, "app.interrupt") {
            return if self.cancellable_turn_visible() {
                Action::CancelTurn
            } else {
                self.world.ctrl_c_armed = false;
                Action::Continue
            };
        }
        if self.keybindings.matches(data, "app.message.followUp")
            && (self.turn_running || self.compacting)
        {
            // Queue a follow-up for turn end (ADR-0038) - unless it is a
            // slash command, which dispatches as a command, never as
            // model text (composer-polish fold-in). Gh #401: the same key
            // works during a compaction, where the message parks in the
            // compaction queue with its follow-up mode intact.
            let text = self.editor.submit();
            if text.trim().is_empty() {
                return Action::Continue;
            }
            if text.trim().starts_with('/') {
                if is_turn_boundary_command(text.trim()) {
                    self.queue_command(text, lca_protocol::SubmitMode::FollowUp);
                    return Action::Continue;
                }
                return self.dispatch_command(text.trim());
            }
            if self.compacting && !self.turn_running {
                self.queue_for_compaction(text, lca_protocol::SubmitMode::FollowUp);
                return Action::Continue;
            }
            self.queue_submit(text, lca_protocol::SubmitMode::FollowUp);
            return Action::Continue;
        }
        Action::Continue
    }

    /// Handle a submitted editor buffer.
    fn on_submit(&mut self, text: String) -> Action {
        self.submit_text(text, lca_protocol::SubmitMode::Steer)
    }

    /// Submit a buffer in one mode (gh #401). `Enter` submits as a steer,
    /// `Alt+Enter` as a follow-up; the mode only matters when the submit
    /// cannot start a turn, where it decides how the queue drains.
    fn submit_text(&mut self, text: String, mode: lca_protocol::SubmitMode) -> Action {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Action::Continue;
        }
        // `!cmd` / `!!cmd` shell mode (FR-UI-14).
        if trimmed.starts_with('!') {
            return self.run_shell_mode(trimmed);
        }
        if trimmed.starts_with('/') {
            // A slash command typed mid-turn never reaches the model as
            // text: turn-boundary commands queue as commands for turn
            // end, the rest dispatch at once (composer-polish fold-in).
            if self.turn_running && is_turn_boundary_command(trimmed) {
                self.queue_command(trimmed.to_string(), lca_protocol::SubmitMode::Steer);
                return Action::Continue;
            }
            return self.dispatch_command(trimmed);
        }
        // Gh #401 (pi's `queueCompactionMessage`): a compaction owns the
        // session, so a submit during it parks in the compaction queue
        // and flushes when it ends. Extension commands above still run
        // immediately, which is pi's rule and keeps `/...` responsive.
        if self.compacting && !self.turn_running {
            self.queue_for_compaction(text, mode);
            return Action::Continue;
        }
        // A turn needs a model; say so rather than failing deep in the
        // provider with a transport error.
        if self.model_label().trim().is_empty() {
            self.say(
                "No model is active. Use /login to sign in, or /model to choose one.".to_string(),
            );
            return Action::Continue;
        }
        // ADR-0038: while a turn runs, a submitted message queues rather
        // than starting a second turn. Enter steers (injected at the next
        // boundary); Alt+Enter queues a follow-up for turn end.
        if self.turn_running {
            self.queue_submit(text, mode);
            return Action::Continue;
        }
        self.transcript.push_user(text.clone());
        self.submitted_queue = None;
        self.submitted = Some(text);
        Action::Submit
    }
    /// Put the separator into a compaction with a reason (gh #388 gap 2
    /// tests). The live path calls the same setter from `poll_compact`.
    #[cfg(test)]
    pub(crate) fn mark_compacting_for_test(&mut self, reason: crate::CompactionReason) {
        self.separator.compacting(reason);
    }
}

/// The visible text of a line with ANSI stripped (for search matching).
pub(super) fn strip_ansi(line: &str) -> String {
    lca_tui::engine::text::strip_terminal_sequences(line)
}

/// Wrap case-insensitive occurrences of `query` in inverse video
/// (char-boundary-safe; skips a line when case folding changes its length).
pub(super) fn highlight_matches(line: &str, query: &str) -> String {
    let line_chars: Vec<char> = line.chars().collect();
    let query_chars: Vec<char> = query.to_lowercase().chars().collect();
    if query_chars.is_empty() {
        return line.to_string();
    }
    let lower: Vec<char> = line.to_lowercase().chars().collect();
    if lower.len() != line_chars.len() {
        return line.to_string();
    }
    let mut out = String::new();
    let mut i = 0;
    while i + query_chars.len() <= line_chars.len() {
        if lower[i..i + query_chars.len()] == query_chars[..] {
            out.push_str("\x1b[7m");
            out.extend(&line_chars[i..i + query_chars.len()]);
            out.push_str("\x1b[27m");
            i += query_chars.len();
        } else {
            out.push(line_chars[i]);
            i += 1;
        }
    }
    out.extend(&line_chars[i..]);
    out
}

#[cfg(test)]
#[path = "chat_offset_tests.rs"]
mod chat_offset_tests;
#[cfg(test)]
#[path = "chat_reload_keys_tests.rs"]
mod chat_reload_keys_tests;
#[cfg(test)]
#[path = "chat_toast_scrollback_tests.rs"]
mod chat_toast_scrollback_tests;
#[cfg(test)]
#[path = "chat_command_guards.rs"]
mod command_guards;
#[cfg(test)]
#[path = "chat_meter_tests.rs"]
mod meter_tests;
#[cfg(test)]
#[path = "startup_header_wired_tests.rs"]
mod startup_header_wired_tests;
#[cfg(test)]
#[path = "chat_tests.rs"]
pub(crate) mod tests;
#[cfg(test)]
#[path = "usage_tests.rs"]
mod usage_tests;

/// The caret's paint decision, for tests outside this crate (gh #371 bug 4).
///
/// `paint_caret` is the composition itself; this exists so the CLI's
/// config-to-frame rows can assert the same arithmetic the interface uses
/// without constructing a whole `Chat`.
pub fn paint_caret_for_tests(theme_is_colored: bool, show_hardware_cursor: bool) -> bool {
    crate::chat_focus::paint_caret(theme_is_colored, show_hardware_cursor)
}
