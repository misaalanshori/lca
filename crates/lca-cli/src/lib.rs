//! The `lca` binary's logic: argument dispatch, headless mode with the
//! `--json` envelope contract from `docs/headless.md`, and the session
//! commands. Interactive mode lives in `lca-tui`.

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use lca_config::{ColorMode, Config, LoadInput, MergeSource};
use lca_core::{Agent, AgentConfig, StopReason, TurnEvent, TurnOutcome, TurnSink, TurnStatus};
use lca_permissions::{GrantStore, PermissionPrompt, ProposalDiff};
use lca_session::{ExportOptions, SessionStore};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

/// [`version_text`], leaked to the `'static` lifetime clap's derive wants.
pub fn version_static() -> &'static str {
    Box::leak(version_text().into_boxed_str())
}

/// `lca --version`: agent version, ABI version, crate version, build target
/// (release policy, ABI policy host version reporting).
pub fn version_text() -> String {
    format!(
        "{}\nabi {}\ncrate {}\ntarget {}",
        env!("CARGO_PKG_VERSION"),
        lca_session::ABI_VERSION,
        env!("CARGO_PKG_VERSION"),
        env!("LCA_BUILD_TARGET"),
    )
}

/// The parsed command line.
#[derive(Parser, Debug)]
#[command(
    name = "lca",
    version = version_static(),
    about = "A lightweight, cross-platform, extensible coding agent for the terminal"
)]
pub struct Cli {
    // FR-CORE-3: one turn, no interface.
    /// Run a single prompt and print the reply, without the interface.
    #[arg(short = 'p', long = "prompt", value_name = "PROMPT")]
    pub prompt: Option<String>,
    // `docs/headless.md`: the JSON-lines envelope.
    /// Print one JSON object per line, for scripts.
    #[arg(long)]
    pub json: bool,
    #[command(subcommand)]
    /// A subcommand, when one is present.
    pub command: Option<Command>,
}

/// The session and configuration subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    // FR-SESS-2: list or reopen.
    /// List this project's sessions, or reopen one by id.
    Resume {
        /// The session id to reopen; omit it to list.
        id: Option<String>,
    },
    // FR-SESS-3.
    /// Copy a session up to a message into a new session.
    Fork {
        /// The session to fork from.
        session: String,
        /// The record id to fork at.
        message: String,
    },
    /// Give a session a new title.
    Rename {
        /// The session to rename.
        session: String,
        /// The new title.
        title: String,
    },
    // FR-SESS-7, `docs/session-log-format.md`.
    /// Write a session out for sharing or inspection.
    Export {
        /// The session to export.
        session: String,
        /// Include permission and extension-event records.
        #[arg(long)]
        audit: bool,
    },
    // FR-CFG-2.
    /// Show the merged configuration and where each value came from.
    Config,
    // The SRDD's command-line section, FR-DIST-*.
    /// Install, update, remove, and inspect extensions.
    Ext {
        /// What to do with extensions.
        #[command(subcommand)]
        cmd: ext::ExtCmd,
    },
}

/// `lca ext ...`: resolve, consent, store (FR-DIST-*).
pub mod ext;

/// Interactive mode, wired to `lca-tui`.
pub mod tui;

/// The daily background update check (FR-CFG-6).
pub mod update;

/// Exit codes from `docs/headless.md`.
pub mod exit {
    /// The turn completed.
    pub const OK: i32 = 0;
    /// Unexpected internal error.
    pub const INTERNAL: i32 = 1;
    /// Usage error: bad flags or arguments.
    pub const USAGE: i32 = 2;
    /// Provider error after the retry limit.
    pub const PROVIDER: i32 = 3;
    /// Permission denied: an action needed approval and headless mode
    /// cannot prompt.
    pub const PERMISSION: i32 = 4;
    /// Turn aborted: iteration limit or a rejection from an extension.
    pub const ABORTED: i32 = 5;
    /// Session error: missing, malformed, or another project's session.
    pub const SESSION: i32 = 6;
}

/// FR-PROV-6's report, shared by both front ends and by the mid-session
/// identity commands: no enabled provider answers the configured name,
/// so there is no model, and the install command is the way out.
pub(crate) fn no_model_message(provider: &str) -> String {
    format!(
        "No model is available (provider `{provider}` is not enabled). \
         Install one with `lca ext install <reference>`."
    )
}

