//! Interactive mode: resolve sessions, provider, and tools, then run the
//! terminal interface against the real agent loop.
//!
//! **The composition root (S1).** `run` builds one `Ui` — the wiring
//! state every closure here used to capture as a local — and then hands the
//! interface exactly three things: its options, its hooks, and its turn
//! runner. The closures are methods on `Ui` (`commands`, `hooks`, `login`),
//! so `run` is orchestration and its helpers stay under the line budget.

mod attach;
mod clipboard;
mod close;
mod commands;
mod commands_list;
pub(crate) use attach::stage_initial_attachments;
pub(crate) use commands::settings_text;
pub(crate) use model_select::OfferedCache;
mod display;
mod editor;
mod extman;
mod hooks;
mod hooks_mcp;
mod hub;
pub(crate) use hub::OFFICIAL_CATALOG_URL;
mod cells;
mod label_hooks;
pub mod login;
mod model_rows;
mod model_select;
// Gh #543: the pre-interface half of the composition root.
mod open;
mod pending_step;
// Gh #543: the CLI's own prompt surfaces (`SessionPrompt`, `SessionDialogs`).
mod prompt_dialogs;
// `pub(in crate::tui)`, matching the items' own reach: a re-export may not
// widen what it re-exports, and `pub(super)` here would mean the crate root.
pub(in crate::tui) use prompt_dialogs::{SessionDialogs, SessionPrompt};
mod persona;
// Gh #543: `/reload` and the `ui.json` flag it re-reads.
mod reload;
mod runner;
mod session_hooks;
pub mod startup;
pub mod tmux_doctor;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lca_config::Config;
use lca_core::{AgentConfig, ExtensionRegistry};
use lca_permissions::{GrantStore, Proposals, SharedPrompt};
use lca_provider::Provider;
use lca_session::{Session, SessionStore};
use lca_tools::ToolExecutor;
use lca_ui::{RegionInteractor, RegionRenderer};

use crate::lock;

// Gh #543: the pre-interface helpers moved to `cells.rs`/`open.rs`; re-exported
// here because sibling modules and one test read `super::<name>`, exactly as
// they did when the definitions lived at the bottom of this file.
// `pub(in crate::tui)`: the same reach a private item at this file used to have
// for its descendants, so nothing is widened by the move.
pub(in crate::tui) use cells::{
    adopt_login_identity, agent_config_for, live_cells, model_context_window,
};
// Was `pub(super)` in this file, which is `pub(crate)`; see `cells.rs`.
pub(crate) use cells::apply_models_scope;
pub(in crate::tui) use open::{region_interactor, region_renderer, resolve_model_id};

/// Collect pre-parse markdown transforms from extension handles (gh
/// #12): each native handle's optional Rust-side transform becomes one
/// pipeline entry in registration order; absent transforms contribute
/// nothing. WASM handles carry none (no WIT surface yet), so this only
/// ever fires for native handles.
pub fn collect_markdown_transformers(
    handles: &[lca_ext_native::NativeHandle],
) -> Vec<lca_ui::MarkdownTransformer> {
    handles
        .iter()
        .filter_map(|handle| {
            let transform = handle.markdown_transformer()?;
            let adapted: lca_ui::MarkdownTransformer = Arc::new(move |text, context| {
                transform(
                    text,
                    &lca_ext_abi::MarkdownTransformContext {
                        message_type: match context.message_type {
                            lca_ui::MarkdownMessageType::User => {
                                lca_ext_abi::MarkdownMessageType::User
                            }
                            lca_ui::MarkdownMessageType::Assistant => {
                                lca_ext_abi::MarkdownMessageType::Assistant
                            }
                            // Gh #390 gap 2: a reasoning run is forwarded as
                            // itself, not folded into `Assistant` - an extension
                            // that registers an artefact transformer needs the
                            // same distinction the built-in mermaid path uses.
                            lca_ui::MarkdownMessageType::AssistantThinking => {
                                lca_ext_abi::MarkdownMessageType::AssistantThinking
                            }
                        },
                        is_streaming: context.is_streaming,
                        available_width: context.available_width,
                    },
                )
            });
            Some(adapted)
        })
        .collect()
}

