//! Interactive mode: resolve sessions, provider, and tools, then run the
//! terminal interface against the real agent loop.
//!
//! **The composition root (S1).** `run` builds one `Ui` — the wiring
//! state every closure here used to capture as a local — and then hands the
//! interface exactly three things: its options, its hooks, and its turn
//! runner. The closures are methods on `Ui` (`commands`, `hooks`, `login`),
//! so `run` is orchestration and its helpers stay under the line budget.

mod commands;
mod display;
mod hooks;
mod login;
mod runner;

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
    /// The session store.
    store: Arc<SessionStore>,
    /// The merged configuration.
    config: Config,
    /// The shared grant store (one owner: ADR-0006).
    grants: Arc<Mutex<GrantStore>>,
    /// The session the interface is showing; swappable (`/tree`, `/resume`).
    current_session: Arc<Mutex<Session>>,
    /// The active provider extension name.
    provider_name: String,
    /// The loaded extension registry.
    registry: Arc<ExtensionRegistry>,
    /// The resolved provider.
    provider: Arc<dyn Provider>,
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
    /// The agent config reused per turn.
    agent_config: AgentConfig,
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
    /// The project's proposed permission patterns, when trusted.
    proposals: Option<Proposals>,
    /// The swappable prompt slot capability engines share.
    shared_prompt: SharedPrompt,
    /// Extension render regions (FR-UI-1).
    render_regions: Option<RegionRenderer>,
    /// Extension ui events (FR-UI-6).
    ui_events: Option<RegionInteractor>,
    /// The initial transcript: warnings that frame it (head), the session's
    /// records for the rich replay (FR-UI-7), and trailing notices.
    initial_head: Vec<String>,
    initial_records: Vec<lca_protocol::Record>,
    initial_tail: Vec<String>,
    /// The background update check's finding (FR-CFG-6).
    update_notice: Arc<std::sync::OnceLock<String>>,
    /// R4: a background login/identity step's result, taken by the
    /// interface's `poll_login` hook.
    login_pending: Arc<Mutex<Option<lca_ui::LoginNext>>>,
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
pub fn run(cwd: &Path, resume: Option<&str>, yolo: bool) -> anyhow::Result<i32> {
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
    let _temp_guard = crate::SessionTempGuard;
    let ui = Arc::new(Ui::new(cwd, resume, yolo)?);
    crate::init_session_temp(&ui.session_id());
    let options = ui.options();
    let runner = ui.turn_runner();
    let result = lca_ui::run(options, runner);
    ui.close();
    result
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

/// The agent config for the resolved model and registry.
fn agent_config_for(
    config: &Config,
    provider_name: &str,
    model_id: &str,
    context_window: u32,
    registry: &Arc<ExtensionRegistry>,
    completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>>,
    cwd: &Path,
) -> AgentConfig {
    AgentConfig {
        provider: provider_name.to_string(),
        model: model_id.to_string(),
        retry_limit: config.provider_retry_limit() as u32,
        max_iterations: config.tool_max_iterations() as u32,
        extensions: registry.clone(),
        compaction_threshold: config.compaction_threshold(),
        model_context_window: context_window,
        completion_backend,
        system_prompt: lca_core::identity_prompt(model_id, std::env::consts::OS),
        skills_roots: crate::skills_roots(cwd),
        ..AgentConfig::default()
    }
}

impl Ui {
    /// Build the wiring state (S1): resolve the session, provider, and
    /// tools, then assemble the extension registry and the live cells.
    fn new(cwd: &Path, resume: Option<&str>, yolo: bool) -> anyhow::Result<Ui> {
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
        } = open(cwd, resume, yolo)?;

        // ADR-0041: the interpreter is resolved once, here, so the tool
        // description, `/settings`, and every call agree - and a configured
        // interpreter that is missing is loud at startup, never a silent
        // switch to another shell.
        let ops = crate::native_ops(&config);
        if let Some(error) = ops.error() {
            tracing::warn!(%error, "shell resolution failed");
            initial_head.push(format!("warning: {error}"));
        }
        let tools = Arc::new(Mutex::new(ToolExecutor::new(
            Arc::new(ops),
            cwd.to_path_buf(),
            cwd.to_path_buf(),
            config.tool_result_limit_bytes() as usize,
            std::time::Duration::from_secs(config.tool_timeout_seconds()),
        )));
        let proposals: Option<Proposals> = trusted.then(|| config.permissions_proposals().clone());
        // The one swappable prompt slot every capability engine shares; the
        // turn runner installs the interface's modal into it, so an
        // extension's own process/pty command asks the user exactly like a
        // model command does.
        let shared_prompt = SharedPrompt::default();

        let mut registry = load_registry(
            cwd,
            &config,
            shared_prompt.clone(),
            &grants,
            &store,
            &current_session,
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
        let model_id = resolve_model_id(&config, provider_is_ready, provider.as_ref());
        let provider_backend = register_compaction(
            &mut registry,
            &provider,
            &model_id,
            &session_id,
            cwd,
            shared_prompt.clone(),
            &grants,
        );
        let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> = provider_backend
            .clone()
            .map(|backend| backend as Arc<dyn lca_tools::CompletionBackend>);
        crate::apply_enablement(&mut registry, |name| {
            lock(&grants).extension_enabled(cwd, name) == Some(false)
        });
        let registry = Arc::new(registry);

        let context_window = model_context_window(provider.as_ref(), &model_id);
        let agent_config = agent_config_for(
            &config,
            &provider_name,
            &model_id,
            context_window,
            &registry,
            completion_backend,
            cwd,
        );

        // The ui view over the registry: only handles that declared
        // regions register here (FR-UI-1's pull table; deny-by-default
        // comes from ui_regions returning empty).
        let render_regions = region_renderer(registry.clone());
        let ui_events = region_interactor(registry.clone());

        // E5: name the login preset when one was stored, else the extension.
        let identity = crate::stored_provider_preset(&data, &provider_name)
            .unwrap_or_else(|| provider_name.clone());
        let cells = live_cells(
            &identity,
            &model_id,
            context_window,
            config.thinking().map(str::to_string),
        );

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
            store,
            config,
            grants,
            current_session,
            provider_name,
            registry,
            provider,
            settings_cell,
            model_cell: cells.model,
            label_cell: cells.label,
            identity_cell: cells.identity,
            context_window_cell: cells.context_window,
            thinking_cell: cells.thinking,
            theme_cell: Arc::new(Mutex::new(theme_setting)),
            provider_backend,
            agent_config,
            pending_attachments: Arc::new(Mutex::new(Vec::new())),
            flow,
            login_answer: Arc::new(Mutex::new(None)),
            preset_overrides,
            tools,
            proposals,
            shared_prompt,
            render_regions,
            ui_events,
            initial_head,
            initial_records,
            initial_tail,
            update_notice,
            login_pending: Arc::new(Mutex::new(None)),
            login_handle: Arc::new(Mutex::new(None)),
            login_manual_offered: Arc::new(Mutex::new(false)),
            login_url_shown: Arc::new(Mutex::new(None)),
            login_wait_since: Arc::new(Mutex::new(None)),
            login_cancelled: Arc::new(Mutex::new(false)),
        })
    }

    /// The session the interface is showing, for the temp-dir setup.
    fn session_id(&self) -> String {
        self.current_session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .id()
            .to_string()
    }

    /// The close-out: let hooks flush state, then close the session.
    fn close(&self) {
        let registry = self.registry.clone();
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
fn open(cwd: &Path, resume: Option<&str>, yolo: bool) -> anyhow::Result<Opened> {
    let data = crate::data_dir();
    let store = Arc::new(SessionStore::new(data.clone()));
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&data.join("grants.json"))
            .map_err(|err| anyhow::anyhow!("cannot open the grant store: {err}"))?,
    ));
    let trusted = lock(&grants).is_trusted(cwd);
    let config = crate::load_config(cwd, &lock(&grants), false, yolo)?;
    // Today's update check, if enabled and due: stamped, then spawned - the
    // startup path never waits on it (FR-CFG-6), and the status line picks
    // the finding up from the shared cell once it lands.
    let update_notice = Arc::new(std::sync::OnceLock::new());
    crate::update::spawn(config.update_check(false), Some(update_notice.clone()));
    let session = resolve_session(&store, cwd, resume)?;
    let current_session = Arc::new(Mutex::new(session.clone()));
    let provider_name = config.provider().to_string();
    let (mut initial_head, initial_records) =
        initial_view(&store, &session, &grants, cwd, &data, &provider_name);
    // ADR-0042: the mode applies to the shared grant store, so the model's
    // tool calls and an extension's `process` calls answer alike. The banner
    // leads the transcript: hands-free must never mean invisible.
    if let Some(banner) = crate::apply_permission_mode(&config, &mut lock(&grants)) {
        initial_head.insert(0, banner.to_string());
    }
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
        Some(id) => Ok(store
            .session(cwd, id)
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
    grants: &Arc<Mutex<GrantStore>>,
    cwd: &Path,
    data: &Path,
    provider_name: &str,
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
    // The configured endpoint can be outside the provider's manifest hosts;
    // without its ad hoc grant every turn fails with a permission denial. Say
    // so up front and name the one command that fixes it (FR-PERM-16). This is
    // the env-var path, which never runs `/login` on its own.
    if crate::provider_ready(provider_name, data)
        && let Some(host) = crate::ungranted_host(grants, cwd, crate::openai_ad_hoc_host(data))
    {
        head.push(format!(
            "note: the endpoint {host} is not granted for this project - run /login to approve it"
        ));
    }
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
fn load_registry(
    cwd: &Path,
    config: &Config,
    shared_prompt: SharedPrompt,
    grants: &Arc<Mutex<GrantStore>>,
    store: &Arc<SessionStore>,
    session_cell: &Arc<Mutex<Session>>,
) -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    crate::ext::load_installed(
        &mut registry,
        cwd,
        config.extensions_log_limit_bytes() as usize,
        shared_prompt.clone(),
    );
    let stats_store = store.clone();
    let stats_session = session_cell.clone();
    for handle in lca_ext_native::default_native_extensions(Arc::new(move || {
        let session = stats_session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        session_stats(&stats_store, &session)
    })) {
        registry.register(handle);
    }
    #[cfg(feature = "bundled-openai-compat")]
    registry.register(Arc::new(openai_compatible::OpenAiCompat::new(
        crate::openai_capabilities(cwd, shared_prompt, grants.clone()),
    )));
    crate::apply_enablement(&mut registry, |name| {
        lock(grants).extension_enabled(cwd, name) == Some(false)
    });
    registry
}

/// Register the bundled compaction strategy and return its backend.
#[cfg(feature = "bundled-compaction-default")]
fn register_compaction(
    registry: &mut ExtensionRegistry,
    provider: &Arc<dyn Provider>,
    model_id: &str,
    session_id: &str,
    cwd: &Path,
    shared_prompt: SharedPrompt,
    grants: &Arc<Mutex<GrantStore>>,
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
) -> Option<Arc<lca_core::ext_provider::ProviderBackend>> {
    None
}

/// The configured model, or the provider's first when it is ready and the
/// configuration names none (the honest "no model" state otherwise).
fn resolve_model_id(config: &Config, provider_is_ready: bool, provider: &dyn Provider) -> String {
    let configured = config.model().unwrap_or_default();
    if !configured.is_empty() {
        configured.to_string()
    } else if provider_is_ready {
        // An empty id counts as no model: a provider that answered a model
        // probe with something unparseable must not leave the session with
        // a blank model label and a `complete` call the extension refuses.
        provider
            .list_models()
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