/// Whether the configured provider looks ready to answer: a stored
/// credential for its namespace (`<data>/credentials/<name>.json`, whose
/// value is the extension identity per FR-PERM-6/7), or - for the bundled
/// `openai-compatible` provider - one of its documented environment keys.
/// When it is not ready the interface starts with no model and says how to
/// sign in, instead of offering a model whose first turn will fail with a
/// transport error.
pub(crate) fn provider_ready(name: &str, data: &Path) -> bool {
    if name == "openai-compatible"
        && ["OPENAI_API_KEY", "OPENCODE_API_KEY"]
            .iter()
            .any(|key| std::env::var(key).is_ok_and(|value| !value.is_empty()))
    {
        return true;
    }
    let Ok(text) = std::fs::read_to_string(data.join("credentials").join(format!("{name}.json")))
    else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value.as_object().is_some_and(|object| {
        object
            .values()
            .any(|value| value.as_str().is_some_and(|text| !text.is_empty()))
    })
}

/// Store one secret in a provider's credential namespace (the `/login`
/// flow): the same atomic, owner-only writer extensions use. The namespace
/// is the provider identity, never guest input (FR-PERM-6).
pub(crate) fn store_provider_secret(
    data: &Path,
    cwd: &Path,
    provider: &str,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let roots = lca_permissions::ScopeRoots {
        workspace: cwd.to_path_buf(),
        private: data.join("private"),
        home_config: config_dir(),
        temp: session_temp(),
        state_dir: data.to_path_buf(),
    };
    let capabilities = lca_tools::Capabilities::new(
        provider,
        lca_tools::CapabilityGrants {
            credentials: true,
            ..Default::default()
        },
        roots,
        Arc::new(std::sync::Mutex::new(
            lca_permissions::SharedPrompt::default(),
        )),
        open_grants(data),
        cwd.to_path_buf(),
        None,
    );
    capabilities
        .credentials_set(key, value)
        .map_err(|err| err.to_string())
}

/// The endpoint host an openai-compatible login would need an ad hoc `net`
/// grant for: the configured base URL's host when it is not the manifest's
/// fixed `api.openai.com` (FR-PERM-16, ADR-0022). `None` when the default
/// endpoint is in use.
pub(crate) fn openai_ad_hoc_host(data: &Path) -> Option<String> {
    let base = std::env::var("OPENAI_BASE_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let path = data.join("credentials").join("openai-compatible.json");
            let text = std::fs::read_to_string(path).ok()?;
            let value: serde_json::Value = serde_json::from_str(&text).ok()?;
            value
                .get("base_url")
                .and_then(|url| url.as_str())
                .map(str::to_string)
        })?;
    let rest = base.split("://").nth(1).unwrap_or(&base);
    ad_hoc_host_from_authority(rest)
}

/// The host in a URL authority, or `None` when it is the default endpoint.
pub(crate) fn ad_hoc_host_from_authority(rest: &str) -> Option<String> {
    let authority = rest.split('/').next().unwrap_or("");
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    (!host.is_empty() && host != "api.openai.com").then_some(host)
}

/// Persist an ad hoc `net` grant the user approved at login (FR-PERM-16).
/// It writes through the shared grant-store handle so the running session's
/// capability engines and the turn loop see it and cannot clobber it.
pub(crate) fn store_ad_hoc_grant(
    store: &std::sync::Arc<std::sync::Mutex<GrantStore>>,
    cwd: &Path,
    host: &str,
) -> Result<(), String> {
    let mut store = store
        .lock()
        .map_err(|_| "the grant store lock is poisoned".to_string())?;
    store
        .approve_net_pattern(cwd, host)
        .map_err(|err| err.to_string())
}

/// FR-PROV-9's disable knob (FR-PERM-19's storage): every handle the
/// grant store has disabled for this project leaves the registry.
pub(crate) fn apply_enablement(
    registry: &mut lca_core::ExtensionRegistry,
    disabled_here: impl Fn(&str) -> bool,
) {
    for name in registry.registered_names() {
        if disabled_here(&name) {
            registry.set_enabled(&name, false);
        }
    }
}

