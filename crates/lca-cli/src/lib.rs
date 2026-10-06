//! The `lca` binary's logic: argument dispatch, headless mode with the
//! `--json` envelope contract from `docs/headless.md`, and the session
//! commands. Interactive mode lives in `lca-tui`.
//!
//! Unsafe code is `deny`ed rather than `forbid`ed so the one documented
//! exemption below can exist: [`sigpipe`] restores `SIGPIPE`'s default
//! disposition, which is a single `signal(2)` call std has no safe wrapper
//! for (GitHub issue #19). Every `unsafe` block in this crate carries a
//! `SAFETY` note, the discipline `lca-tools` and `lca-tui` already use.
#![deny(unsafe_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use std::collections::BTreeMap;

use lca_config::{Config, LoadInput};
use lca_core::{Agent, AgentConfig, StopReason, TurnEvent, TurnOutcome, TurnSink, TurnStatus};
use lca_permissions::{GrantStore, PermissionPrompt, ProposalDiff};
use lca_session::{ExportOptions, SessionStore};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

/// Lock a mutex, recovering a poisoned guard rather than panicking.
///
/// A panic while another thread held the lock leaves it poisoned; refusing
/// to recover would take the whole agent down with a lock that is still
/// perfectly usable (S3: one poison-tolerant style everywhere).
pub(crate) fn lock<T: ?Sized>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The `shell` tool's interpreter for this configuration (ADR-0041): the
/// config's `shell.tool`/`shell.path` through the ladder. `Err` carries the
/// message to surface when a configured interpreter is missing.
pub fn resolve_shell(config: &Config) -> Result<lca_tools::Shell, String> {
    lca_tools::shell::resolve(config.shell_tool().unwrap_or("auto"), config.shell_path())
}

/// The desktop tool backend for this configuration: the resolved shell, or
/// a backend whose every call fails with the resolution error - the
/// "never a silent fallback" half of ADR-0041.
pub fn native_ops(config: &Config) -> NativeOps {
    use lca_tools::ShellProbe as _;
    let fallback = || lca_tools::Shell::fallback(lca_tools::shell::Real.os());
    match resolve_shell(config) {
        Ok(shell) => NativeOps::new(shell),
        Err(err) => NativeOps::broken(fallback(), err),
    }
}

/// [`version_text`], leaked to the `'static` lifetime clap's derive wants.
pub fn version_static() -> &'static str {
    Box::leak(version_text().into_boxed_str())
}

/// `lca --version`: agent version, ABI version, crate version, build target
/// (release policy, ABI policy host version reporting).
///
/// The first line carries the *product* version: the crate version on the
/// stable line, `X.Y.Z.b<sha7>` on an unstable build (ADR-0043). The
/// `crate` line keeps the crate version, so the other three lines hold
/// their shape on both lines.
pub fn version_text() -> String {
    format!(
        "{}\nabi {}\ncrate {}\ntarget {}",
        env!("PRODUCT_VERSION"),
        lca_session::ABI_VERSION,
        env!("CARGO_PKG_VERSION"),
        env!("LCA_BUILD_TARGET"),
    )
}

/// Split a product version into its `X.Y.Z` base and its optional
/// `.b<sha7>` unstable suffix (ADR-0043's scheme). `--version`'s shape
/// test runs it over both forms.
pub fn split_product_version(product: &str) -> (&str, Option<&str>) {
    match product.split_once(".b") {
        Some((base, sha)) => (base, Some(sha)),
        None => (product, None),
    }
}
/// The parsed command line: the clap types and their documentation.
///
/// Split out for the file ceiling (gate 11); everything is re-exported
/// below, so `lca_cli::Cli` and `crate::Cli` are unchanged.
mod cli_args;
pub use cli_args::{Cli, CliFlags, Command, SessionCmd};

/// `lca ext ...`: resolve, consent, store (FR-DIST-*).
pub mod diagnostics;
pub use diagnostics::{init_diagnostics, init_diagnostics_with_dir, rotate_log_if_oversized};
pub mod ext;
mod headless;
mod models;
mod persist;
pub(crate) mod prompt;