pub(crate) use display::session_stats;

/// The session's live model: the runner reads it per turn, the
/// status-line label follows it, and the compaction backend is moved
/// with `set_model` - `/model` rewrites all three.
#[derive(Clone, Debug)]
pub(crate) struct ModelChoice {
    /// The model identifier.
    pub(crate) id: String,
    /// Its context window (the threshold math needs it).
    pub(crate) window: u32,
}

/// The composition root's wiring state. Every field was a `run` local a
/// closure captured; the closures are methods now.
pub(crate) struct Ui {
    /// The workspace the interface opened in.
    cwd: PathBuf,
    /// The data directory (grant store, credentials, sessions).
    data: PathBuf,
    /// This session's temp dir (gh #160).
    temp_dir: PathBuf,
    /// The command line's flag layer (gh #30): re-read with the config
    /// whenever the `/settings` selector asks for rows, so a flag-set
    /// value keeps its `flag` source after a write.
    flags: crate::CliFlags,
    /// The session store.
    store: Arc<SessionStore>,
    /// The merged configuration, swappable at runtime (gh #204's
    /// checklist and gh #130's reload both write through this cell, so
    /// the cycle always reads the live scope, never a stale copy).
    config: Mutex<Config>,
    /// The shared grant store (one owner: ADR-0006).
    grants: Arc<Mutex<GrantStore>>,
    /// The session the interface is showing; swappable (`/tree`, `/resume`).
    current_session: Arc<Mutex<Session>>,
    /// Every session id this run opened (gh #209): tabs, switches,
    /// and resumes all land here, so `close` ends each one cleanly
    /// instead of leaving background tabs crash-shaped.
    opened_sessions: Arc<Mutex<Vec<String>>>,
    /// Open the session picker instead of a fresh session (gh #110:
    /// bare `-r`), carried into `UiOptions` for the run loop.
    resume_picker: bool,
    /// The live provider generation (gh #177): the name, the resolved
    /// provider, and the agent config built for them, under one lock so
    /// a `/model` switch or a `/login` move never mixes generations.
    /// A turn clones this whole; the compaction backend is stable (its
    /// own `set_provider` follows the switch in place).
    live: Arc<Mutex<LiveTarget>>,
    /// The loaded extension registry, behind a lock so `/reload` (gh
    /// #130) can swap in a fresh discovery while turns hold their own
    /// clone - a swap never disturbs a running turn.
    registry: Mutex<Arc<ExtensionRegistry>>,
    /// The session's inbound HTTP listener (gh #363, ADR-0049), behind a lock
    /// because `Ui` is shared via `Arc` and stopping needs `&mut`. `Drop` also
    /// stops it, so an exiting session leaves no bound port.
    session_http: Mutex<crate::session_http::SessionHttp>,
    /// The terminal taken before this load, with what was typed into it
    /// (gh #400). Behind a lock because `options` takes `&Arc<Self>` and the
    /// loop takes ownership exactly once.
    early_input: Mutex<Option<lca_ui::run_early_input::EarlyInput>>,
    /// The adapter's settings cell (ADR-0035).
    settings_cell: Arc<Mutex<Vec<(String, String)>>>,
    /// The live model (`/model`).
    model_cell: Arc<Mutex<ModelChoice>>,
    /// The status-line label cell.
    label_cell: Arc<Mutex<String>>,
    /// The display identity (E5): the login preset when one is known
    /// (`opencode-go`), else the extension name. `/model` reads it so a
    /// switch keeps the preset label.
    identity_cell: Arc<Mutex<String>>,
    /// The live context window (FR-UI-20).
    context_window_cell: Arc<Mutex<u64>>,
    /// The live thinking level (R1).
    thinking_cell: Arc<Mutex<Option<String>>>,
    /// The live theme setting (E2): the committed `/theme` pick, so
    /// `/settings` agrees with it the way it agrees on `thinking`.
    theme_cell: Arc<Mutex<String>>,
    /// The compaction provider backend, when the bundled strategy is on.
    provider_backend: Option<Arc<lca_core::ext_provider::ProviderBackend>>,

