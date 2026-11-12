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

use lca_protocol::{Record, Usage};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::widgets::editor::Editor;

use crate::chat_commands::provider_for;
use crate::chat_pickers::{
    GrantPicker, ModelPicker, ShellRun, ThemePicker, ThinkingPicker, TreePicker, TrustPicker,
};

use crate::footer::Footer;

use crate::separator::Separator;
use crate::state::{TurnStatusLine, UiOptions, UiState};
// The tests reach `LoginNext` through this module's former import.
#[cfg(test)]
use crate::state::LoginNext;
// Gh #574: the tests reach `TurnEvent` and `Action` through this module's
// former imports, exactly as they reach `LoginNext` above. The library imports
// moved with the code that used them; the test glob (`super::*`) needs them
// here, and a test-side re-export is not a public-surface widening.
#[cfg(test)]
use crate::state::Action;
use crate::theme::Theme;
use crate::transcript::Transcript;
#[cfg(test)]
use lca_protocol::TurnEvent;

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

/// How long a toast floats: the **caveat/failure** window (gh #239, gh #374 gap 4).
///
/// pi gives its copy failures 5000 ms (`COPY_ERROR_FLASH_DURATION_MS`) against a
/// 1000 ms success default, because an error is a claim the reader acts on. A
/// success uses [`TOAST_SUCCESS_SECS`] instead.
pub const TOAST_SECS: u64 = 5;

/// How long a **success** toast floats (gh #374 gap 4): shorter, because a success
/// flash that overstays is noise while a caveat has to be read.
pub const TOAST_SUCCESS_SECS: u64 = 2;

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
    /// The document the last `viewport` composed (gh #374 gap 1), taken by
    /// [`Chat::take_document`]: one `sections` walk serves both the frame and the
    /// fullscreen exit dump.
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
    /// Gh #590: whether this turn's `TurnEvent::Error` already painted its
    /// transcript line. `TurnState::reap` reads it to decide whether the
    /// outcome's error still needs the fallback notice - a failed turn painted
    /// its message twice until this flag existed.
    ///
    /// Per **turn**, not per interface: it is reset by [`Chat::begin_turn`] and
    /// travels with its tab in the gh #462 swap set, so a background turn's
    /// suppression lands on the tab that painted it and never on the tab the
    /// reader happens to be looking at when the worker finishes.
    pub(crate) turn_error_painted: bool,
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
    /// A pending viewport target, and the jump already applied (FR-UI-11, gh #173,
    /// gh #375 gap 3). An **identity**, not a coordinate: see [`crate::chat_jump`].
    pub(crate) jump_target: Option<crate::chat_jump::JumpTarget>,
    pub(crate) pinned_jump: Option<crate::chat_jump::PinnedJump>,
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
    /// An in-progress picker press (gh #167): the pressed item **and the selection
    /// it replaced**. The second half is gh #386 gap 2 - a press previews, so a
    /// release elsewhere has to put the selection back, or the next `Enter` would
    /// confirm the preview's row. `None` means no press owns the gesture.
    pub(crate) picker_press: Option<(Option<usize>, usize)>,
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
    /// The size the last frame resolved overlays against (gh #373 gap 6).
    ///
    /// Kept because **input dispatch runs between frames** and must not disagree with
    /// the frame about whether an overlay is on screen. A predicate that is asked at
    /// paint time and never at key time is how an overlay hidden by a width breakpoint
    /// goes on owning the keyboard - the exact failure the option exists to prevent.
    pub(super) last_frame_size: std::cell::Cell<Option<(u16, u16)>>,
    /// The extension side panel's own visibility predicate (gh #373 gap 6).
    ///
    /// The panel is LCA's floating HUD: it takes the right 40 columns while open, so on
    /// a narrow pane it should hide itself. `None` means always show it while
    /// `panel_open` - every existing host's behaviour.
    pub(super) panel_visible: Option<Box<dyn Fn(u16, u16) -> bool>>,
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
        // Gh #373 gap 6: the host's panel breakpoint, taken before `world` moves in.
        let panel_visible: Option<Box<dyn Fn(u16, u16) -> bool>> =
            world.options.hooks.panel_visible.clone().map(
                |visible| -> Box<dyn Fn(u16, u16) -> bool> {
                    Box::new(move |cols, rows| visible(cols, rows))
                },
            );
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
            turn_error_painted: false,
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
            pinned_jump: None,
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
            last_frame_size: std::cell::Cell::new(None),
            panel_visible,
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
}

mod chat_highlight;
pub(crate) use chat_highlight::{highlight_matches, strip_ansi};

// The turn lifecycle and the theme/keybinding group live in sibling files
// (gh #574): an inherent `impl Chat` may be partitioned this way, so the
// struct keeps its one definition and every method keeps its path.
mod chat_input;
mod chat_theme_keys;
mod chat_turn_state;
// Gh #388 gap 2's test seam, beside the lifecycle it fakes (gh #574).
#[cfg(test)]
mod chat_turn_test_seams;

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
#[path = "chat_image_crop_tests.rs"]
mod image_crop_tests;
#[cfg(test)]
#[path = "chat_meter_tests.rs"]
mod meter_tests;
#[cfg(test)]
#[path = "chat_paste_tests.rs"]
mod paste_tests;
#[cfg(test)]
#[path = "startup_header_wired_tests.rs"]
mod startup_header_wired_tests;
#[cfg(test)]
#[path = "chat_tests.rs"]
pub(crate) mod tests;
#[cfg(test)]
#[path = "chat_tests_support.rs"]
mod tests_support;
#[cfg(test)]
#[path = "usage_tests.rs"]
mod usage_tests;

/// The caret's paint decision for rows outside this crate (gh #371 bug 4): a
/// wrapper, not a `pub use`, because `chat_focus::paint_caret` is `pub(crate)` -
/// widening *that* would put a helper on every embedder's surface.
pub fn paint_caret_for_tests(theme_is_colored: bool, show_hardware_cursor: bool) -> bool {
    crate::chat_focus::paint_caret(theme_is_colored, show_hardware_cursor)
}