pub use persist::persist_setting;
mod registry;
mod session_cmds;
/// Restoring `SIGPIPE`'s default disposition (GitHub issue #19).
pub mod sigpipe;

pub use headless::{HeadlessSink, exit_code, headless};
use session_cmds::*;
/// The `/login` picker flow (ADR-0033, `api-key-login-plan.md` D1): the
/// state machine, with no I/O of its own.
pub mod login;

/// The request-path consent for an endpoint host the manifest does not
/// cover, and this run's `--allow-host` grant (gh #29, QA-004).
pub mod net_consent;

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
///
/// The message names the escape route explicitly: disabling the only
/// provider leaves `lca` unable to start at all, so "install one" is not
/// the only answer - re-enabling is the common one.
pub(crate) fn no_model_message(provider: &str) -> String {
    format!(
        "No model is available: no enabled provider answers `{provider}`. \
         Check the name (`lca ext list`), re-enable one that is disabled \
         (`lca ext enable <name>`), or install a provider with \
         `lca ext install <reference>`."
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

/// Why a secret write or an ad hoc grant write failed, typed for the
/// login flow rather than collapsed to a string at the seam.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SecretError {
    /// The credential store refused the write.
    #[error("credentials: {0}")]
    Credentials(String),
    /// The grant store's lock is poisoned (a bug elsewhere; fail closed).
    #[error("the grant store is locked")]
    GrantStoreLocked,
    /// The grant store refused the write.
    #[error("grant store: {0}")]
    GrantStore(String),
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
    grants: &std::sync::Arc<std::sync::Mutex<GrantStore>>,
) -> Result<(), SecretError> {
    secret_capabilities(data, cwd, provider, grants)
        .credentials_set(key, value)
        .map_err(|err| SecretError::Credentials(err.to_string()))
}

/// The capability engine a credential is written through: a credentials-
/// only grant, the platform scope roots, and `grants` - the session's own
/// handle, passed through untouched, so storing a secret cannot clobber a
/// grant and a grant cannot clobber a secret (gh #29 review: exactly one
/// `GrantStore` instance manages `grants.json` in a running process).
/// Extracted so the guard can see which handle it carries.
fn secret_capabilities(
    data: &Path,
    cwd: &Path,
    provider: &str,
    grants: &std::sync::Arc<std::sync::Mutex<GrantStore>>,
) -> lca_tools::Capabilities {
    let roots = lca_permissions::ScopeRoots {
        workspace: cwd.to_path_buf(),
        private: data.join("private"),
        home_config: config_dir(),
        temp: session_temp(),
        state_dir: data.to_path_buf(),
    };
    lca_tools::Capabilities::new(
        provider,
        lca_tools::CapabilityGrants {
            credentials: true,
            ..Default::default()
        },
        roots,
        Arc::new(std::sync::Mutex::new(
            lca_permissions::SharedPrompt::default(),
        )),
        grants.clone(),
        cwd.to_path_buf(),
        None,
    )
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

/// The preset id a provider's login last stored (E5), so the footer names
/// `opencode-go` rather than the extension. `None` for a provider set up
/// directly (env vars, a custom endpoint) that never chose a preset.
pub(crate) fn stored_provider_preset(data: &Path, provider: &str) -> Option<String> {
    let path = data.join("credentials").join(format!("{provider}.json"));
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("preset")
        .and_then(|preset| preset.as_str())
        .filter(|preset| !preset.is_empty())
        .map(str::to_string)
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

/// The endpoint host that still needs an ad hoc `net` grant for this project
/// (FR-PERM-16). `None` when none is needed: no non-default endpoint is
/// configured, or the host is already granted. A poisoned store reads as
/// granted so a broken lock never nags.
pub(crate) fn ungranted_host(
    store: &std::sync::Mutex<GrantStore>,
    cwd: &Path,
    host: Option<String>,
) -> Option<String> {
    let host = host?;
    let covered = store
        .lock()
        .map(|store| {
            store
                .net_patterns(cwd)
                .iter()
                .any(|pattern| pattern == &host)
        })
        .unwrap_or(true);
    (!covered).then_some(host)
}

/// Persist an ad hoc `net` grant the user approved at login (FR-PERM-16).
/// It writes through the shared grant-store handle so the running session's
/// capability engines and the turn loop see it and cannot clobber it.
pub(crate) fn store_ad_hoc_grant(
    store: &std::sync::Arc<std::sync::Mutex<GrantStore>>,
    cwd: &Path,
    host: &str,
) -> Result<(), SecretError> {
    let mut store = store.lock().map_err(|_| SecretError::GrantStoreLocked)?;
    store
        .approve_net_pattern(cwd, host)
        .map_err(|err| SecretError::GrantStore(err.to_string()))
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
// #92: only bundled extensions call this; without either feature it would
// be dead code, and dead code with a warning is a gate failure.
#[cfg(any(
    feature = "bundled-openai-compat",
    feature = "bundled-compaction-default"
))]
pub(crate) fn extension_capabilities(
    cwd: &Path,
    name: &str,
    grants: lca_tools::CapabilityGrants,
    prompt: lca_permissions::SharedPrompt,
    store: std::sync::Arc<std::sync::Mutex<GrantStore>>,
    resources: lca_tools::ResourceSource,
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
    let mut engine = lca_tools::Capabilities::new(
        name,
        grants,
        roots,
        Arc::new(Mutex::new(prompt)),
        store,
        // The grant project is the workspace, not the data dir: shell
        // "allow always" patterns and mid-session ad hoc `net` grants
        // (FR-PERM-16, FR-PERM-18) are keyed by project, and the consent
        // flows write them for `cwd`. Keying the engine by the data dir
        // made those grants project-agnostic and hid every ad hoc grant
        // attached after startup.
        cwd.to_path_buf(),
        None,
    );
    // The extension's own `resources/` bag (ADR-0032): the compiled-in
    // table for a bundled extension, an installed package's directory for
    // a downloaded one. Without this `resource_read` finds nothing and the
    // picker has no presets to show.
    engine.set_resources(resources);
    Arc::new(engine)
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
    extension_capabilities(
        cwd,
        "openai-compatible",
        grants,
        prompt,
        store,
        openai_compatible::resources(),
    )
}

/// The host-side skill sources (FR-CTX-2, ADR-0030): the workspace's
/// `.lca/skills`, the user skills dir, and the extension install tree.
pub(crate) fn skills_roots(cwd: &Path) -> lca_tools::skills::SkillsRoots {
    // A package disabled for this project contributes no skills (FR-PROV-9):
    // the merge reads the install tree directly, so it must apply the same
    // enablement filter the registry does.
    let disabled = lca_permissions::GrantStore::open(&data_dir().join("grants.json"))
        .map(|store| store.disabled_extensions(cwd))
        .unwrap_or_default();
    lca_tools::skills::SkillsRoots {
        project: cwd.to_path_buf(),
        user: data_dir().join("skills"),
        extensions: data_dir().join("extensions"),
        disabled,
    }
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

/// The user configuration file: `~/.lca/config.toml` (R7). It lives with
/// the rest of the agent's data, not in the platform config directory -
/// `config_dir` stays what it always was, the `home-config` scope another
/// tool's logins resolve to.
pub fn config_file() -> PathBuf {
    data_dir().join("config.toml")
}

/// The explicitly configured `shell` default timeout (gh #40): `Some`
/// when `tool.timeout_seconds` came from a flag, env, or file, `None`
/// when it is just the built-in default. Callers add their own fallback
/// for the unset case - none where the user can cancel (interactive),
/// the historical backstop where nobody can (headless).
pub fn configured_tool_timeout(config: &Config) -> Option<std::time::Duration> {
    if config.source_of("tool.timeout_seconds") == lca_config::MergeSource::Default {
        None
    } else {
        Some(std::time::Duration::from_secs(
            config.tool_timeout_seconds(),
        ))
    }
}

/// Load merged configuration for `cwd`, honoring project-file trust
/// (FR-CFG-1, FR-PERM-9).
pub fn load_config(
    cwd: &Path,
    grants: &GrantStore,
    headless: bool,
    yolo: bool,
) -> anyhow::Result<Config> {
    load_config_flags(cwd, grants, headless, yolo, &CliFlags::default())
}

/// [`load_config`] with the command line's flag-layer values (`--models`
/// and friends, gh #8): the flags join the same precedence the `--yolo`
/// flag already rides, so `lca config` and `/settings` name the source.
pub fn load_config_flags(
    cwd: &Path,
    grants: &GrantStore,
    headless: bool,
    yolo: bool,
    cli: &CliFlags,
) -> anyhow::Result<Config> {
    let project_file = cwd.join(".lca").join("config.toml");
    let user_file = config_file().exists().then(config_file);
    let mut flags: BTreeMap<String, String> = cli.layer();
    if yolo {
        // The flag is the loudest layer (FR-CFG-1's precedence): it beats
        // a config file that says `ask`.
        flags.insert("permissions.mode".to_string(), "yolo".to_string());
    }
    let input = LoadInput {
        flags,
        env: lca_config::collect_env(),
        project_file: project_file.is_file().then_some(project_file),
        trusted: grants.is_trusted(cwd),
        user_file,
        headless,
    };
    Ok(Config::load(&input)?)
}

/// One line naming yolo mode, unmissable on purpose (ADR-0042).
pub const YOLO_BANNER: &str = "YOLO MODE: every permission prompt is auto-approved as \"always\" and recorded in the \
session log; explicit deny rules still deny. /settings shows permissions.mode.";

/// Apply `permissions.mode` to the grant store (ADR-0042) and return the
/// banner when yolo is on. One call, both surfaces: the interface and the
/// capability engines share this store, so every permission check
/// (extension `process` calls included) answers the same way.
pub fn apply_permission_mode(config: &Config, grants: &mut GrantStore) -> Option<&'static str> {
    let mode = config
        .permissions_mode()
        .and_then(lca_permissions::PermissionMode::parse)
        .unwrap_or_default();
    grants.set_permission_mode(mode);
    matches!(mode, lca_permissions::PermissionMode::Yolo).then_some(YOLO_BANNER)
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

pub use session_cmds::{Route, SessionSelector, check_flag_contradictions, route};

/// Dispatch a parsed command line; returns the process exit code.
pub async fn run(cli: Cli) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::INTERNAL;
        }
    };
    // pi 1.0.0 parity: `--provider` exists to scope the `--model`
    // lookup, so it refuses to run without one instead of quietly doing
    // nothing - exit 2 (a usage error), naming the rule.
    if cli.provider.is_some() && cli.model.is_none() {
        eprintln!("error: --provider scopes the --model lookup; pass --model <pattern> with it");
        return exit::USAGE;
    }
    // #109/#111: contradictory flags are usage errors, said out loud
    // before routing (route owns no exit code).
    if let Some(err) = check_flag_contradictions(&cli) {
        eprintln!("error: {err}");
        return exit::USAGE;
    }
    let flags = CliFlags::from_cli(&cli);
    // `--list-models` lists and exits: it outranks the session routes,
    // pi's "lists, then exits".
    if let Some(search) = cli.list_models.as_deref() {
        return list_models_command(&cwd, search, &flags, &cli.allow_host);
    }
    match route(&cli) {
        Route::Headless {
            messages,
            model,
            session,
        } => {
            headless(
                &messages,
                model.as_deref(),
                &session,
                cli.json,
                &cwd,
                &cli.attach,
                cli.yolo,
                &cli.allow_host,
                &flags,
            )
            .await
        }
        Route::Interactive {
            resume,
            model,
            initial,
        } => interactive(
            &cwd,
            resume.as_deref(),
            cli.yolo,
            model.as_deref(),
            &initial,
            &cli.allow_host,
            &flags,
        ),
        Route::Config => config_command(&cwd),
        Route::ResumeList => resume_list(&cwd),
        Route::Fork { session, message } => fork_command(&cwd, &session, &message),
        Route::Rename { session, title } => rename_command(&cwd, &session, &title),
        Route::Export { session, audit } => export_command(&cwd, &session, audit),
        Route::Gc { session } => gc_command(&cwd, &session),
        Route::Ext(cmd) => ext::run(cmd).await,
    }
}

