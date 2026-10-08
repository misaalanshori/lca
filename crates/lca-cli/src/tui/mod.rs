//! Interactive mode: resolve sessions, provider, and tools, then run the
//! terminal interface against the real agent loop.
//!
//! **The composition root (S1).** `run` builds one `Ui` — the wiring
//! state every closure here used to capture as a local — and then hands the
//! interface exactly three things: its options, its hooks, and its turn
//! runner. The closures are methods on `Ui` (`commands`, `hooks`, `login`),
//! so `run` is orchestration and its helpers stay under the line budget.

mod commands;
pub(crate) use commands::settings_text;
mod display;
mod hooks;
mod login;
mod runner;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lca_config::Config;
use lca_core::{AgentConfig, ExtensionRegistry};
use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, Proposals, SharedPrompt,
};
use lca_provider::Provider;
use lca_session::{Session, SessionStore};
use lca_tools::ToolExecutor;
use lca_ui::{RegionInteractor, RegionRenderer};

use crate::lock;

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

/// The built-in slash slots the interface itself claims; the spec's
/// sixth built-in, `/stats`, arrives from the native hooks extension
/// that holds the stats source (ADR-0013).
const BUILTIN_SLOTS: [&str; 7] = [
    "login", "logout", "usage", "model", "compact", "attach", "session",
];

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
    initial_records: Vec<lca_protocol::Record>,
    initial_tail: Vec<String>,
    /// Positional CLI messages (#109), submitted on open in order.
    initial_messages: Vec<String>,
    /// The background update check's finding (FR-CFG-6).
    update_notice: Arc<std::sync::OnceLock<String>>,
    /// R4: a background login/identity step's result, taken by the
    /// interface's `poll_login` hook.
    login_pending: Arc<Mutex<Option<lca_ui::LoginNext>>>,
    /// The background `/compact`'s state, read by the interface's
    /// `poll_compact` hook so a summarization call never blocks it.
    compact_state: Arc<Mutex<lca_ui::CompactState>>,
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
    allow_host: &[String],
    flags: &crate::CliFlags,
) -> anyhow::Result<i32> {
    // #96: installed here too, so the TUI entry restores the terminal
    // even when reached without `main` (embedding, tests). Idempotent.
    lca_tui::install_panic_hook();
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
    let ui = Arc::new(Ui::new(
        cwd,
        resume,
        resume_picker,
        yolo,
        model,
        initial,
        allow_host,
        flags,
    )?);
    // Gh #160: the guard owns this session's temp dir (resolved and
    // validated inside `Ui::new`); a switch resolves the next one.
    let _temp_guard = crate::SessionTempGuard(ui.temp_dir().to_path_buf());
    let options = ui.options();
    let runner = ui.turn_runner();
    let result = lca_ui::run(options, runner);
    ui.close();
    result
}

/// The prompt the host's own consent asks through when no turn is running:
/// it sends the request into the interface's session channel and waits for
/// the answer, exactly as a turn's prompt does, so the *same*
/// [`crate::net_consent::endpoint_consent`] seam works before `/model`'s
/// live discovery (gh #31 review; gh #29's rule that every live request
/// to a profile endpoint is consented first). With no channel yet - before
/// `run` starts - it denies, which is the honest answer with nobody to ask.
pub(super) struct SessionPrompt {
    slot: Arc<Mutex<Option<std::sync::mpsc::SyncSender<lca_ui::PromptRequest>>>>,
}