    /// Images staged for the next turn (`/attach`).
    pending_attachments: Arc<Mutex<Vec<lca_core::StagedAttachment>>>,
    /// Held for a volatile run: dropping it removes the temp store.
    _volatile: Option<crate::invoke::VolatileStore>,
    /// The `/login` flow state.
    flow: Arc<Mutex<crate::login::LoginFlow>>,
    /// The last `/login` answer, so the endpoint grant's approval can re-run
    /// model discovery now that the host is reachable (#4).
    login_answer: Arc<Mutex<Option<LoginAnswer>>>,
    /// The provider override presets (`provider-presets.toml`).
    preset_overrides: String,
    /// The tool executor (shared with the runner).
    tools: Arc<Mutex<ToolExecutor>>,
    /// The shell the executor resolved at startup, mirrored so `/settings`
    /// never locks `tools` (the runner holds that lock for a whole turn -
    /// #20 / V1).
    resolved_shell: Option<lca_tools::Shell>,
    /// The shell resolution error, when the backend is broken (mirrored
    /// with the shell for the same reason).
    resolved_shell_error: Option<String>,
    /// The project's proposed permission patterns, when trusted.
    proposals: Option<Proposals>,
    /// The swappable prompt slot capability engines share.
    shared_prompt: SharedPrompt,
    /// The dialog router extensions ask through (gh #124), kept so
    /// `/reload` reuses it instead of orphaning the slot.
    shared_dialogs: lca_permissions::SharedDialogs,
    /// The CLI `--yolo` flag, re-applied when settings reload.
    yolo: bool,
    /// The interface's session-lifetime prompt sender: published into
    /// `UiOptions` for `run` to fill, read by [`SessionPrompt`] so the
    /// host's own consent can ask outside a turn (gh #31 review).
    prompt_slot: Arc<Mutex<Option<std::sync::mpsc::SyncSender<lca_ui::PromptRequest>>>>,
    /// The session-lifetime dialog sender (gh #124): published into
    /// `UiOptions` for `run` to fill, read by [`SessionDialogs`].
    dialog_slot: Arc<Mutex<Option<std::sync::mpsc::SyncSender<lca_ui::DialogExchange>>>>,
    /// Rows discovered after the endpoint consent landed, for the picker
    /// to open with (`None`: nothing pending).
    pending_models: Arc<Mutex<Option<Vec<lca_ui::ModelRow>>>>,
    /// The last model enumeration, for the pick path (gh #240): a
    /// picker Enter carries only the row id, and re-enumerating every
    /// provider (WASM calls, credential probes, live `GET /models`)
    /// on selection froze the interface for seconds. The memo serves
    /// exact-id hits with zero enumeration; anything else falls back
    /// to the fresh listing. Invalidation is structural, never timed:
    /// the registry Arc changes on `/reload`, and membership flips
    /// (login/logout/toggle) clear eagerly at the command site —
    /// both directions are safe (a stale miss just re-enumerates).
    offered_cache: Mutex<OfferedCache>,
    /// Whether the picker's endpoint consent is already asking, so a
    /// second `/model` while it waits does not ask again.
    consent_in_flight: Arc<std::sync::atomic::AtomicBool>,
    /// Extension render regions (FR-UI-1).
    render_regions: Option<RegionRenderer>,
    /// Extension ui events (FR-UI-6).
    ui_events: Option<RegionInteractor>,
    /// The initial transcript: warnings that frame it (head), the session's
    /// records for the rich replay (FR-UI-7), and trailing notices.
    initial_head: Vec<String>,
    /// The startup header's two prepared states (gh #400 gaps 2 and 3): the compact
    /// line and the expanded table, kept apart from `initial_head` because the
    /// header is a live entry (it toggles, and it outlives `/new`) rather than a
    /// baked string.
    initial_header: (Vec<String>, Vec<String>),
    initial_records: Vec<lca_protocol::Record>,
    initial_tail: Vec<String>,
    /// Positional CLI messages (#109), submitted on open in order.
    initial_messages: Vec<String>,
    /// The background update check's finding (FR-CFG-6).
    update_notice: Arc<std::sync::OnceLock<String>>,
    /// R4: a background login/identity step's result, taken by the
    /// interface's `poll_login` hook.
    login_pending: Arc<Mutex<Option<lca_ui::LoginNext>>>,
    /// #102: the generation counter behind `login_pending`. Each new
    /// step issues one; a delivery or cancel from another generation is
    /// stale and dropped, so a slow discovery can never pop a picker
    /// its Escape already dismissed.
    login_generation: Arc<std::sync::atomic::AtomicU64>,
    /// The background `/compact`'s state, read by the interface's
    /// `poll_compact` hook so a summarization call never blocks it.
    compact_state: Arc<Mutex<lca_ui::CompactState>>,
    /// #227: a finished background install's notice, taken once by
    /// the interface's `poll_install` hook.
    hub_install: Arc<Mutex<hub::InstallSlot>>,
    /// R4: the provider handle a background step is running against, for
    /// manual-callback delivery and cancellation.
    login_handle: Arc<Mutex<Option<lca_ext_native::NativeHandle>>>,
    /// R4: whether the manual "paste the callback URL" field was offered
    /// for the current wait.
    login_manual_offered: Arc<Mutex<bool>>,
    /// R4: the auth URL already folded into the waiting label.
    login_url_shown: Arc<Mutex<Option<String>>>,
    /// R4: when the current wait began (the manual offer follows a quiet
    /// period).
    login_wait_since: Arc<Mutex<Option<std::time::Instant>>>,
    /// R4: Escape cancelled the current wait, so its background result is
    /// ours to report as a cancel rather than as a failure.
    login_cancelled: Arc<Mutex<bool>>,
    /// The MCP session manager (gh #53): files, bridge, and actions.
    /// The cell swaps on `/reload`; the bridge rebuilds in place.
    mcp: Arc<Mutex<Arc<lca_mcp::manager::McpManager>>>,
    /// The MCP load warnings, kept for the `/mcp` modal (gh #418 Phase G): a
    /// config error explains a server that is missing from the list, and the
    /// modal shows them above the rows. The startup head showed them once; a
    /// user opening `/mcp` later still needs them.
    mcp_warnings: Arc<Vec<String>>,
}