/// The capability environment for a bundled extension: the platform's scope
/// roots, the shared grant store, and the caller's prompt. Every engine and
/// the turn loop share one `Arc<Mutex<GrantStore>>` so a grant written by
/// one path is visible to (and never clobbered by) another.
/// Open (or create) the process grant store, or start fail-closed (an empty
/// store grants nothing) if the file is unreadable: a bad store must not
/// abort a session with a panic.
fn open_grants(data: &Path) -> std::sync::Arc<std::sync::Mutex<GrantStore>> {
    let store = match GrantStore::open(&data.join("grants.json")) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("warning: grant store unreadable ({err}); starting with no grants");
            GrantStore::empty()
        }
    };
    std::sync::Arc::new(std::sync::Mutex::new(store))
}

pub(crate) fn extension_capabilities(
    cwd: &Path,
    name: &str,
    grants: lca_tools::CapabilityGrants,
    prompt: lca_permissions::SharedPrompt,
    store: std::sync::Arc<std::sync::Mutex<GrantStore>>,
) -> std::sync::Arc<lca_tools::Capabilities> {
    use std::sync::{Arc, Mutex};

    let data = data_dir();
    let roots = lca_permissions::ScopeRoots {
        workspace: cwd.to_path_buf(),
        private: data.join("private"),
        home_config: config_dir(),
        temp: session_temp(),
        state_dir: data.clone(),
    };
    Arc::new(lca_tools::Capabilities::new(
        name,
        grants,
        roots,
        Arc::new(Mutex::new(prompt)),
        store,
        data,
        None,
    ))
}

#[cfg(feature = "bundled-openai-compat")]
pub(crate) fn openai_capabilities(
    cwd: &Path,
    prompt: lca_permissions::SharedPrompt,
    store: std::sync::Arc<std::sync::Mutex<GrantStore>>,
) -> std::sync::Arc<lca_tools::Capabilities> {
    let mut grants = openai_compatible::manifest_grants();
    grants.adhoc_net = store
        .lock()
        .map(|store| {
            store
                .net_patterns(cwd)
                .iter()
                .filter_map(|pattern| lca_permissions::parse_net_pattern(pattern).ok())
                .collect()
        })
        .unwrap_or_default();
    extension_capabilities(cwd, "openai-compatible", grants, prompt, store)
}

/// The user data directory for sessions, grants, and state.
pub fn data_dir() -> PathBuf {
    lca_session::default_data_dir()
}

/// The per-session temporary directory the `temp` scope resolves to (FR-PERM
/// via the capability catalog: a per-session dir, removed at exit). Set once
/// when the session starts.
static SESSION_TEMP: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The current session's temp root, or the system temp dir before one is set.
pub(crate) fn session_temp() -> PathBuf {
    SESSION_TEMP
        .get()
        .cloned()
        .unwrap_or_else(std::env::temp_dir)
}

/// Create and remember the session's temp directory (`<data>/tmp/<id>`).
pub(crate) fn init_session_temp(session_id: &str) -> PathBuf {
    let path = data_dir().join("tmp").join(session_id);
    let _ = std::fs::create_dir_all(&path);
    let _ = SESSION_TEMP.set(path.clone());
    path
}

/// Remove the session's temp directory at a clean exit.
pub(crate) fn cleanup_session_temp() {
    if let Some(path) = SESSION_TEMP.get() {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Removes the session temp directory on drop, so every exit path after the
/// session starts cleans up.
pub(crate) struct SessionTempGuard;

impl Drop for SessionTempGuard {
    fn drop(&mut self) {
        cleanup_session_temp();
    }
}

/// The platform configuration directory `home-config` resolves to. This is
/// the directory an extension reads *another tool's* saved login from, not
/// the agent's own subdirectory; the state-directory exclusion keeps the
/// agent's own tree (sessions, extensions, credentials) unreadable even on
/// macOS and Windows, where it sits under this directory.
pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "linux") {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| ".".into());
                home.join(".config")
            })
    } else if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| ".".into());
        home.join("Library/Application Support")
    } else {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| ".".into())
    }
}

/// The user configuration file path for this platform.
pub fn config_file() -> PathBuf {
    config_dir().join("lca/config.toml")
}

/// Load merged configuration for `cwd`, honoring project-file trust
/// (FR-CFG-1, FR-PERM-9).
pub fn load_config(cwd: &Path, grants: &GrantStore, headless: bool) -> anyhow::Result<Config> {
    let project_file = cwd.join(".lca").join("config.toml");
    let user_file = config_file().exists().then(config_file);
    let input = LoadInput {
        flags: Default::default(),
        env: lca_config::collect_env(),
        project_file: project_file.is_file().then_some(project_file),
        trusted: grants.is_trusted(cwd),
        user_file,
        headless,
    };
    Ok(Config::load(&input)?)
}