impl PermissionPrompt for SessionPrompt {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        let Some(sender) = self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        else {
            return Decision::Denied;
        };
        let (respond, response) = std::sync::mpsc::sync_channel(1);
        if sender
            .send(lca_ui::PromptRequest {
                action: action.display(),
                respond,
            })
            .is_err()
        {
            return Decision::Denied;
        }
        response.recv().unwrap_or(Decision::Denied)
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// The session-lifetime dialog bridge (gh #124): `SharedDialogs` routes
/// here, and each question rendezvous with the TUI loop over
/// `dialog_slot`. A dead loop answers the denied values, exactly like
/// [`SessionPrompt`] denies a dead prompt.
struct SessionDialogs {
    slot: Arc<Mutex<Option<std::sync::mpsc::SyncSender<lca_ui::DialogExchange>>>>,
}

impl SessionDialogs {
    /// Ask one question: send the exchange, wait for the answer, and map
    /// a dead loop to `denied`.
    fn ask(&mut self, dialog: lca_protocol::UiDialog) -> lca_protocol::DialogAnswer {
        // Decided up front: both dead-loop paths below answer this.
        let denied = denied_answer(&dialog);
        let Some(sender) = self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        else {
            return denied;
        };
        let (respond, response) = std::sync::mpsc::sync_channel(1);
        if sender
            .send(lca_ui::DialogExchange { dialog, respond })
            .is_err()
        {
            return denied;
        }
        response.recv().unwrap_or(denied)
    }
}

/// The denied value for one question (gh #124's headless contract,
/// shared with the dead-loop path above).
fn denied_answer(dialog: &lca_protocol::UiDialog) -> lca_protocol::DialogAnswer {
    match dialog {
        lca_protocol::UiDialog::Confirm { .. } => lca_protocol::DialogAnswer::Confirm(false),
        lca_protocol::UiDialog::Select { .. } => lca_protocol::DialogAnswer::Select(None),
        lca_protocol::UiDialog::Input { .. } => lca_protocol::DialogAnswer::Input(None),
        lca_protocol::UiDialog::Notify { .. } => lca_protocol::DialogAnswer::Notify,
    }
}

impl lca_permissions::DialogPrompt for SessionDialogs {
    fn confirm(&mut self, title: &str, message: &str) -> bool {
        matches!(
            self.ask(lca_protocol::UiDialog::Confirm {
                title: title.to_string(),
                message: message.to_string(),
            }),
            lca_protocol::DialogAnswer::Confirm(true)
        )
    }

    fn select(&mut self, title: &str, options: &[String]) -> Option<String> {
        match self.ask(lca_protocol::UiDialog::Select {
            title: title.to_string(),
            options: options.to_vec(),
        }) {
            lca_protocol::DialogAnswer::Select(choice) => choice,
            _ => None,
        }
    }

    fn input(&mut self, label: &str, placeholder: Option<&str>) -> Option<String> {
        match self.ask(lca_protocol::UiDialog::Input {
            label: label.to_string(),
            placeholder: placeholder.map(str::to_string),
        }) {
            lca_protocol::DialogAnswer::Input(text) => text,
            _ => None,
        }
    }

    fn notify(&mut self, message: &str, level: &str) {
        let _ = self.ask(lca_protocol::UiDialog::Notify {
            message: message.to_string(),
            level: level.to_string(),
        });
    }
}

/// A stashed `/login` answer: the provider, the chosen option id, and the
/// typed field values, replayed to re-run discovery after the grant (#4).
type LoginAnswer = (String, String, std::collections::BTreeMap<String, String>);

/// The live cells the footer and `/model`/`/thinking` share.
struct Cells {
    model: Arc<Mutex<ModelChoice>>,
    label: Arc<Mutex<String>>,
    context_window: Arc<Mutex<u64>>,
    thinking: Arc<Mutex<Option<String>>>,
    identity: Arc<Mutex<String>>,
}

/// Build the live cells from the resolved model (FR-UI-20, R1). `identity`
/// is the preset id when the login stored one, else the extension name (E5).
fn live_cells(
    identity: &str,
    model_id: &str,
    context_window: u32,
    thinking: Option<String>,
) -> Cells {
    Cells {
        model: Arc::new(Mutex::new(ModelChoice {
            id: model_id.to_string(),
            window: context_window,
        })),
        label: Arc::new(Mutex::new(if model_id.is_empty() {
            String::new()
        } else {
            format!("{identity}/{model_id}")
        })),
        context_window: Arc::new(Mutex::new(u64::from(context_window))),
        thinking: Arc::new(Mutex::new(thinking)),
        identity: Arc::new(Mutex::new(identity.to_string())),
    }
}

/// Adopt a finished login's identity into the live cell (E5, the cycle-6
/// drive's regression). The login updated only the label, so a later
/// `/model` switch read the stale startup identity and the footer reverted
/// from the login preset to the extension name. Named so the
/// mid-session-login regression is testable without a live provider.
fn adopt_login_identity(identity_cell: &Arc<Mutex<String>>, identity: &str) {
    *identity_cell
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = identity.to_string();
}

/// The active model's context window, or 0 when the provider does not
/// publish one (the threshold check then skips, FR-SESS-4).
fn model_context_window(provider: &dyn Provider, model_id: &str) -> u32 {
    provider
        .list_models()
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| model.context_window)
        .unwrap_or(0)
}