fn interactive(
    cwd: &Path,
    resume: Option<&str>,
    yolo: bool,
    model: Option<&str>,
    initial: &[String],
    allow_host: &[String],
    flags: &CliFlags,
) -> i32 {
    // Wired to `lca-tui` in this phase; kept as one seam so the headless
    // contract stays independently testable.
    match lca_tui_entry(cwd, resume, yolo, model, initial, allow_host, flags) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            exit::INTERNAL
        }
    }
}

// #92 (QA-017): the interactive entry is provider-agnostic. Whichever
// provider the registry resolves wins; none resolves and the session opens
// in the zero-provider state (FR-PROV-9) with the `/login` recovery path,
// never a compile-time refusal. A provider-specific feature flag must not
// decide whether the interface exists.
fn lca_tui_entry(
    cwd: &Path,
    resume: Option<&str>,
    yolo: bool,
    model: Option<&str>,
    initial: &[String],
    allow_host: &[String],
    flags: &CliFlags,
) -> anyhow::Result<i32> {
    crate::tui::run(cwd, resume, yolo, model, initial, allow_host, flags)
}

#[cfg(test)]
mod tests {

    // Verifies: gh #40 (the timeout default is mode-dependent): an
    // explicit `tool.timeout_seconds` surfaces as the default, while
    // the built-in default surfaces as none - the caller adds its own
    // fallback (backstop headless, none interactive).
    #[test]
    fn an_explicit_tool_timeout_is_the_default_and_the_builtin_is_none() {
        let plain = Config::load(&LoadInput::default()).expect("load");
        assert_eq!(
            configured_tool_timeout(&plain),
            None,
            "the built-in 120s default is not an explicit setting"
        );
        let mut flags = BTreeMap::new();
        flags.insert("tool.timeout_seconds".to_string(), "60".to_string());
        let set = Config::load(&LoadInput {
            flags,
            ..Default::default()
        })
        .expect("load");
        assert_eq!(
            configured_tool_timeout(&set),
            Some(std::time::Duration::from_secs(60))
        );
    }

