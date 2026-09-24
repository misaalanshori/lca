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
    about = "A lightweight, cross-platform, WASM-extensible coding agent"
)]
pub struct Cli {
    /// Run one turn without an interactive interface (FR-CORE-3).
    #[arg(short = 'p', long = "prompt", value_name = "PROMPT")]
    pub prompt: Option<String>,
    /// Machine-readable output: one JSON object per line
    /// (`docs/headless.md`).
    #[arg(long)]
    pub json: bool,
    #[command(subcommand)]
    /// A subcommand, when one is present.
    pub command: Option<Command>,
}

/// The session and configuration subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// List this project's sessions, newest first; with an id, reopen one
    /// interactively (FR-SESS-2).
    Resume {
        /// The session to reopen.
        id: Option<String>,
    },
    /// Fork a session at a message (FR-SESS-3).
    Fork {
        /// The parent session id.
        session: String,
        /// The record id to fork at.
        message: String,
    },
    /// Rename a session.
    Rename {
        /// The session id.
        session: String,
        /// The new title.
        title: String,
    },
    /// Export a session (`docs/session-log-format.md`, FR-SESS-7).
    Export {
        /// The session id.
        session: String,
        /// Keep permission and extension-event records.
        #[arg(long)]
        audit: bool,
    },
    /// Print the merged configuration and each value's source (FR-CFG-2).
    Config,
    /// Manage extensions: install, update, remove, info, list
    /// (SRDD command-line section, FR-DIST-*).
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

/// The capability environment for the bundled native provider: the
/// manifest's grants plus this project's ad hoc `net` patterns
/// (FR-PERM-16), over the user's own scope roots.
/// ponytail: opens a second grant-store instance; nothing in the
/// capability engine writes grants yet, so there is no writer conflict,
/// and one shared owner comes back with the ad hoc attach flow (ADR-0022).
/// One capability engine for a bundled extension: the platform's scope
/// roots, this user's grant store, and a prompt that denies - only a
/// net ad hoc attach ever asks, and that flow arrives with the install
/// and login modals (Phases 5-7, ADR-0022).
/// ponytail: opens a second grant-store instance per engine; nothing in
/// these engines writes grants yet (ADR-0022's note applies to all of
/// them), one shared owner comes back with the attach flow.
pub(crate) fn extension_capabilities(
    cwd: &Path,
    name: &str,
    grants: lca_tools::CapabilityGrants,
    prompt: lca_permissions::SharedPrompt,
) -> std::sync::Arc<lca_tools::Capabilities> {
    use std::sync::{Arc, Mutex};

    let data = data_dir();
    let roots = lca_permissions::ScopeRoots {
        workspace: cwd.to_path_buf(),
        private: data.join("private"),
        home_config: config_dir(),
        temp: std::env::temp_dir(),
        state_dir: data.clone(),
    };
    let store =
        GrantStore::open(&data.join("grants.json")).expect("the grant store was read at startup");
    Arc::new(lca_tools::Capabilities::new(
        name,
        grants,
        roots,
        Arc::new(Mutex::new(prompt)),
        Arc::new(Mutex::new(store)),
        data,
        None,
    ))
}

#[cfg(feature = "bundled-openai-compat")]
pub(crate) fn openai_capabilities(
    cwd: &Path,
    prompt: lca_permissions::SharedPrompt,
) -> std::sync::Arc<lca_tools::Capabilities> {
    let data = data_dir();
    let store =
        GrantStore::open(&data.join("grants.json")).expect("the grant store was read at startup");
    let mut grants = openai_compatible::manifest_grants();
    grants.adhoc_net = store
        .net_patterns(cwd)
        .iter()
        .filter_map(|pattern| lca_permissions::parse_net_pattern(pattern).ok())
        .collect();
    extension_capabilities(cwd, "openai-compatible", grants, prompt)
}

/// The user data directory for sessions, grants, and state.
pub fn data_dir() -> PathBuf {
    lca_session::default_data_dir()
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
        Ok(grants) => grants,
        Err(err) => {
            eprintln!("error: cannot open the grant store: {err}");
            return exit::INTERNAL;
        }
    };
    let config = match load_config(cwd, &grants, true) {
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

    let mut tools = ToolExecutor::new(
        std::sync::Arc::new(NativeOps),
        cwd.to_path_buf(),
        cwd.to_path_buf(),
        config.tool_result_limit_bytes() as usize,
        std::time::Duration::from_secs(config.tool_timeout_seconds()),
    );
    let mut grants = grants;
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
        openai_capabilities(cwd, shared_prompt.clone()),
    )));
    // The grant store's disable wins before the provider resolves
    // (FR-PROV-9/FR-PERM-19); applied again after the two
    // completion-dependent handles register below.
    apply_enablement(&mut registry, |name| {
        grants.extension_enabled(cwd, name) == Some(false)
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
    ))));
    apply_enablement(&mut registry, |name| {
        grants.extension_enabled(cwd, name) == Some(false)
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
    let proposals = if grants.is_trusted(cwd) {
        Some(config.permissions_proposals().clone())
    } else {
        None
    };
    let mut sink = HeadlessSink::new(json, session_truncated);
    let outcome = {
        let mut agent = Agent::new(
            &store,
            &session,
            provider.as_ref(),
            &mut tools,
            &mut grants,
            &mut prompt_impl,
            proposals.as_ref(),
            agent_config,
        );
        agent.run_turn(prompt, &mut sink, &CancelFlag::new()).await
    };
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