/// Swap the live configuration scope (gh #204): the `/scoped-models`
/// checklist writes through this cell, so the model cycle reads the
/// live scope without a restart. `/reload` swaps the whole cell.
pub(super) fn apply_models_scope(config: &Mutex<Config>, ids: Vec<String>) {
    crate::lock(config).set_models_enabled(ids);
}

/// The agent config for the resolved model and registry.
#[allow(clippy::too_many_arguments)]
fn agent_config_for(
    config: &Config,
    provider_name: &str,
    model_id: &str,
    context_window: u32,
    image_policy: lca_tools::ImagePolicy,
    registry: &Arc<ExtensionRegistry>,
    completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>>,
    cwd: &Path,
    flags: &crate::CliFlags,
    trusted: bool,
) -> Result<AgentConfig, String> {
    let system_prompt = crate::prompt::agent_system_prompt(cwd, model_id, flags, trusted)?;
    Ok(AgentConfig {
        provider: provider_name.to_string(),
        model: model_id.to_string(),
        image_policy,
        retry_limit: config.provider_retry_limit() as u32,
        max_iterations: config.tool_max_iterations() as u32,
        extensions: registry.clone(),
        compaction_threshold: config.compaction_threshold(),
        compaction_enabled: config.compaction_enabled(),
        compaction_reserve_tokens: config.compaction_reserve_tokens(),
        compaction_keep_recent_tokens: config.compaction_keep_recent_tokens(),
        model_context_window: context_window,
        completion_backend,
        system_prompt,
        skills_roots: crate::skills_roots(cwd),
        skills_inject_matched: config.skills_inject_matched(),
        edit_requires_read: config.tool_edit_requires_read(),
        ..AgentConfig::default()
    })
}

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
        allow_host: &[String],
        flags: &crate::CliFlags,
    ) -> anyhow::Result<Ui> {
        let Opened {
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
        } = open(cwd, resume, yolo, allow_host, flags)?;

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
        let temp_dir = crate::ensure_session_temp(&data, &session_id)?;
        let mut registry = load_registry(
            cwd,
            &config,
            shared_prompt.clone(),
            shared_dialogs.clone(),
            &grants,
            &store,
            &current_session,
            &temp_dir,
        );

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
        let endpoint_needs =
            crate::provider_needs::provider_needs(&registry, &data, &provider_name);
        if provider_is_ready
            && let Some(host) = crate::ungranted_host(
                &grants,
                cwd,
                crate::provider_needs::provider_ad_hoc_host(
                    &data,
                    &provider_name,
                    endpoint_needs.as_ref(),
                ),
            )
        {
            initial_head.push(format!(
                "note: the endpoint {host} is not granted for this project - the first \
                 request will ask you to approve it; a script passes --allow-host {host}"
            ));
        }
        // Gh #112: malformed screen-mode persistence falls back and
        // says so on the transcript head (never a silent substring).
        if let (_, Some(warning)) =
            crate::tui::hooks::initial_screen_mode(&data, config.ui_fullscreen())
        {
            initial_head.push(warning);
        }
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
        let provider_backend = register_compaction(
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

        Ok(Ui {
            cwd: cwd.to_path_buf(),
            data,
            temp_dir: temp_dir.clone(),
            flags: flags.clone(),
            store,
            config: Mutex::new(config),
            grants,
            current_session,
            resume_picker,
            live: Arc::new(Mutex::new(LiveTarget {
                name: provider_name,
                provider,
                agent_config,
            })),
            registry: Mutex::new(registry),
            settings_cell,
            model_cell: cells.model,
            label_cell: cells.label,
            identity_cell: cells.identity,
            context_window_cell: cells.context_window,
            thinking_cell: cells.thinking,
            theme_cell: Arc::new(Mutex::new(theme_setting)),
            provider_backend,
            pending_attachments: Arc::new(Mutex::new(Vec::new())),
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
            consent_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            render_regions,
            ui_events,
            initial_head,
            initial_records,
            initial_tail,
            initial_messages: initial.to_vec(),
            update_notice,
            login_pending: Arc::new(Mutex::new(None)),
            compact_state: Arc::new(Mutex::new(lca_ui::CompactState::Idle)),
            login_handle: Arc::new(Mutex::new(None)),
            login_manual_offered: Arc::new(Mutex::new(false)),
            login_url_shown: Arc::new(Mutex::new(None)),
            login_wait_since: Arc::new(Mutex::new(None)),
            login_cancelled: Arc::new(Mutex::new(false)),
        })
    }

    /// This session's temp dir (gh #160), resolved and validated at
    /// construction; the guard in `run` owns it.
    fn temp_dir(&self) -> &std::path::Path {
        &self.temp_dir
    }

    /// The close-out: let hooks flush state, then close the session.
    fn close(&self) {
        let registry = self.registry();
        lca_core::drive_blocking(async move {
            registry.on_session_close().await;
        });
        let final_session = self
            .current_session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let _ = self.store.close(&final_session);
    }
}
/// The session-and-store setup, before the extension registry exists.
struct Opened {
    data: PathBuf,
    store: Arc<SessionStore>,
    config: Config,
    grants: Arc<Mutex<GrantStore>>,
    trusted: bool,
    current_session: Arc<Mutex<Session>>,
    session_id: String,
    provider_name: String,
    initial_head: Vec<String>,
    initial_records: Vec<lca_protocol::Record>,
    initial_tail: Vec<String>,
    update_notice: Arc<std::sync::OnceLock<String>>,
}