/// Headless mode cannot prompt: it denies and remembers that approval was
/// needed, which becomes exit code 4 (`docs/headless.md`).
#[derive(Default, Clone)]
pub struct HeadlessPrompt {
    needed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl HeadlessPrompt {
    /// Whether any action asked for approval.
    pub fn needed_approval(&self) -> bool {
        self.needed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl PermissionPrompt for HeadlessPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        self.needed.store(true, std::sync::atomic::Ordering::SeqCst);
        lca_permissions::Decision::Denied
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// Maps a turn outcome onto the documented exit codes.
pub fn exit_code(outcome: &TurnOutcome, needed_approval: bool, error_class: Option<&str>) -> i32 {
    if needed_approval {
        return exit::PERMISSION;
    }
    match outcome.status {
        TurnStatus::Ok => exit::OK,
        TurnStatus::Error => match outcome.stop_reason {
            StopReason::IterationLimit | StopReason::Cancelled => exit::ABORTED,
            StopReason::Error => match error_class {
                Some("transport") | Some("auth") | Some("invalid") => exit::PROVIDER,
                Some("internal") => exit::INTERNAL,
                Some(_) => exit::ABORTED,
                None => exit::INTERNAL,
            },
            StopReason::Stop => exit::OK,
        },
    }
}

/// The sink headless mode renders through: `--json` envelopes or plain
/// final text (`docs/headless.md`).
pub struct HeadlessSink {
    /// Emit one JSON object per line.
    pub json: bool,
    /// The session loaded with a truncation warning.
    pub session_truncated: bool,
    plain: String,
    last_error_class: Option<String>,
    out: std::io::Stdout,
}

impl HeadlessSink {
    /// A sink writing to stdout.
    pub fn new(json: bool, session_truncated: bool) -> HeadlessSink {
        HeadlessSink {
            json,
            session_truncated,
            plain: String::new(),
            last_error_class: None,
            out: std::io::stdout(),
        }
    }

    /// The class of the last error event, for exit-code mapping.
    pub fn error_class(&self) -> Option<&str> {
        self.last_error_class.as_deref()
    }

    fn emit(&mut self, line: serde_json::Value) {
        let _ = writeln!(self.out, "{line}");
    }
}

impl TurnSink for HeadlessSink {
    fn on_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::AssistantText(text) => {
                if self.json {
                    self.emit(serde_json::json!({ "type": "text", "content": text }));
                } else {
                    self.plain.push_str(&text);
                }
            }
            TurnEvent::ToolStarted(call) if self.json => {
                self.emit(serde_json::json!({
                    "type": "tool-call",
                    "id": call.call_id,
                    "call_id": call.call_id,
                    "name": call.name,
                    "arguments": call.arguments,
                }));
            }
            TurnEvent::ToolFinished(result) if self.json => {
                self.emit(serde_json::json!({
                    "type": "tool-result",
                    "id": result.call_id,
                    "call_id": result.call_id,
                    "status": result.status,
                    "content": result.content,
                    "truncated": result.truncated,
                }));
            }
            TurnEvent::Usage(usage) if self.json => {
                self.emit(serde_json::json!({
                    "type": "usage",
                    "input": usage.input,
                    "output": usage.output,
                    "cache_read": usage.cache_read,
                    "cache_write": usage.cache_write,
                    "cache_write_1h": usage.cache_write_1h,
                    "cost": usage.cost,
                }));
            }
            TurnEvent::RetryScheduled {
                attempt,
                max,
                error,
                ..
            } if self.json => {
                self.emit(serde_json::json!({
                    "type": "error",
                    "message": format!("retry {attempt}/{max}: {error}"),
                    "class": "transport",
                    "retryable": true,
                }));
            }
            TurnEvent::Error {
                message,
                class,
                retryable,
            } => {
                self.last_error_class = Some(class.clone());
                if self.json {
                    self.emit(serde_json::json!({
                        "type": "error",
                        "message": message,
                        "class": class,
                        "retryable": retryable,
                    }));
                } else {
                    eprintln!("error: {message}");
                }
            }
            TurnEvent::TurnEnded {
                status,
                stop_reason,
            } => {
                if self.json {
                    self.emit(serde_json::json!({
                        "type": "turn-end",
                        "status": match status { TurnStatus::Ok => "ok", TurnStatus::Error => "error" },
                        "stop_reason": stop_reason_name(stop_reason),
                        "truncated_session": self.session_truncated,
                    }));
                } else if self.session_truncated {
                    eprintln!("warning: session loaded with a truncation warning");
                }
            }
            _ => {}
        }
        let _ = self.out.flush();
    }
}

fn stop_reason_name(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Stop => "stop",
        StopReason::IterationLimit => "iteration-limit",
        StopReason::Cancelled => "cancelled",
        StopReason::Error => "error",
    }
}