/// Enter the interactive interface for `cwd`, optionally resuming `resume`.
#[allow(clippy::too_many_arguments)] // thin entry seam: every arg is used once, at one call site.
pub fn run(
    cwd: &Path,
    resume: Option<&str>,
    resume_picker: bool,
    yolo: bool,
    model: Option<&str>,
    initial: &[String],
    initial_attachments: &[std::path::PathBuf],
    fork: Option<&str>,
    session_id: Option<&str>,
    allow_host: &[String],
    flags: &crate::CliFlags,
) -> anyhow::Result<i32> {
    // #96: installed here too, so the TUI entry restores the terminal
    // even when reached without `main` (embedding, tests). Idempotent.
    lca_tui::install_panic_hook();
    // Gh #403 gap 4: `SIGHUP` becomes an orderly exit, not a signal death.
    startup::install_hangup_handler();
    // The interface needs a terminal for raw mode and key events; without
    // one the input read fails with an opaque error. Say what to do instead
    // (the headless path is the scripted one).
    use std::io::IsTerminal as _;
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "the interactive interface needs a terminal; use `lca -p \"...\"` for headless mode"
        );
        return Ok(crate::exit::USAGE);
    }
    // Gh #400: claim the terminal **before** the load below, so the keyboard is
    // read from the first frame, and start the tmux probe there too - the load
    // hides its cost. See `startup::claim_terminal`.
    let (early_input, tmux_doctor) = startup::claim_terminal();
    let ui = Arc::new(Ui::new(
        cwd,
        resume,
        resume_picker,
        yolo,
        model,
        initial,
        initial_attachments,
        fork,
        session_id,
        allow_host,
        flags,
        (early_input, tmux_doctor),
    )?);
    // Gh #400: the checkpoint. A `Ctrl+C` typed during the load above asked to
    // leave; honour it now rather than entering a loop the user already
    // abandoned. The terminal is restored by the drop below, exactly as a
    // normal exit restores it.
    if ui.quit_requested_during_startup() {
        ui.close();
        return Ok(crate::exit::OK);
    }
    // Gh #160: the guard owns this session's temp dir (resolved and
    // validated inside `Ui::new`); a switch resolves the next one.
    let _temp_guard = crate::SessionTempGuard(ui.temp_dir().to_path_buf());
    let options = ui.options();
    let runner = ui.turn_runner();
    let result = lca_ui::run(options, runner);
    ui.close();
    result
}