/// Open the store, the grant store, the merged configuration, and the
/// session to show (resumed or fresh).
fn open(
    cwd: &Path,
    resume: Option<&str>,
    yolo: bool,
    allow_host: &[String],
    flags: &crate::CliFlags,
) -> anyhow::Result<Opened> {
    let data = crate::data_dir();
    let store = Arc::new(SessionStore::new(data.clone()));
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&data.join("grants.json"))
            .map_err(|err| anyhow::anyhow!("cannot open the grant store: {err}"))?,
    ));
    let trusted = lock(&grants).is_trusted(cwd);
    let config = crate::load_config_flags(cwd, &lock(&grants), false, yolo, flags)?;
    // Today's update check, if enabled and due: stamped, then spawned - the
    // startup path never waits on it (FR-CFG-6), and the status line picks
    // the finding up from the shared cell once it lands.
    let update_notice = Arc::new(std::sync::OnceLock::new());
    crate::update::spawn(config.update_check(false), Some(update_notice.clone()));
    let session = resolve_session(&store, cwd, resume)?;
    let current_session = Arc::new(Mutex::new(session.clone()));
    let provider_name = config.provider().to_string();
    let (mut initial_head, initial_records) = initial_view(&store, &session);
    // ADR-0042: the mode applies to the shared grant store, so the model's
    // tool calls and an extension's `process` calls answer alike. The banner
    // leads the transcript: hands-free must never mean invisible.
    if let Some(banner) = crate::apply_permission_mode(&config, &mut lock(&grants)) {
        initial_head.insert(0, banner.to_string());
    }
    // `--allow-host`: a one-run grant, attached to the session set before
    // any turn runs and recorded once (gh #29, QA-004).
    crate::net_consent::attach_allow_hosts(&grants, cwd, allow_host, &store, &session)
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    Ok(Opened {
        data,
        store,
        config,
        grants,
        trusted,
        current_session,
        session_id: session.id().to_string(),
        provider_name,
        initial_head,
        initial_records,
        initial_tail: Vec::new(),
        update_notice,
    })
}