/// Run one headless turn (FR-CORE-3) and return the exit code.
pub async fn headless(prompt: &str, json: bool, cwd: &Path) -> i32 {
    let data = data_dir();
    let store = SessionStore::new(data.clone());
    let grants = match GrantStore::open(&data.join("grants.json")) {
        Ok(grants) => std::sync::Arc::new(std::sync::Mutex::new(grants)),
        Err(err) => {
            eprintln!("error: cannot open the grant store: {err}");
            return exit::INTERNAL;
        }
    };
    let config = match load_config(cwd, &grants.lock().expect("grant store"), true) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    };
    // Headless makes no request unless the option was switched on
    // (the config default is off); when it was, there is no status
    // line to report through, so stderr carries the notice.
    crate::update::spawn(config.update_check(true), None);
    let provider_name = config.provider().to_string();
    let title: String = prompt.chars().take(60).collect();
    let session =
        match store.create_session(cwd, if title.is_empty() { "headless" } else { &title }) {
            Ok(session) => session,
            Err(err) => {
                eprintln!("error: cannot start a session: {err}");
                return exit::INTERNAL;
            }
        };
    let session_truncated = store
        .read(&session)
        .map(|read| read.truncated)
        .unwrap_or(false);
    let _temp_guard = crate::SessionTempGuard;
    crate::init_session_temp(session.id());

    let mut tools = ToolExecutor::new(
        std::sync::Arc::new(NativeOps),
        cwd.to_path_buf(),
        cwd.to_path_buf(),
        config.tool_result_limit_bytes() as usize,
        std::time::Duration::from_secs(config.tool_timeout_seconds()),
    );
    let mut prompt_impl = HeadlessPrompt::default();
    // Extension-originated commands route through the same denying prompt, so a
    // headless approval need still surfaces as exit code 4.
    let shared_prompt = lca_permissions::SharedPrompt::default();
    shared_prompt.set(std::sync::Arc::new(std::sync::Mutex::new(
        prompt_impl.clone(),
    )));
    // Hooks apply headless too: register the first-party native set with
    // a stats source over this session (ADR-0013).
    let stats_store = store.clone();
    let stats_session = session.clone();
    let mut registry = lca_core::ExtensionRegistry::new();
    // Installed extensions first (FR-DIST-8's digest load; an installed
    // copy shadows the bundled one of the same name).
    crate::ext::load_installed(
        &mut registry,
        cwd,
        config.extensions_log_limit_bytes() as usize,
        shared_prompt.clone(),
    );
    for handle in lca_ext_native::default_native_extensions(Arc::new(move || {
        crate::tui::session_stats(&stats_store, &stats_session)
    })) {
        registry.register(handle);
    }
    #[cfg(feature = "bundled-openai-compat")]
    registry.register(Arc::new(openai_compatible::OpenAiCompat::new(
        openai_capabilities(cwd, shared_prompt.clone(), grants.clone()),
    )));
    // The grant store's disable wins before the provider resolves
    // (FR-PROV-9/FR-PERM-19); applied again after the two
    // completion-dependent handles register below.
    apply_enablement(&mut registry, |name| {
        grants
            .lock()
            .expect("grant store")
            .extension_enabled(cwd, name)
            == Some(false)
    });
    // FR-PROV-6: the configured provider must resolve to an enabled
    // handle; zero providers is an ordinary, reportable state. Resolved
    // before the completion-dependent handles register (they need it).
    let provider: Arc<dyn lca_provider::Provider> = match registry.provider(&provider_name) {
        Some(handle) => Arc::new(lca_core::ExtensionProvider::new(handle.clone())),
        None => {
            eprintln!("{}", no_model_message(&provider_name));
            return exit::USAGE;
        }
    };
    let model_id = {
        let configured = config.model().unwrap_or_default();
        if configured.is_empty() {
            provider
                .list_models()
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_else(|| provider_name.clone())
        } else {
            configured.to_string()
        }
    };
    #[cfg(feature = "bundled-compaction-default")]
    let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> = {
        let backend = Arc::new(lca_core::ext_provider::ProviderBackend::new(
            provider.clone(),
            model_id.clone(),
            session.id().to_string(),
        ));
        let cap = extension_capabilities(
            cwd,
            "compaction-default",
            compaction_default::manifest_grants(),
            shared_prompt.clone(),
            grants.clone(),
        );
        cap.set_completion(backend.clone());
        registry.register(Arc::new(compaction_default::CompactionDefault::new(cap)));
        Some(backend)
    };
    #[cfg(not(feature = "bundled-compaction-default"))]
    let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> = None;
    #[cfg(feature = "bundled-skills")]
    registry.register(Arc::new(skills::Skills::new(extension_capabilities(
        cwd,
        "skills",
        skills::manifest_grants(),
        shared_prompt.clone(),
        grants.clone(),
    ))));
    apply_enablement(&mut registry, |name| {
        grants
            .lock()
            .expect("grant store")
            .extension_enabled(cwd, name)
            == Some(false)
    });
    let agent_config = AgentConfig {
        provider: provider_name.clone(),
        model: model_id.clone(),
        retry_limit: config.provider_retry_limit() as u32,
        max_iterations: config.tool_max_iterations() as u32,
        extensions: Arc::new(registry),
        compaction_threshold: config.compaction_threshold(),
        model_context_window: provider
            .list_models()
            .iter()
            .find(|model| model.id == model_id)
            .map(|model| model.context_window)
            .unwrap_or(0),
        completion_backend,
        ..AgentConfig::default()
    };
    let proposals = if grants.lock().expect("grant store").is_trusted(cwd) {
        Some(config.permissions_proposals().clone())
    } else {
        None
    };
    let mut sink = HeadlessSink::new(json, session_truncated);
    let close_registry = agent_config.extensions.clone();
    let outcome = {
        let mut agent = Agent::new(
            &store,
            &session,
            provider.as_ref(),
            &mut tools,
            grants.clone(),
            &mut prompt_impl,
            proposals.as_ref(),
            agent_config,
        );
        agent.run_turn(prompt, &mut sink, &CancelFlag::new()).await
    };
    // `session-close`: the session is about to end (SRDD hook points).
    lca_core::drive_blocking(async move {
        close_registry.on_session_close().await;
    });
    let _ = store.close(&session);
    let class = sink.error_class().map(str::to_string);
    if !json && !sink.plain.is_empty() {
        println!("{}", sink.plain);
    }
    exit_code(&outcome, prompt_impl.needed_approval(), class.as_deref())
}