    // Verifies: FR-PERM-26 (ADR-0042) - `--yolo` reaches the config through the flag
    // layer (which beats a file that says ask), lands on the shared grant
    // store, and returns the banner the interface and headless print.
    #[test]
    fn the_yolo_flag_sets_the_mode_and_banners() {
        let root = lca_testkit::scratch_path("lca-yolo-flag");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");

        // A file that says ask, and a flag that says yolo: the flag wins.
        let file = root.join("config.toml");
        std::fs::write(&file, "permissions.mode = \"ask\"\n").expect("write");
        let mut flags = BTreeMap::new();
        flags.insert("permissions.mode".to_string(), "yolo".to_string());
        let config = Config::load(&lca_config::LoadInput {
            flags,
            user_file: Some(file),
            ..Default::default()
        })
        .expect("load");
        assert_eq!(config.permissions_mode(), Some("yolo"));

        let mut grants = GrantStore::open(&root.join("grants.json")).expect("open");
        let banner = apply_permission_mode(&config, &mut grants).expect("banner");
        assert!(banner.contains("YOLO MODE"), "{banner}");
        assert_eq!(
            grants.permission_mode(),
            lca_permissions::PermissionMode::Yolo
        );

        // Ask mode is quiet and leaves the store in the default.
        let quiet = Config::defaults();
        assert!(apply_permission_mode(&quiet, &mut grants).is_none());
        assert_eq!(
            grants.permission_mode(),
            lca_permissions::PermissionMode::Ask
        );
    }