/// Resolve the session to open: the resumed one, or a fresh one.
fn resolve_session(
    store: &SessionStore,
    cwd: &Path,
    resume: Option<&str>,
) -> anyhow::Result<Session> {
    match resume {
        // Gh #110: the reference may be a session-directory path.
        Some(id) => Ok(store
            .session_ref(cwd, id)
            .map_err(|err| anyhow::anyhow!("{err}"))?),
        None => store
            .create_session(cwd, lca_session::DEFAULT_TITLE)
            .map_err(|err| anyhow::anyhow!("cannot start a session: {err}")),
    }
}

/// The initial transcript lines: the records, the truncation/skip
/// warnings, and the ungranted-endpoint note (FR-PERM-16).
/// The resumed transcript's framing: head warnings (top of screen) and the
/// session's records, which the interface replays with the live rendering
/// (FR-UI-7) instead of the flattened lines this used to return.
fn initial_view(
    store: &SessionStore,
    session: &Session,
) -> (Vec<String>, Vec<lca_protocol::Record>) {
    let read = match store.read_with(session, lca_session::ViewMode::Display) {
        Ok(read) => read,
        Err(err) => {
            return (
                vec![format!("error: cannot read the session: {err}")],
                Vec::new(),
            );
        }
    };
    let mut head: Vec<String> = Vec::new();
    // A session that loaded with a truncation or a skipped line must say so:
    // a short or empty transcript with no explanation reads as data loss.
    if read.truncated {
        head.push(
            "warning: the session log was truncated; only the records that loaded are shown"
                .to_string(),
        );
    } else if read.skipped_unknown > 0 {
        head.push(format!(
            "warning: {} record(s) were skipped (unknown type or version)",
            read.skipped_unknown
        ));
    }
    (head, read.records)
}

/// Load every extension: installed first (an installed copy shadows the
/// bundled one), then the native-linked first-party set, then the bundled
/// provider (ADR-0013).
#[allow(clippy::too_many_arguments)] // one more explicit than a regroup: every arg is used once, at one call site.
fn load_registry(
    cwd: &Path,
    config: &Config,
    shared_prompt: SharedPrompt,
    shared_dialogs: lca_permissions::SharedDialogs,
    grants: &Arc<Mutex<GrantStore>>,
    store: &Arc<SessionStore>,
    session_cell: &Arc<Mutex<Session>>,
    temp: &Path,
) -> ExtensionRegistry {
    // The one assembly, shared with headless mode and `--list-models`
    // (gh #8): only the stats source differs, and a session's own reads
    // the session this interface is showing.
    let stats_store = store.clone();
    let stats_session = session_cell.clone();
    crate::registry::assemble(
        cwd,
        config,
        shared_prompt,
        shared_dialogs,
        grants,
        Arc::new(move || {
            let session = stats_session
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            session_stats(&stats_store, &session)
        }),
        temp,
    )
}

/// Register the bundled compaction strategy and return its backend.
#[cfg(feature = "bundled-compaction-default")]
#[allow(clippy::too_many_arguments)] // one more explicit than a regroup: every arg is used once, at one call site.
fn register_compaction(
    registry: &mut ExtensionRegistry,
    provider: &Arc<dyn Provider>,
    model_id: &str,
    session_id: &str,
    cwd: &Path,
    shared_prompt: SharedPrompt,
    grants: &Arc<Mutex<GrantStore>>,
    temp: &Path,
) -> Option<Arc<lca_core::ext_provider::ProviderBackend>> {
    let backend = Arc::new(lca_core::ext_provider::ProviderBackend::new(
        provider.clone(),
        model_id.to_string(),
        session_id.to_string(),
    ));
    let cap = crate::extension_capabilities(
        cwd,
        "compaction-default",
        compaction_default::manifest_grants(),
        shared_prompt,
        grants.clone(),
        // No `resources/` bag: a compaction strategy carries code.
        lca_tools::ResourceSource::None,
        temp,
    );
    cap.set_completion(backend.clone());
    registry.register(Arc::new(compaction_default::CompactionDefault::new(cap)));
    Some(backend)
}

/// No bundled compaction: no backend.
#[cfg(not(feature = "bundled-compaction-default"))]
fn register_compaction(
    _registry: &mut ExtensionRegistry,
    _provider: &Arc<dyn Provider>,
    _model_id: &str,
    _session_id: &str,
    _cwd: &Path,
    _shared_prompt: SharedPrompt,
    _grants: &Arc<Mutex<GrantStore>>,
    _temp: &Path,
) -> Option<Arc<lca_core::ext_provider::ProviderBackend>> {
    None
}