/// What a parsed command line asks for. Split out so the dispatch rule
/// itself is testable without a terminal.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// One headless turn (FR-CORE-3).
    Headless {
        /// The prompt.
        prompt: String,
    },
    /// The interactive interface in the working directory (FR-CORE-2).
    Interactive {
        /// The session to resume, when the subcommand names one.
        resume: Option<String>,
    },
    /// The merged-configuration printout (FR-CFG-2).
    Config,
    /// An extension-management subcommand (FR-DIST-*).
    Ext(ext::ExtCmd),
    /// The session listing (FR-SESS-2).
    ResumeList,
    /// Fork at a message (FR-SESS-3).
    Fork {
        /// Parent session id.
        session: String,
        /// Record id to fork at.
        message: String,
    },
    /// Rename a session.
    Rename {
        /// Session id.
        session: String,
        /// New title.
        title: String,
    },
    /// Export a session (FR-SESS-7).
    Export {
        /// Session id.
        session: String,
        /// Keep audit records.
        audit: bool,
    },
}

/// Resolve a parsed command line to a route.
pub fn route(cli: &Cli) -> Route {
    match &cli.command {
        None => match &cli.prompt {
            Some(prompt) => Route::Headless {
                prompt: prompt.clone(),
            },
            None => Route::Interactive { resume: None },
        },
        Some(Command::Config) => Route::Config,
        Some(Command::Resume { id }) => match id {
            None => Route::ResumeList,
            Some(id) => Route::Interactive {
                resume: Some(id.clone()),
            },
        },
        Some(Command::Fork { session, message }) => Route::Fork {
            session: session.clone(),
            message: message.clone(),
        },
        Some(Command::Rename { session, title }) => Route::Rename {
            session: session.clone(),
            title: title.clone(),
        },
        Some(Command::Export { session, audit }) => Route::Export {
            session: session.clone(),
            audit: *audit,
        },
        Some(Command::Ext { cmd }) => Route::Ext(cmd.clone()),
    }
}