/// A stashed `/login` answer: the provider, the chosen option id, and the
/// typed field values, replayed to re-run discovery after the grant (#4).
type LoginAnswer = (String, String, std::collections::BTreeMap<String, String>);

/// One provider generation a session runs on (gh #177).
struct LiveTarget {
    /// The active provider extension name.
    name: String,
    /// The resolved provider.
    provider: Arc<dyn Provider>,
    /// The agent config reused per turn, built for this generation.
    agent_config: AgentConfig,
}

impl Ui {
    /// The active provider extension name.
    fn live_name(&self) -> String {
        crate::lock(&self.live).name.clone()
    }

    /// The current extension registry (gh #130).
    fn registry(&self) -> Arc<ExtensionRegistry> {
        crate::lock(&self.registry).clone()
    }

    /// Whether the user asked to quit while startup was still loading
    /// (gh #400). Drains the buffer, so a keystroke typed during the load is
    /// not lost to this check.
    fn quit_requested_during_startup(&self) -> bool {
        crate::lock(&self.early_input)
            .as_mut()
            .is_some_and(lca_ui::run_early_input::EarlyInput::quit_requested)
    }

    /// The MCP session manager (gh #418 Phase E: the turn waits on its
    /// startup connections, bounded, before the first request).
    fn mcp_manager(&self) -> Arc<lca_mcp::manager::McpManager> {
        crate::lock(&self.mcp).clone()
    }

    /// The resolved provider this generation runs on.
    fn live_provider(&self) -> Arc<dyn Provider> {
        crate::lock(&self.live).provider.clone()
    }