/// The configured model, or the first model *in the enabled scope* when
/// the provider is ready and the configuration names none (the honest
/// "no model" state otherwise): a scope narrower than the provider's list
/// must not start the session on a model its own cycle cannot reach (gh #8).
fn resolve_model_id(config: &Config, provider_is_ready: bool, provider: &dyn Provider) -> String {
    let configured = config.model().unwrap_or_default();
    if !configured.is_empty() {
        configured.to_string()
    } else if provider_is_ready {
        // An empty id counts as no model: a provider that answered a model
        // probe with something unparseable must not leave the session with
        // a blank model label and a `complete` call the extension refuses.
        crate::models::filter_enabled(provider.list_models(), config.models_enabled())
            .into_iter()
            .map(|model| model.id)
            .find(|id| !id.is_empty())
            .unwrap_or_default()
    } else {
        String::new()
    }
}

/// The extension render-region view (FR-UI-1's pull table).
fn region_renderer(registry: Arc<ExtensionRegistry>) -> Option<RegionRenderer> {
    Some(std::sync::Arc::new(
        move |region: &str| -> Vec<(String, lca_protocol::WidgetTree)> {
            registry
                .enabled()
                .filter(|handle| handle.ui_regions().iter().any(|r| r == region))
                .filter_map(|handle| {
                    handle
                        .render(region)
                        .ok()
                        .flatten()
                        .map(|tree| (handle.name().to_string(), tree))
                })
                .collect()
        },
    ) as RegionRenderer)
}

/// The extension ui-event router (FR-UI-6's only source).
fn region_interactor(registry: Arc<ExtensionRegistry>) -> Option<RegionInteractor> {
    Some(std::sync::Arc::new(
        move |region: &str,
              input: &lca_protocol::UiInput|
              -> Option<(String, lca_protocol::UiEffect)> {
            registry
                .enabled()
                .filter(|handle| handle.ui_regions().iter().any(|r| r == region))
                .find_map(|handle| {
                    handle
                        .on_ui_event(region, input)
                        .ok()
                        .map(|effect| (handle.name().to_string(), effect))
                })
        },
    ) as RegionInteractor)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: E5 - the status label names the preset identity, not the
    // extension, when the login stored one.
    #[test]
    fn live_cells_label_the_preset_identity() {
        let cells = live_cells("opencode-go", "deepseek-v4.1-flash", 0, None);
        assert_eq!(
            *cells.label.lock().unwrap(),
            "opencode-go/deepseek-v4.1-flash"
        );
    }

    // Cycle-6 drive regression: a mid-session login must move the identity
    // cell with the preset, or a later `/model` switch reads the stale
    // startup identity and the footer names the extension instead of the
    // preset. The drive reproduced this after logging in on a session that
    // started logged out.
    #[test]
    fn a_mid_session_login_adopts_the_preset_into_the_identity_cell() {
        // Startup had no stored preset, so the cell held the extension.
        let cells = live_cells("openai-compatible", "deepseek-v4.1-flash", 0, None);
        // The login then completes on the opencode-go preset.
        adopt_login_identity(&cells.identity, "opencode-go");
        assert_eq!(*cells.identity.lock().unwrap(), "opencode-go");
        // A `/model` switch keeps the preset in the footer label.
        let label = format!(
            "{}/{}",
            *cells.identity.lock().unwrap(),
            "deepseek-v4.1-flash"
        );
        assert_eq!(label, "opencode-go/deepseek-v4.1-flash");
    }

    // SRDD's interface section: the built-in slots, of which the five
    // spec-named ones live in this list alongside `/attach`, and /stats
    // arrives from the native hooks extension that holds the stats source.
    #[test]
    fn the_spec_named_builtins_are_claimed() {
        for slot in ["login", "logout", "usage", "model", "compact", "attach"] {
            assert!(
                BUILTIN_SLOTS.contains(&slot),
                "/{slot} is a built-in the interface claims"
            );
        }
    }
}