/// Dispatch a parsed command line; returns the process exit code.
pub async fn run(cli: Cli) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::INTERNAL;
        }
    };
    match route(&cli) {
        Route::Headless { prompt } => headless(&prompt, cli.json, &cwd).await,
        Route::Interactive { resume } => interactive(&cwd, resume.as_deref()),
        Route::Config => config_command(&cwd),
        Route::ResumeList => resume_list(&cwd),
        Route::Fork { session, message } => fork_command(&cwd, &session, &message),
        Route::Rename { session, title } => rename_command(&cwd, &session, &title),
        Route::Export { session, audit } => export_command(&cwd, &session, audit),
        Route::Ext(cmd) => ext::run(cmd).await,
    }
}

fn interactive(cwd: &Path, resume: Option<&str>) -> i32 {
    // Wired to `lca-tui` in this phase; kept as one seam so the headless
    // contract stays independently testable.
    match lca_tui_entry(cwd, resume) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            exit::INTERNAL
        }
    }
}

#[cfg(feature = "bundled-openai-compat")]
fn lca_tui_entry(cwd: &Path, resume: Option<&str>) -> anyhow::Result<i32> {
    crate::tui::run(cwd, resume)
}

#[cfg(not(feature = "bundled-openai-compat"))]
fn lca_tui_entry(_cwd: &Path, _resume: Option<&str>) -> anyhow::Result<i32> {
    anyhow::bail!("interactive mode requires a bundled provider feature")
}

fn config_command(cwd: &Path) -> i32 {
    let data = data_dir();
    let grants = match GrantStore::open(&data.join("grants.json")) {
        Ok(grants) => grants,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::INTERNAL;
        }
    };
    let config = match load_config(cwd, &grants, false) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    };
    let mut lines: Vec<(String, String, MergeSource)> = config
        .resolved()
        .map(|(key, value, source)| (key.to_string(), value, source))
        .collect();
    lines.sort();
    for (key, value, source) in lines {
        println!("{key} = {value}  [{source}]");
    }
    let _ = ColorMode::Auto; // documented in the table above; nothing extra to print
    0
}

fn open_store() -> Result<(SessionStore, PathBuf), i32> {
    let data = data_dir();
    Ok((SessionStore::new(data.clone()), data))
}

fn resume_list(cwd: &Path) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.list_sessions(cwd) {
        Ok(sessions) => {
            if sessions.is_empty() {
                println!("No sessions yet. Start one with `lca -p \"...\"`.");
                return exit::OK;
            }
            for session in sessions {
                println!(
                    "{}\t{}\t{} messages",
                    session.id, session.title, session.message_count
                );
            }
            exit::OK
        }
        Err(err) => {
            eprintln!("error: cannot list sessions: {err}");
            exit::SESSION
        }
    }
}

fn fork_command(cwd: &Path, id: &str, message: &str) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    let parent = match store.session(cwd, id) {
        Ok(parent) => parent,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::SESSION;
        }
    };
    match store.fork(&parent, message) {
        Ok(child) => {
            println!("{}", child.id());
            exit::OK
        }
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

fn rename_command(cwd: &Path, id: &str, title: &str) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.session(cwd, id) {
        Ok(session) => match store.rename(&session, title) {
            Ok(()) => exit::OK,
            Err(err) => {
                eprintln!("error: {err}");
                exit::SESSION
            }
        },
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

fn export_command(cwd: &Path, id: &str, audit: bool) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.session(cwd, id) {
        Ok(session) => match store.export(&session, ExportOptions { audit }) {
            Ok(path) => {
                println!("{}", path.display());
                exit::OK
            }
            Err(err) => {
                eprintln!("error: {err}");
                exit::SESSION
            }
        },
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}