    // Verifies: ADR-0041 - a configured interpreter that is missing breaks
    // the backend loudly (every call fails with the resolution message),
    // and the auto ladder always resolves to something runnable.
    #[test]
    fn a_missing_shell_path_breaks_the_backend_loudly() {
        let root = lca_testkit::scratch_path("lca-shell-missing");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let file = root.join("config.toml");
        std::fs::write(&file, "shell.path = \"/definitely/not/a/shell\"\n").expect("write");
        let config = Config::load(&lca_config::LoadInput {
            user_file: Some(file),
            ..Default::default()
        })
        .expect("load");
        let ops = native_ops(&config);
        let error = ops.error().expect("the backend is broken");
        assert!(error.contains("does not name a file"), "{error}");

        let auto = native_ops(&Config::defaults());
        assert!(auto.error().is_none(), "auto resolves on every host");
        assert!(!auto.shell().program.is_empty());
    }

    // Verifies: FR-PROV-6 - the report names every way back, including
    // re-enabling. Disabling the only provider leaves `lca` unable to start
    // until one is enabled again, so "install a provider" alone is advice
    // that cannot be followed from inside the stuck state.
    #[test]
    fn the_no_model_report_names_the_way_back() {
        let report = no_model_message("openai-compatible");
        assert!(report.contains("No model is available"), "{report}");
        assert!(
            report.contains("lca ext enable"),
            "re-enabling is the common escape: {report}"
        );
        assert!(report.contains("lca ext install"), "{report}");
    }