    /// Build the wiring state (S1): resolve the session, provider, and
    /// tools, then assemble the extension registry and the live cells.
    #[allow(clippy::too_many_arguments)] // one call site; a params struct for nine scalars is theater.
    fn new(
        cwd: &Path,
        resume: Option<&str>,
        resume_picker: bool,
        yolo: bool,
        model_override: Option<&str>,
        initial: &[String],
        initial_attachments: &[std::path::PathBuf],
        fork: Option<&str>,
        session_id: Option<&str>,
        allow_host: &[String],
        flags: &crate::CliFlags,
        (early_input, tmux_doctor): (lca_ui::run_early_input::EarlyInput, tmux_doctor::Cell),
    ) -> anyhow::Result<Ui> {
        let open::Opened {
            _volatile: volatile,
            data,
            store,
            config,
            grants,
            trusted,
            current_session,
            session_id,
            provider_name,
            mut initial_head,
            initial_records,
            mut initial_tail,
            update_notice,
        } = open::open(cwd, resume, fork, session_id, yolo, allow_host, flags)?;
        // gh #69: the session temp follows the store root (volatile runs
        // keep everything under their temp dir).
        let session_data_root = volatile
            .as_ref()
            .map(|volatile| volatile.root().to_path_buf())
            .unwrap_or_else(|| data.clone());

        // ADR-0041: the interpreter is resolved once, here, so the tool
        // description, `/settings`, and every call agree - and a configured
        // interpreter that is missing is loud at startup, never a silent
        // switch to another shell.
        let ops = crate::native_ops(&config);
        if let Some(error) = ops.error() {
            tracing::warn!(%error, "shell resolution failed");
            initial_head.push(format!("warning: {error}"));
        }
        // Interactive runs carry no default timeout (gh #40, pi's
        // default): the user cancels, and that is the mechanism. An
        // explicit `tool.timeout_seconds` still applies.
        let tools = Arc::new(Mutex::new(ToolExecutor::new(
            Arc::new(ops),
            cwd.to_path_buf(),
            cwd.to_path_buf(),
            config.tool_result_limit_bytes() as usize,
            crate::configured_tool_timeout(&config),
        )));
        // V1 (#20): the resolved shell, mirrored out of the executor once,
        // because `/settings` renders on the input thread while the turn
        // worker holds `tools` for the whole turn - locking it there froze
        // the interface and queued Ctrl+C behind a running turn.
        let (resolved_shell, resolved_shell_error) = {
            let guard = tools.lock().unwrap_or_else(|p| p.into_inner());
            (
                guard.resolved_shell().cloned(),
                guard.resolved_shell_error().map(str::to_string),
            )
        };
        let proposals: Option<Proposals> = trusted.then(|| config.permissions_proposals().clone());
        // The one swappable prompt slot every capability engine shares; the
        // turn runner installs the interface's modal into it, so an
        // extension's own process/pty command asks the user exactly like a
        // model command does.
        let shared_prompt = SharedPrompt::default();
        // The host's own consent (the picker's endpoint consent, gh #31
        // review) asks through the same seam a turn's prompt uses, and it
        // must work before any turn has run - so a session-lifetime prompt
        // goes in here; `turn_worker` replaces it with the turn's own,
        // which now rides the same session channel anyway.
        let prompt_slot: Arc<Mutex<Option<std::sync::mpsc::SyncSender<lca_ui::PromptRequest>>>> =
            Arc::new(Mutex::new(None));
        shared_prompt.set(Arc::new(Mutex::new(SessionPrompt {
            slot: prompt_slot.clone(),
        })));
        // The dialog seam (gh #124): same session-lifetime shape as the
        // prompt above, installed once - dialogs can arrive outside
        // turns too (a command asking), so this is not per-turn.
        let dialog_slot: Arc<Mutex<Option<std::sync::mpsc::SyncSender<lca_ui::DialogExchange>>>> =
            Arc::new(Mutex::new(None));
        let shared_dialogs = lca_permissions::SharedDialogs::default();
        shared_dialogs.set(Arc::new(Mutex::new(SessionDialogs {
            slot: dialog_slot.clone(),
        })));

        // Gh #160: this session's temp dir resolves here, from this
        // session - creation failures fail startup, never silently.
        let temp_dir = crate::ensure_session_temp(&session_data_root, &session_id)?;
        let (mut registry, mcp_manager, mcp_warnings) = startup::load_registry(
            cwd,
            &config,
            shared_prompt.clone(),
            shared_dialogs.clone(),
            &grants,
            &store,
            &current_session,
            &temp_dir,
            flags,
        );
        // gh #53: MCP load warnings ride the startup head with the
        // rest (invalid entries never block the valid ones).
        for warning in &mcp_warnings {
            initial_head.push(warning.clone());
        }
        // gh #67: the run's tool selection, warned into the startup head.
        for warning in crate::invoke::apply_tool_selection(&registry, flags) {
            initial_head.push(warning);
        }

        // Gh #363 / ADR-0049: start the inbound HTTP listener. It mounts only
        // the handles that declared `serve-http`, so an ungranted extension has
        // no route rather than a route that refuses; and a listener that cannot
        // start degrades to a notice instead of costing the user their agent.
        let session_http = crate::session_http::start_for_session(&registry, &session_id);
        // The notice names the URL and the port actually bound, never the
        // default: a user told 8080 while the listener is on 8083 has been told
        // something false. It is also the only place the token is rendered.
        if let Some(notice) = session_http.notice() {
            initial_head.push(notice);
        }

        // FR-PROV-6: the configured provider resolves to an enabled handle,
        // or the session opens in the zero-provider state (FR-PROV-9). The
        // interactive surface must never be lockable from the inside, so a
        // user who disables the only provider still has a `/login` to bring
        // one back with. (The registry stays mutable until the two
        // completion-dependent handles are in.)
        let missing_provider = registry.provider(&provider_name).is_none();
        // The adapter's settings cell is how the host-persisted settings
        // reach `list-models` (ADR-0035). The login flow writes what it
        // persists into this same cell, so `list-models` sees the discovered
        // model list from the one place `complete` already reads it.
        let settings_cell: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn Provider> = match registry.provider(&provider_name) {
            Some(handle) => Arc::new(lca_core::ExtensionProvider::new_with_settings(
                handle.clone(),
                settings_cell.clone(),
            )),
            None => Arc::new(lca_provider::NoProvider::new(provider_name.clone())),
        };
        if missing_provider {
            // The first frame says what is wrong and how to leave this state;
            // the report is the same one the headless path prints. It trails
            // the transcript, after the records replay.
            initial_tail.push(String::new());
            initial_tail.push(crate::no_model_message(&provider_name));
            initial_tail.push("Use /login to sign in to a provider.".to_string());
        }

        let provider_is_ready = crate::provider_ready(&provider_name, &data);
        // The configured endpoint can be outside the provider's manifest
        // hosts (gh #29, gh #157): computed here, where the registry
        // exists to read the manifest off, not in `initial_view`, which
        // runs before it. Say so up front and name what actually happens
        // next: the first request raises the consent prompt naming that
        // host, and a scripted run has `--allow-host`. This is the
        // env-var path, which never runs `/login` on its own.
        let needs = crate::provider_needs::provider_needs(&registry, &data, &provider_name);
        if provider_is_ready
            && let Some(host) = crate::ungranted_host_in(
                &grants,
                cwd,
                crate::provider_needs::provider_ad_hoc_host(&data, &provider_name, needs.as_ref()),
                needs.as_ref().map_or(&[][..], |n| n.local.as_slice()),
            )
        {
            initial_head.push(format!(
                "note: the endpoint {host} is not granted for this project - the first \
                 request will ask you to approve it; a script passes --allow-host {host}"
            ));
        }
        // Gh #112: malformed screen-mode persistence falls back and says so on
        // the transcript head (never a silent substring).
        if let (_, Some(warning)) = hooks::initial_screen_mode(&data, config.ui_fullscreen()) {
            initial_head.push(warning);
        }
        // Gh #131/#400: the header names the loaded resources - one compact line
        // with the table behind `ctrl+o` since gaps 2/3, plus the registry's
        // collision notices and the tmux doctor's finding (gap 4).
        let initial_header =
            startup::launch_header(&data, cwd, trusted, flags, &registry, &config, &tmux_doctor);
        // `--model <pattern>[:thinking]` resolves against the provider's
        // list the way pi's resolver does (EFG-041), inside `--provider`'s
        // scope when a profile is named; the `:thinking` suffix becomes
        // this session's level. A pattern nothing matches is the id
        // itself - the endpoint may know a model the list does not.
        let mut override_thinking: Option<String> = None;
        let model_id = match model_override {
            Some(pattern) => {
                let mut candidates = provider.list_models();
                if let Some(profile) = flags.provider.as_deref() {
                    candidates
                        .retain(|model| crate::models::in_provider(model, profile, &provider_name));
                    if candidates.is_empty() {
                        anyhow::bail!(
                            "unknown provider \"{profile}\". Use --list-models to see available models."
                        );
                    }
                }
                match crate::models::resolve_pattern(pattern, &candidates) {
                    Ok(resolved) => {
                        override_thinking = resolved.thinking;
                        resolved.id
                    }
                    Err(err) => anyhow::bail!(err),
                }
            }
            None => resolve_model_id(&config, provider_is_ready, provider.as_ref()),
        };
        let provider_backend = crate::registry::register_compaction(
            &mut registry,
            &provider,
            &model_id,
            &session_id,
            cwd,
            shared_prompt.clone(),
            &grants,
            &temp_dir,
        );
        let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> = provider_backend
            .clone()
            .map(|backend| backend as Arc<dyn lca_tools::CompletionBackend>);
        crate::apply_enablement(&mut registry, |name| {
            lock(&grants).extension_enabled(cwd, name) == Some(false)
        });
        let registry = Arc::new(registry);

        let context_window = model_context_window(provider.as_ref(), &model_id);
        // #39: the resolved model's image behavior reaches the tools.
        let image_policy = crate::models::image_policy_for(&provider.list_models(), &model_id);
        let trusted = crate::lock(&grants).is_trusted(cwd);
        let agent_config = agent_config_for(
            &config,
            &provider_name,
            &model_id,
            context_window,
            image_policy,
            &registry,
            completion_backend,
            cwd,
            flags,
            trusted,
        )
        .map_err(|err| anyhow::anyhow!("{err}"))?;

        // The ui view over the registry: only handles that declared
        // regions register here (FR-UI-1's pull table; deny-by-default
        // comes from ui_regions returning empty).
        let render_regions = region_renderer(registry.clone());
        let ui_events = region_interactor(registry.clone());

        // E5: name the login preset when one was stored, else the extension.
        let identity = crate::stored_provider_preset(&data, &provider_name)
            .unwrap_or_else(|| provider_name.clone());
        // gh #8 phase 4: the session's level follows the model it starts
        // on. `--model sonnet:high` is explicit (clamped into what the
        // model accepts, never persisted); otherwise the model's own
        // configured default wins over the configured `thinking` - pi's
        // per-model precedence - which is clamped into the model's set.
        let thinking = match override_thinking {
            Some(level) => config.clamp_thinking(Some(&level), &model_id),
            None => {
                config.switch_thinking(config.thinking().map(str::to_string).as_deref(), &model_id)
            }
        };
        let cells = live_cells(&identity, &model_id, context_window, thinking);

        // Every picker choice: each enabled provider extension's own options,
        // plus the user's named custom endpoints (D1's override layer). The
        // host's universal "Custom endpoint..." entry is appended by the flow.
        let preset_overrides =
            std::fs::read_to_string(crate::data_dir().join("provider-presets.toml"))
                .unwrap_or_default();
        let flow = Arc::new(Mutex::new(crate::login::LoginFlow::new()));
        let theme_setting = config.ui_theme().unwrap_or("auto").to_string();

        // gh #71: opener `@file` images stage before construction
        // (the struct moves the session, so the clone happens here).
        let staged_attachments =
            stage_initial_attachments(&store, &Arc::clone(&current_session), initial_attachments);
        Ok(Ui {
            cwd: cwd.to_path_buf(),
            data,
            temp_dir: temp_dir.clone(),
            flags: flags.clone(),
            store,
            config: Mutex::new(config),
            grants,
            current_session: Arc::clone(&current_session),
            opened_sessions: Arc::new(Mutex::new(vec![
                current_session
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .id()
                    .to_string(),
            ])),
            resume_picker,
            live: Arc::new(Mutex::new(LiveTarget {
                name: provider_name,
                provider,
                agent_config,
            })),
            registry: Mutex::new(registry),
            session_http: Mutex::new(session_http),
            early_input: Mutex::new(Some(early_input)),
            settings_cell,
            model_cell: cells.model,
            label_cell: cells.label,
            identity_cell: cells.identity,
            context_window_cell: cells.context_window,
            thinking_cell: cells.thinking,
            theme_cell: Arc::new(Mutex::new(theme_setting)),
            provider_backend,
            pending_attachments: Arc::new(Mutex::new(staged_attachments)),
            _volatile: volatile,
            flow,
            login_answer: Arc::new(Mutex::new(None)),
            preset_overrides,
            tools,
            resolved_shell,
            resolved_shell_error,
            proposals,
            shared_prompt,
            shared_dialogs: shared_dialogs.clone(),
            yolo,
            prompt_slot,
            dialog_slot,
            pending_models: Arc::new(Mutex::new(None)),
            offered_cache: Mutex::new(OfferedCache::default()),
            consent_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            render_regions,
            ui_events,
            initial_head,
            initial_header,
            initial_records,
            initial_tail,
            initial_messages: initial.to_vec(),
            update_notice,
            login_pending: Arc::new(Mutex::new(None)),
            login_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            compact_state: Arc::new(Mutex::new(lca_ui::CompactState::Idle)),
            hub_install: Arc::new(Mutex::new(hub::InstallSlot::default())),
            login_handle: Arc::new(Mutex::new(None)),
            login_manual_offered: Arc::new(Mutex::new(false)),
            login_url_shown: Arc::new(Mutex::new(None)),
            login_wait_since: Arc::new(Mutex::new(None)),
            login_cancelled: Arc::new(Mutex::new(false)),
            mcp: Arc::new(Mutex::new(mcp_manager)),
            mcp_warnings: Arc::new(mcp_warnings),
        })
    }
}
