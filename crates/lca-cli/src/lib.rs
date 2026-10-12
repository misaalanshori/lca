//! The `lca` binary's logic: dispatch, headless mode (`docs/headless.md`),
//! and session commands. Interactive mode lives in `lca-tui`.
//!
//! Unsafe is `deny`ed (not `forbid`ed) for one exemption: [`sigpipe`]
//! restores `SIGPIPE`'s disposition via `signal(2)` (issue #19). Every
//! `unsafe` block carries a `SAFETY` note (the `lca-tools` discipline).
#![deny(unsafe_code)]

/// Lock a mutex, recovering a poisoned guard rather than panicking.
///
/// A panic while another thread held the lock leaves it poisoned; refusing
/// to recover would take the whole agent down with a lock that is still
/// perfectly usable (S3: one poison-tolerant style everywhere).
pub(crate) fn lock<T: ?Sized>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The parsed command line: the clap types and their documentation.
///
/// Split out for the file ceiling (gate 11); everything is re-exported
/// below, so `lca_cli::Cli` and `crate::Cli` are unchanged.
mod cli_args;
pub use cli_args::{AuthCmd, Cli, CliFlags, Command, SessionCmd};
pub use trust::{apply_startup_trust, load_config, load_config_flags, trust_default_fallback};

/// `lca auth ...`: pi's credential commands without the printers (gh
/// #72, #38).
mod auth;

/// `lca bug` and `/bug`: the local diagnostic bundle (gh #57).
mod bug;

/// `lca ext ...`: resolve, consent, store (FR-DIST-*).
pub mod diagnostics;
/// `lca doctor`: the diagnostics dump (gh #81).
pub mod doctor;
pub use diagnostics::{init_diagnostics, init_diagnostics_with_dir, rotate_log_if_oversized};
pub mod ext;
mod headless;
mod invoke;
mod models;
mod persist;
/// Project-local extensions (gh #138).
mod project_ext;
pub(crate) mod prompt;
/// Manifest-declared provider needs (gh #157): default hosts, the
/// credential namespace, and the login env override, with no host
/// literals about any provider.
pub(crate) mod provider_needs;
mod rpc;

pub use persist::{persist_setting, persist_setting_list};
mod registry;
mod session_cmds;
mod session_maint;
/// Restoring `SIGPIPE`'s default disposition (GitHub issue #19).
pub mod sigpipe;

pub use headless::{HeadlessSink, exit_code, headless};
pub use rpc::rpc;
/// The `/login` picker flow (ADR-0033, `api-key-login-plan.md` D1): the
/// state machine, with no I/O of its own.
pub mod login;

/// MCP management (gh #53): `mcp.json`, the manager, prompts.
pub mod mcp;

/// The request-path consent for an endpoint host the manifest does not
/// cover, and this run's `--allow-host` grant (gh #29, QA-004).
pub mod net_consent;

/// Interactive mode, wired to `lca-tui`.
mod trust;
pub mod tui;

/// The daily background update check (FR-CFG-6).
pub mod update;

mod exec;
pub mod exit;
mod paths;
mod policy;
mod run;
mod secrets;
mod version;

pub use exec::{native_ops, resolve_shell};
pub(crate) use paths::SessionTempGuard;
pub use paths::{config_dir, config_file, data_dir, ensure_session_temp, load_user_keybindings};
pub(crate) use policy::no_model_message;
pub use policy::{HeadlessPrompt, YOLO_BANNER, apply_permission_mode, configured_tool_timeout};
pub use run::run;
pub(crate) use secrets::{
    ad_hoc_host_from_authority, apply_enablement, extension_capabilities, prompt_roots,
    provider_ready, skills_roots, store_ad_hoc_grant, store_provider_secret,
    stored_provider_preset, ungranted_host,
};
// Mirrors the source gating: the bundled provider's manifest grants
// exist only with its feature (no-default builds have no provider
// worlds to describe).
#[cfg(feature = "bundled-openai-compat")]
pub(crate) use secrets::provider_capabilities;
pub use version::{split_product_version, version_static, version_text};

pub use session_cmds::{
    OutputMode, Route, SessionSelector, check_flag_contradictions, output_mode, resolve_session,
    route,
};