    use super::*;
    use lca_core::TurnSink;

    // A tool-using turn emits one assistant message before the tool and one
    // after; plain headless glued them into one run-on line.
    #[test]
    fn plain_headless_separates_multiple_assistant_messages() {
        let mut sink = HeadlessSink::new(false, false);
        sink.on_event(TurnEvent::AssistantText("I'll read the file.".into()));
        sink.on_event(TurnEvent::AssistantText("The answer is 42.".into()));
        assert_eq!(sink.plain, "I'll read the file.\n\nThe answer is 42.");
    }

    // The `/login` secret is stored through the same writer extensions use,
    // in the provider's own namespace, owner-only on Unix (B2).
    #[test]
    fn store_provider_secret_writes_the_namespace_credential() {
        let root = lca_testkit::scratch_path("lca-login");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let grants = std::sync::Arc::new(std::sync::Mutex::new(GrantStore::empty()));
        store_provider_secret(
            &root,
            &root,
            "openai-compatible",
            "api_key",
            "sk-x",
            &grants,
        )
        .expect("store the secret");
        let path = root.join("credentials").join("openai-compatible.json");
        let text = std::fs::read_to_string(&path).expect("read the credential file");
        assert!(text.contains("sk-x"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "owner-only credential file");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // The ad hoc host is the base URL's host, unless it is the manifest's
    // fixed default (FR-PERM-16: only a non-default endpoint needs the grant).
    #[test]
    fn the_ad_hoc_host_is_the_non_default_endpoint_host() {
        assert_eq!(
            ad_hoc_host_from_authority("llm.example.com:8443/v1"),
            Some("llm.example.com".to_string())
        );
        assert_eq!(
            ad_hoc_host_from_authority("user@internal.local/v1"),
            Some("internal.local".to_string())
        );
        assert_eq!(ad_hoc_host_from_authority("api.openai.com/v1"), None);
        assert_eq!(ad_hoc_host_from_authority(""), None);
    }

    // The approved grant is persisted for this project, so the next run's
    // capability environment picks it up (ADR-0022).
    #[test]
    fn the_ad_hoc_grant_is_persisted_for_the_project() {
        let root = lca_testkit::scratch_path("lca-adhoc");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        store_ad_hoc_grant(&store, &project, "llm.example.com").expect("store");
        let reread = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open");
        assert_eq!(
            reread.net_patterns(&project),
            vec!["llm.example.com".to_string()]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: FR-PERM-16 (the check behind the startup notice and
    // `/login`'s grant prompt). A non-default endpoint is ungranted until the
    // ad hoc grant is stored; no non-default endpoint never needs one. This
    // is the path an env-var key takes, which never runs the login prompt.
    #[test]
    fn a_non_default_endpoint_is_ungranted_until_the_grant_is_stored() {
        let root = lca_testkit::scratch_path("lca-ungranted");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        assert_eq!(
            ungranted_host(&store, &project, None),
            None,
            "the default endpoint needs no grant"
        );
        assert_eq!(
            ungranted_host(&store, &project, Some("opencode.ai".to_string())),
            Some("opencode.ai".to_string()),
            "a non-default endpoint needs a grant"
        );
        store_ad_hoc_grant(&store, &project, "opencode.ai").expect("grant");
        assert_eq!(
            ungranted_host(&store, &project, Some("opencode.ai".to_string())),
            None,
            "the stored grant satisfies the check"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: FR-PERM-18 (the engine's grant project is the workspace).
    // The production wiring once passed the data dir, so a grant attached for
    // the project after startup was invisible to the same engine - the ad hoc
    // `net` grant `/login` stores never took effect until a restart.
    #[cfg(feature = "bundled-openai-compat")]
    #[test]
    fn the_engine_honors_a_grant_attached_for_the_workspace() {
        let root = lca_testkit::scratch_path("lca-engine-project");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        let caps = extension_capabilities(
            &project,
            "openai-compatible",
            openai_compatible::manifest_grants(),
            lca_permissions::SharedPrompt::default(),
            store.clone(),
            openai_compatible::resources(),
        );
        store_ad_hoc_grant(&store, &project, "127.0.0.1").expect("grant");
        // Reaching the socket layer (and failing to connect) proves the grant
        // was honored; a permission refusal means it was not.
        let err = caps
            .net_request("GET", "http://127.0.0.1:9/", &[], None)
            .expect_err("nothing listens on port 9");
        assert!(
            !matches!(
                err,
                lca_protocol::CapabilityError::Permission(_)
                    | lca_protocol::CapabilityError::NotGranted(_)
            ),
            "the ad hoc grant was honored, not refused: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: FR-PERM-16 (the ad hoc net grant and an engine-persisted
    // `always` pattern share one store, so neither save clobbers the other
    // - deferred plan E1). Before the single-owner wiring, the login seam
    // opened its own handle and its save dropped the engine's pattern.
    #[test]
    fn one_grant_store_holds_the_login_grant_and_an_engine_pattern() {
        let root = lca_testkit::scratch_path("lca-shared");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        // The login flow writes the ad hoc net grant ...
        store_ad_hoc_grant(&store, &project, "llm.example.com").expect("net grant");
        // ... then the engine (or the turn loop) persists an `always`.
        store
            .lock()
            .expect("store")
            .approve_pattern(&project, "cargo test")
            .expect("pattern");
        let reread = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open");
        assert_eq!(
            reread.net_patterns(&project),
            vec!["llm.example.com".to_string()]
        );
        assert!(reread.is_allowed(
            &project,
            &lca_permissions::Action::Shell {
                command: "cargo test".to_string(),
                cwd: project.clone(),
            }
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: FR-PERM-16 (an approved ad hoc net grant is persisted and
    // project-scoped: it survives a restart, and another project never sees
    // it). Automates the manual tmux check B1 carried.
    #[test]
    fn an_ad_hoc_grant_survives_a_restart_and_stays_project_scoped() {
        let root = lca_testkit::scratch_path("lca-adhoc-persist");
        let _ = std::fs::remove_dir_all(&root);
        let project_a = root.join("a");
        let project_b = root.join("b");
        std::fs::create_dir_all(&project_a).expect("mkdir");
        std::fs::create_dir_all(&project_b).expect("mkdir");
        let path = root.join("grants.json");
        // The login flow's handle is dropped here: the store on disk is all
        // that survives a restart.
        {
            let store = std::sync::Arc::new(std::sync::Mutex::new(
                lca_permissions::GrantStore::open(&path).expect("open"),
            ));
            store_ad_hoc_grant(&store, &project_a, "llm.example.com").expect("grant");
        }

        let reloaded = lca_permissions::GrantStore::open(&path).expect("reopen");
        assert_eq!(
            reloaded.net_patterns(&project_a),
            vec!["llm.example.com".to_string()]
        );
        assert!(
            reloaded.net_patterns(&project_b).is_empty(),
            "the grant never leaks to another project"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: E5 - the login preset id is readable after a restart, so the
    // footer names `opencode-go` rather than the extension.
    #[test]
    fn stored_provider_preset_reads_the_persisted_id() {
        let root = lca_testkit::scratch_path("lca-provider-preset");
        let grants = std::sync::Arc::new(std::sync::Mutex::new(GrantStore::empty()));
        store_provider_secret(
            &root,
            &root,
            "openai-compatible",
            "preset",
            "opencode-go",
            &grants,
        )
        .expect("store");
        assert_eq!(
            stored_provider_preset(&root, "openai-compatible").as_deref(),
            Some("opencode-go")
        );
        assert_eq!(stored_provider_preset(&root, "other"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
