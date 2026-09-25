//! The WASM extension host: embeds Wasmtime, instantiates components,
//! wires host imports from the granted set, enforces resource limits, and
//! isolates traps (ADR-0001, ADR-0014, `docs/flows.md`).
//!
//! Capability interfaces (fs, process, pty) are always linked but live in
//! a denied state until the manifest declares them: an undeclared call
//! returns a permission error and is recorded (FR-PERM-3), while grants
//! themselves resolve through [`lca_tools::Capabilities`], the same engine
//! a native-linked extension calls (conformance identity by construction).
//!
//! Every call runs in a fresh store carrying the manifest's limits: a
//! memory ceiling, a fuel budget per call (FR-EXT-4), and epoch
//! interruption for cancellation (FR-CONC-1). Traps and limit breaches
//! disable the extension for the session while the host continues
//! (FR-EXT-3, FR-EXT-5).
//!
//! The crate needs no `unsafe`: components load through Wasmtime's safe
//! `Component::new`, and calls go through generated typed bindings.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lca_ext_abi::host::compaction::CompactionPre;
use lca_ext_abi::host::context_transform::ContextTransformPre;
use lca_ext_abi::host::provider::ProviderPre;
use lca_ext_abi::host::tool::{Tool, ToolPre};
use lca_ext_abi::host::ui::UiPre;
use lca_ext_abi::{DeliveryMode, World};
use lca_permissions::{
    GrantStore, OAuthSettings, PermissionPrompt, Proposals, ScopeGrant, ScopeRoots,
};
use lca_protocol::{
    CapabilityError, CommandEffect, CompletionRequest, DispatchError, EventSink, HookAction,
    IdentityOutcome, ModelInfo, PostToolObservation, ToolCall, ToolResultStatus, ToolSpec, Usage,
};
use lca_tools::{Capabilities, CapabilityGrants, Denial};
use wasmtime::component::{HasSelf, Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

/// The ABI lines this host loads: the current minor and the one before it
/// (NFR-19).
pub const SUPPORTED_ABI_WINDOW: &str = "0.1..=1.0";

/// Resource limits applied to every call, resolved from the manifest and
/// clamped to host maximums (`docs/flows.md`).
#[derive(Debug, Clone)]
pub struct ExtensionLimits {
    /// Linear memory ceiling per instance.
    pub memory_bytes: usize,
    /// Fuel budget for one call.
    pub fuel_per_call: u64,
    /// Extension log messages above this many bytes truncate (FR-EXT-10).
    pub log_limit_bytes: usize,
}

/// What every load needs from the host: where scopes resolve, how user
/// approval is asked, and which project's grant store holds it.
pub struct HostEnvironment {
    /// Scope roots; `private` is the base, each extension gets a
    /// subdirectory inside it (capability catalog).
    pub roots: ScopeRoots,
    /// The approval prompt (TUI modal, headless deny, scripted in tests).
    pub prompt: Arc<Mutex<dyn PermissionPrompt>>,
    /// The user grant store (ADR-0006).
    pub grant_store: Arc<Mutex<GrantStore>>,
    /// The current project, keyed into the grant store by canonical path.
    pub project: PathBuf,
    /// Permission proposals from a trusted project file.
    pub proposals: Option<Proposals>,
}

/// A parsed `extension.toml`.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Extension identity; also its credential namespace later (FR-PERM-6).
    pub name: String,
    /// Extension release version.
    pub version: String,
    /// The ABI line, `major.minor`.
    pub abi: String,
    /// Worlds the component implements.
    pub worlds: Vec<String>,
    /// Declared `fs` scopes with their modes (FR-PERM-1).
    pub fs: Vec<ScopeGrant>,
    /// The `process` capability was declared (FR-PERM-1).
    pub process: bool,
    /// The `pty` capability was declared (FR-PERM-1).
    pub pty: bool,
    /// `net` host patterns (internet, HTTPS only).
    pub net: Vec<lca_permissions::NetPattern>,
    /// `net-local` address patterns (local ranges, HTTP allowed).
    pub net_local: Vec<lca_permissions::LocalPattern>,
    /// The loopback OAuth flow settings, when declared.
    pub oauth: Option<OAuthSettings>,
    /// The credential namespace, when declared; must equal `name`
    /// (FR-PERM-6, no cross-namespace read at any level, FR-PERM-7).
    pub credentials: bool,
    /// The `completion` capability was declared (ADR-0015).
    pub completion: bool,
    /// The `ui` regions this manifest declares (capability catalog's
    /// four-region enum; empty means no rendering rights at all).
    pub ui_regions: Vec<String>,
    /// The manifest's resource hints, clamped to the host's maxima at
    /// load (schema `limits`: memory64MB/fuel10M defaults,512MB/1B
    /// maximums; absent means the host's own values apply).
    pub limits: Option<ExtensionLimits>,
}

fn reason_of(value: &toml::Value, key: &str) -> Result<String, LoadError> {
    let reason = value
        .get("reason")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LoadError::InvalidManifest(format!("{key} needs a reason string")))?;
    if reason.len() < 10 {
        return Err(LoadError::InvalidManifest(format!(
            "{key}'s reason must say something a person can evaluate (at least10 characters)"
        )));
    }
    Ok(reason.to_string())
}

impl Manifest {
    /// Parse and validate identity and capability declarations (the
    /// schema's rules; full JSON-schema validation lands with Phase 5).
    pub fn parse(toml_text: &str) -> Result<Manifest, LoadError> {
        let value: toml::Value = toml::from_str(toml_text)
            .map_err(|err| LoadError::InvalidManifest(format!("not valid TOML: {err}")))?;
        let get = |key: &str| {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| LoadError::InvalidManifest(format!("missing `{key}`")))
        };
        let name = get("name")?;
        if !name.starts_with(|c: char| c.is_ascii_lowercase())
            || name.len() < 2
            || name.len() > 64
            || !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || name.contains("--")
            || name.ends_with('-')
        {
            return Err(LoadError::InvalidManifest(format!(
                "`name` {name:?} does not match the manifest identifier rules"
            )));
        }
        let version = get("version")?;
        let abi = get("abi")?;
        let worlds = value
            .get("worlds")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .filter(|worlds| !worlds.is_empty())
            .ok_or_else(|| LoadError::InvalidManifest("`worlds` must list at least one".into()))?;

        let mut fs = Vec::new();
        let mut process = false;
        let mut pty = false;
        let mut parsed_net: Vec<lca_permissions::NetPattern> = Vec::new();
        let mut parsed_net_local: Vec<lca_permissions::LocalPattern> = Vec::new();
        let mut parsed_oauth: Option<OAuthSettings> = None;
        let mut parsed_credentials = false;
        let mut parsed_completion = false;
        let mut parsed_ui_regions: Vec<String> = Vec::new();
        let mut parsed_limits: Option<ExtensionLimits> = None;
        if let Some(capabilities) = value.get("capabilities") {
            let table = capabilities.as_table().ok_or_else(|| {
                LoadError::InvalidManifest("`capabilities` must be a table".into())
            })?;
            for key in table.keys() {
                match key.as_str() {
                    "fs" | "process" | "pty" | "net" | "net-local" | "oauth" | "credentials"
                    | "completion" | "ui" => {}
                    other => {
                        return Err(LoadError::InvalidManifest(format!(
                            "unknown capability `{other}`"
                        )));
                    }
                }
            }
            if let Some(cap) = table.get("fs") {
                let scopes = cap.as_table().ok_or_else(|| {
                    LoadError::InvalidManifest("`capabilities.fs` must be a table".into())
                })?;
                for (scope, mode) in scopes {
                    let mode = match mode.as_str() {
                        Some("read") => lca_permissions::FsMode::Read,
                        Some("read-write") => lca_permissions::FsMode::ReadWrite,
                        other => {
                            return Err(LoadError::InvalidManifest(format!(
                                "`{scope}` mode must be `read` or `read-write`, got {other:?}"
                            )));
                        }
                    };
                    fs.push(ScopeGrant::parse(scope, mode).map_err(|_| {
                        LoadError::InvalidManifest(format!("unknown fs scope `{scope}`"))
                    })?);
                }
                if fs.is_empty() {
                    return Err(LoadError::InvalidManifest(
                        "`capabilities.fs` must grant at least one scope".into(),
                    ));
                }
            }
            if let Some(cap) = table.get("process") {
                reason_of(cap, "capabilities.process")?;
                process = true;
            }
            if let Some(cap) = table.get("pty") {
                reason_of(cap, "capabilities.pty")?;
                pty = true;
            }
            let mut net = Vec::new();
            if let Some(cap) = table.get("net") {
                let hosts = cap.get("hosts").and_then(|v| v.as_array()).ok_or_else(|| {
                    LoadError::InvalidManifest("`capabilities.net.hosts` must be a list".into())
                })?;
                for host in hosts {
                    let host = host.as_str().ok_or_else(|| {
                        LoadError::InvalidManifest("net host patterns are strings".into())
                    })?;
                    net.push(
                        lca_permissions::parse_net_pattern(host).map_err(|err| {
                            LoadError::InvalidManifest(format!("`{host}`: {err}"))
                        })?,
                    );
                }
            }
            let mut net_local = Vec::new();
            if let Some(cap) = table.get("net-local") {
                let addresses =
                    cap.get("addresses")
                        .and_then(|v| v.as_array())
                        .ok_or_else(|| {
                            LoadError::InvalidManifest(
                                "`capabilities.net-local.addresses` must be a list".into(),
                            )
                        })?;
                for address in addresses {
                    let address = address.as_str().ok_or_else(|| {
                        LoadError::InvalidManifest("net-local addresses are strings".into())
                    })?;
                    net_local.push(lca_permissions::parse_local_pattern(address).map_err(
                        |err| LoadError::InvalidManifest(format!("`{address}`: {err}")),
                    )?);
                }
            }
            let mut oauth = None;
            if let Some(cap) = table.get("oauth") {
                // The token exchange always needs `net` (manifest schema
                // allOf): oauth without net is a manifest error, not a
                // runtime surprise.
                if net.is_empty() {
                    return Err(LoadError::InvalidManifest(
                        "capabilities.oauth requires capabilities.net for the token exchange"
                            .into(),
                    ));
                }
                let redirect_path = cap
                    .get("redirect_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("/callback")
                    .to_string();
                let timeout_seconds = cap
                    .get("timeout_seconds")
                    .and_then(|v| v.as_integer())
                    .unwrap_or(300) as u64;
                if !(30..=600).contains(&timeout_seconds) {
                    return Err(LoadError::InvalidManifest(
                        "capabilities.oauth.timeout_seconds must be between30 and600".into(),
                    ));
                }
                oauth = Some(OAuthSettings {
                    redirect_path,
                    timeout_seconds,
                });
            }
            let mut credentials = false;
            let mut completion = false;
            if let Some(cap) = table.get("completion") {
                // The reason is required (capability catalog): consent
                // text uses it verbatim.
                reason_of(cap, "capabilities.completion")?;
                completion = true;
            }
            if let Some(cap) = table.get("credentials") {
                let namespace = cap
                    .get("namespace")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        LoadError::InvalidManifest(
                            "capabilities.credentials.namespace required".into(),
                        )
                    })?;
                // FR-PERM-6: the namespace IS the extension identity.
                if namespace != name {
                    return Err(LoadError::InvalidManifest(format!(
                        "credentials namespace `{namespace}` must equal the extension name `{name}`"
                    )));
                }
                credentials = true;
            }
            parsed_net = net;
            parsed_net_local = net_local;
            parsed_oauth = oauth;
            parsed_credentials = credentials;
            parsed_completion = completion;
            let mut ui_regions = Vec::new();
            if let Some(cap) = table.get("ui") {
                let regions = cap
                    .get("regions")
                    .and_then(|r| r.as_array())
                    .ok_or_else(|| {
                        LoadError::InvalidManifest(
                            "`capabilities.ui.regions` must be a list".into(),
                        )
                    })?;
                for region in regions {
                    let region = region.as_str().ok_or_else(|| {
                        LoadError::InvalidManifest("ui regions are strings".into())
                    })?;
                    if !matches!(region, "status-line" | "footer" | "panel" | "modal") {
                        return Err(LoadError::InvalidManifest(format!(
                            "`{region}` is not one of status-line, footer, panel, modal"
                        )));
                    }
                    if ui_regions.contains(&region.to_string()) {
                        return Err(LoadError::InvalidManifest(format!(
                            "duplicate ui region `{region}`"
                        )));
                    }
                    ui_regions.push(region.to_string());
                }
            }
            parsed_ui_regions = ui_regions;
            parsed_limits = manifest_limits(&value)?;
        }

        Ok(Manifest {
            name,
            version,
            abi,
            worlds,
            fs,
            process,
            pty,
            net: parsed_net,
            net_local: parsed_net_local,
            oauth: parsed_oauth,
            credentials: parsed_credentials,
            completion: parsed_completion,
            ui_regions: parsed_ui_regions,
            limits: parsed_limits,
        })
    }

    /// Whether the declared ABI line is inside the supported window
    /// (NFR-19, FR-EXT-8).
    pub fn abi_in_window(&self) -> bool {
        let Some((declared_major, declared_minor)) = parse_abi(&self.abi) else {
            return false;
        };
        let Some((current_major, current_minor)) = parse_abi(lca_ext_abi::ABI_VERSION) else {
            return false;
        };
        if declared_major == current_major {
            return declared_minor == current_minor
                || (current_minor > 0 && declared_minor == current_minor - 1);
        }
        // The freeze grandfather: artifacts published against the line
        // that was current when the ABI froze (0.1) keep loading on a
        // 1.0 host, one cycle of amnesty so nobody's installed
        // extension dies to a version bump that changed no bytes.
        (declared_major, declared_minor) == (0, 1) && (current_major, current_minor) == (1, 0)
    }
}

/// The schema's bounds for `limits` (extension-manifest.schema.json):
/// the host maximums flows.md says every request is clamped to.
pub const MAX_MEMORY_BYTES: usize = 512 * 1024 * 1024;
/// The host's per-call fuel ceiling (schema `limits.fuel_per_call`
/// maximum).
pub const MAX_FUEL_PER_CALL: u64 = 1_000_000_000;

/// Parse the manifest's optional `limits` table against the schema's
/// ranges (out-of-range is clamped at load, here we reject nonsense).
fn manifest_limits(value: &toml::Value) -> Result<Option<ExtensionLimits>, LoadError> {
    let Some(table) = value.get("limits") else {
        return Ok(None);
    };
    let memory_mb = table
        .get("memory_mb")
        .and_then(|v| v.as_integer())
        .unwrap_or(64);
    let fuel = table
        .get("fuel_per_call")
        .and_then(|v| v.as_integer())
        .unwrap_or(10_000_000);
    if memory_mb < 1 || fuel < 1000 {
        return Err(LoadError::InvalidManifest(
            "`limits` values are below the schema minimums".into(),
        ));
    }
    Ok(Some(ExtensionLimits {
        memory_bytes: (memory_mb as usize).min(MAX_MEMORY_BYTES),
        fuel_per_call: (fuel as u64).min(MAX_FUEL_PER_CALL),
        log_limit_bytes: 0, // log stays the host's (config key)
    }))
}

fn parse_abi(value: &str) -> Option<(u64, u64)> {
    let (major, minor) = value.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Something wrong with loading (not with a call).
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// The manifest declares an ABI line outside the supported window
    /// (FR-EXT-8).
    #[error("extension targets ABI {declared}, outside this host's window {SUPPORTED_ABI_WINDOW}")]
    AbiUnsupported {
        /// The declared `major.minor`.
        declared: String,
        /// The host's window, for the report.
        window: &'static str,
    },
    /// The manifest failed validation.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    /// Linking or instantiation failed (an approval-denied capability is
    /// absent from the import table, so this is the deny-by-default
    /// failure path from `docs/flows.md`).
    #[error("cannot link extension: {0}")]
    Link(String),
}

/// Something wrong with one call.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// The extension is disabled for the session (FR-EXT-3, FR-EXT-5).
    #[error("extension is disabled for this session")]
    Disabled,
    /// The call trapped; the extension is now disabled (FR-EXT-3).
    #[error("extension trapped: {0}")]
    Trap(String),
    /// The fuel budget ran out (FR-EXT-4).
    #[error("fuel budget exhausted")]
    FuelExhausted,
    /// Epoch interruption cancelled the call (FR-CONC-1).
    #[error("call cancelled")]
    Cancelled,
    /// The memory ceiling was hit; the extension is now disabled
    /// (FR-EXT-5).
    #[error("memory limit exceeded")]
    MemoryLimit,
    /// The guest's arguments failed the host's shape check.
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
}

type HostLinker = Linker<HostState>;

struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    logs: Arc<Mutex<Vec<String>>>,
    log_limit: usize,
    cap: Arc<Capabilities>,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl lca_ext_abi::host::tool::lca::host::log::Host for HostState {
    fn trace(&mut self, message: String) {
        self.record_log(message);
    }
    fn debug(&mut self, message: String) {
        self.record_log(message);
    }
    fn info(&mut self, message: String) {
        self.record_log(message);
    }
    fn warn(&mut self, message: String) {
        self.record_log(message);
    }
    fn error(&mut self, message: String) {
        self.record_log(message);
    }
}

impl lca_ext_abi::host::tool::lca::ext::types::Host for HostState {}

impl HostState {
    fn record_log(&mut self, message: String) {
        let truncated = truncate_bytes(&message, self.log_limit);
        self.logs.lock().expect("log lock").push(truncated);
    }
}

fn truncate_bytes(message: &str, limit: usize) -> String {
    const MARKER: &str = "... [truncated]";
    if message.len() <= limit {
        return message.to_string();
    }
    let keep = limit.saturating_sub(MARKER.len());
    let mut cut = keep.min(message.len());
    while cut > 0 && !message.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{}", &message[..cut], MARKER)
}

use lca_ext_abi::host::tool::lca::host::fs::Error as FsError;
use lca_ext_abi::host::tool::lca::host::fs::FileInfo;
use lca_ext_abi::host::tool::lca::host::process::Error as ProcessError;
use lca_ext_abi::host::tool::lca::host::pty::Error as PtyError;

fn fs_error(err: CapabilityError) -> FsError {
    match err {
        CapabilityError::Permission(detail) => FsError::Permission(detail),
        CapabilityError::NotGranted(detail) => FsError::NotGranted(detail),
        CapabilityError::NotFound(detail) => FsError::NotFound(detail),
        CapabilityError::Io(detail) => FsError::Io(detail),
        CapabilityError::Invalid(detail) => FsError::Invalid(detail),
        CapabilityError::Timeout(detail) => FsError::Io(format!("timed out: {detail}")),
    }
}

fn process_error(err: CapabilityError) -> ProcessError {
    match err {
        CapabilityError::Permission(detail) => ProcessError::Permission(detail),
        CapabilityError::NotGranted(detail) => ProcessError::NotGranted(detail),
        CapabilityError::NotFound(detail) => ProcessError::NotFound(detail),
        CapabilityError::Io(detail) => ProcessError::Io(detail),
        CapabilityError::Invalid(detail) => ProcessError::Invalid(detail),
        CapabilityError::Timeout(detail) => ProcessError::Io(format!("timed out: {detail}")),
    }
}

fn pty_error(err: CapabilityError) -> PtyError {
    match err {
        CapabilityError::Permission(detail) => PtyError::Permission(detail),
        CapabilityError::NotGranted(detail) => PtyError::NotGranted(detail),
        CapabilityError::NotFound(detail) => PtyError::NotFound(detail),
        CapabilityError::Io(detail) => PtyError::Io(detail),
        CapabilityError::Invalid(detail) => PtyError::Invalid(detail),
        CapabilityError::Timeout(detail) => PtyError::Io(format!("timed out: {detail}")),
    }
}

// ---------------------------------------------------------------------------
// The provider world: capability imports the same engine serves, plus the
// WIT <-> protocol mapping every provider event and record crosses.
// ---------------------------------------------------------------------------

use lca_ext_abi::host::provider::exports::lca::ext::provider_completion as wit_completion;
use lca_ext_abi::host::provider::exports::lca::ext::provider_identity as wit_identity;
use lca_ext_abi::host::provider::lca::host as provider_host;

fn net_error(err: CapabilityError) -> provider_host::net::Error {
    use provider_host::net::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(detail) => E::Invalid(detail),
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
        CapabilityError::Timeout(detail) => E::Io(format!("timed out: {detail}")),
    }
}

fn oauth_error(err: CapabilityError) -> provider_host::oauth::Error {
    use provider_host::oauth::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(_) => E::Invalid("not found".to_string()),
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
        CapabilityError::Timeout(detail) => E::Timeout(detail),
    }
}

fn credentials_error(err: CapabilityError) -> provider_host::credentials::Error {
    use provider_host::credentials::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(_) | CapabilityError::Timeout(_) => {
            E::Io("unavailable".to_string())
        }
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
    }
}

impl provider_host::log::Host for HostState {
    fn trace(&mut self, message: String) {
        self.record_log(message);
    }
    fn debug(&mut self, message: String) {
        self.record_log(message);
    }
    fn info(&mut self, message: String) {
        self.record_log(message);
    }
    fn warn(&mut self, message: String) {
        self.record_log(message);
    }
    fn error(&mut self, message: String) {
        self.record_log(message);
    }
}

impl lca_ext_abi::host::provider::lca::ext::types::Host for HostState {}

impl provider_host::net::Host for HostState {
    fn request(
        &mut self,
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<u32, provider_host::net::Error> {
        let refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        self.cap
            .net_request(&method, &url, &refs, body.as_deref())
            .map_err(net_error)
    }

    fn response_status(&mut self, handle: u32) -> Result<u16, provider_host::net::Error> {
        self.cap.net_response_status(handle).map_err(net_error)
    }

    fn response_headers(
        &mut self,
        handle: u32,
    ) -> Result<Vec<(String, String)>, provider_host::net::Error> {
        self.cap.net_response_headers(handle).map_err(net_error)
    }

    fn read_body(
        &mut self,
        handle: u32,
        max: u64,
    ) -> Result<Option<Vec<u8>>, provider_host::net::Error> {
        self.cap
            .net_read_body(handle, max as usize)
            .map_err(net_error)
    }

    fn close_response(&mut self, handle: u32) -> Result<(), provider_host::net::Error> {
        self.cap.net_close_response(handle).map_err(net_error)
    }
}

impl provider_host::oauth::Host for HostState {
    fn begin(
        &mut self,
        redirect_path: String,
    ) -> Result<(String, u32), provider_host::oauth::Error> {
        self.cap.oauth_begin(&redirect_path).map_err(oauth_error)
    }

    fn open(&mut self, url: String) -> Result<(), provider_host::oauth::Error> {
        self.cap.oauth_open(&url).map_err(oauth_error)
    }

    fn await_callback(
        &mut self,
        handle: u32,
    ) -> Result<Vec<(String, String)>, provider_host::oauth::Error> {
        self.cap.oauth_await(handle).map_err(oauth_error)
    }

    fn end_flow(&mut self, handle: u32) -> Result<(), provider_host::oauth::Error> {
        self.cap.oauth_end(handle).map_err(oauth_error)
    }
}

impl provider_host::credentials::Host for HostState {
    fn get(&mut self, key: String) -> Option<String> {
        // Denial reads as absence (capability catalog): checking for an
        // existing login needs no denial/absence distinction.
        self.cap.credentials_get(&key).unwrap_or(None)
    }

    fn set(&mut self, key: String, value: String) -> Result<(), provider_host::credentials::Error> {
        self.cap
            .credentials_set(&key, &value)
            .map_err(credentials_error)
    }

    fn delete(&mut self, key: String) -> Result<(), provider_host::credentials::Error> {
        self.cap.credentials_delete(&key).map_err(credentials_error)
    }
}

fn role_str(role: lca_protocol::MessageRole) -> &'static str {
    use lca_protocol::MessageRole;
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

/// Protocol request -> the WIT record. Content is the concatenated text
/// of the message's blocks, exactly what the WIT `message` record
/// carries; reasoning blocks are model-internal and not resent.
fn to_wit_request(request: &CompletionRequest) -> wit_completion::CompletionRequest {
    use wit_completion::{
        CompletionRequest as WitRequest, Message as WitMessage, ToolSpec as WitToolSpec,
    };
    let extra_pairs = |extras: &std::collections::BTreeMap<String, String>| {
        extras
            .iter()
            .map(|(key, value)| wit_completion::ExtraPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    };
    WitRequest {
        messages: request
            .messages
            .iter()
            .map(|message| WitMessage {
                role: role_str(message.role).to_string(),
                content: message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(
                        |call| lca_ext_abi::host::provider::lca::ext::types::ToolCall {
                            call_id: call.call_id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                            extras: Vec::new(),
                        },
                    )
                    .collect(),
                tool_call_id: message.tool_call_id.clone(),
                extras: Vec::new(),
            })
            .collect(),
        tools: request
            .tools
            .iter()
            .map(|tool| WitToolSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.to_string(),
                extras: extra_pairs(&tool.extras),
            })
            .collect(),
        model: request.model.clone(),
        stable_prefix: request.stable_prefix as u32,
        extras: extra_pairs(&request.extras),
    }
}

/// The WIT stream event -> the protocol event, case per case
/// (ADR-0004: one case each, `vendor-event` the reserved hatch).
fn from_wit_event(event: wit_completion::StreamEvent) -> lca_protocol::StreamEvent {
    use lca_protocol::StreamEvent as P;
    match event {
        wit_completion::StreamEvent::TextDelta(delta) => P::TextDelta { delta },
        wit_completion::StreamEvent::ReasoningDelta(delta) => P::ReasoningDelta { delta },
        wit_completion::StreamEvent::ToolCallStart((call_id, name)) => {
            P::ToolCallStart { call_id, name }
        }
        wit_completion::StreamEvent::ToolCallArgDelta((call_id, delta)) => {
            P::ToolCallArgDelta { call_id, delta }
        }
        wit_completion::StreamEvent::ToolCallEnd(call_id) => P::ToolCallEnd { call_id },
        wit_completion::StreamEvent::Usage(usage) => P::Usage {
            usage: from_wit_usage(usage),
        },
        wit_completion::StreamEvent::Error((message, retryable)) => P::Error { message, retryable },
        wit_completion::StreamEvent::VendorEvent((kind, payload)) => P::VendorEvent {
            kind,
            payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::String(payload)),
        },
    }
}

fn f64_extra(extras: &std::collections::BTreeMap<String, String>, key: &str) -> f64 {
    extras
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0)
}

/// WIT usage -> protocol usage. The cost buckets (ADR-0017) travel in
/// `extras`, since the WIT record predates the bucket split; the reserved
/// keys stay in `extras` too, where a consumer that does not know them
/// ignores them safely.
fn from_wit_usage(usage: wit_completion::Usage) -> Usage {
    let extras: std::collections::BTreeMap<String, String> = usage
        .extras
        .into_iter()
        .map(|pair| (pair.key, pair.value))
        .collect();
    Usage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        cache_write_1h: usage.cache_write_hour,
        cost: usage.cost,
        cost_input: f64_extra(&extras, "cost_input"),
        cost_cache_read: f64_extra(&extras, "cost_cache_read"),
        cost_cache_write: f64_extra(&extras, "cost_cache_write"),
        extras,
    }
}

fn from_wit_identity(outcome: wit_identity::IdentityOutcome) -> IdentityOutcome {
    match outcome {
        wit_identity::IdentityOutcome::Ok => IdentityOutcome::Ok,
        wit_identity::IdentityOutcome::NotSupported => IdentityOutcome::NotSupported,
        wit_identity::IdentityOutcome::Failed(reason) => IdentityOutcome::Failed(reason),
    }
}

impl lca_ext_abi::host::tool::lca::host::fs::Host for HostState {
    fn read(&mut self, scope: String, path: String) -> Result<Vec<u8>, FsError> {
        self.cap.fs_read(&scope, &path).map_err(fs_error)
    }

    fn write(&mut self, scope: String, path: String, bytes: Vec<u8>) -> Result<(), FsError> {
        self.cap.fs_write(&scope, &path, &bytes).map_err(fs_error)
    }

    fn list_entries(&mut self, scope: String, path: String) -> Result<Vec<String>, FsError> {
        self.cap.fs_list(&scope, &path).map_err(fs_error)
    }

    fn stat(&mut self, scope: String, path: String) -> Result<FileInfo, FsError> {
        self.cap
            .fs_stat(&scope, &path)
            .map(|(is_dir, len)| FileInfo { is_dir, len })
            .map_err(fs_error)
    }
}

fn usize_from(max: u64) -> usize {
    usize::try_from(max).unwrap_or(usize::MAX).max(1)
}

impl lca_ext_abi::host::tool::lca::host::process::Host for HostState {
    fn spawn(
        &mut self,
        program: String,
        args: Vec<String>,
        cwd_scope: String,
    ) -> Result<u32, ProcessError> {
        self.cap
            .process_spawn(&program, &args, &cwd_scope)
            .map_err(process_error)
    }

    fn read_stdout(&mut self, handle: u32, max: u64) -> Result<Option<Vec<u8>>, ProcessError> {
        self.cap
            .process_read_stdout(handle, usize_from(max))
            .map_err(process_error)
    }

    fn read_stderr(&mut self, handle: u32, max: u64) -> Result<Option<Vec<u8>>, ProcessError> {
        self.cap
            .process_read_stderr(handle, usize_from(max))
            .map_err(process_error)
    }

    fn write_stdin(&mut self, handle: u32, bytes: Vec<u8>) -> Result<u64, ProcessError> {
        self.cap
            .process_write_stdin(handle, &bytes)
            .map_err(process_error)
    }

    fn wait(&mut self, handle: u32) -> Result<i32, ProcessError> {
        self.cap.process_wait(handle).map_err(process_error)
    }

    fn kill(&mut self, handle: u32) -> Result<(), ProcessError> {
        self.cap.process_kill(handle).map_err(process_error)
    }
}

impl lca_ext_abi::host::tool::lca::host::pty::Host for HostState {
    fn spawn(
        &mut self,
        program: String,
        args: Vec<String>,
        cwd_scope: String,
        rows: u16,
        cols: u16,
    ) -> Result<u32, PtyError> {
        self.cap
            .pty_spawn(&program, &args, &cwd_scope, rows, cols)
            .map_err(pty_error)
    }

    fn read(&mut self, handle: u32, max: u64) -> Result<Option<Vec<u8>>, PtyError> {
        self.cap
            .pty_read(handle, usize_from(max))
            .map_err(pty_error)
    }

    fn write(&mut self, handle: u32, bytes: Vec<u8>) -> Result<u64, PtyError> {
        self.cap.pty_write(handle, &bytes).map_err(pty_error)
    }

    fn resize(&mut self, handle: u32, rows: u16, cols: u16) -> Result<(), PtyError> {
        self.cap.pty_resize(handle, rows, cols).map_err(pty_error)
    }

    fn wait(&mut self, handle: u32) -> Result<i32, PtyError> {
        self.cap.pty_wait(handle).map_err(pty_error)
    }

    fn kill(&mut self, handle: u32) -> Result<(), PtyError> {
        self.cap.pty_kill(handle).map_err(pty_error)
    }
}

/// The host: one engine shared by every extension load.
pub struct ExtHost {
    engine: Engine,
    limits: ExtensionLimits,
    env: Arc<HostEnvironment>,
}

impl ExtHost {
    /// Build the host with its resource limits and environment.
    pub fn new(limits: ExtensionLimits, env: Arc<HostEnvironment>) -> ExtHost {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).expect("engine");
        ExtHost {
            engine,
            limits,
            env,
        }
    }

    /// The engine, so cancellation can increment its epoch from anywhere.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The resolved limits new stores get.
    pub fn limits(&self) -> &ExtensionLimits {
        &self.limits
    }

    /// Load an extension: validate the manifest (identity, capabilities,
    /// ABI window), resolve its grants, then link and probe-instantiate
    /// so a broken component fails here, before the first input
    /// (FR-EXT-1, FR-EXT-2, FR-EXT-8, `docs/flows.md`).
    pub fn load(&mut self, wasm: &[u8], manifest_text: &str) -> Result<WasmExtension, LoadError> {
        let manifest = Manifest::parse(manifest_text)?;
        if !manifest.abi_in_window() {
            return Err(LoadError::AbiUnsupported {
                declared: manifest.abi.clone(),
                window: SUPPORTED_ABI_WINDOW,
            });
        }

        let cap = Arc::new(Capabilities::new(
            manifest.name.clone(),
            CapabilityGrants {
                fs: manifest.fs.clone(),
                fs_declared: !manifest.fs.is_empty(),
                process: manifest.process,
                pty: manifest.pty,
                net: manifest.net.clone(),
                net_local: manifest.net_local.clone(),
                adhoc_net: Vec::new(), // Phase5's install flow attaches
                // ad hoc grants (FR-PERM-16) from user consent.
                oauth: manifest.oauth.clone(),
                credentials: manifest.credentials,
                completion: manifest.completion,
            },
            self.env.roots.clone(),
            self.env.prompt.clone(),
            self.env.grant_store.clone(),
            self.env.project.clone(),
            self.env.proposals.clone(),
        ));

        // Resource limits come from the manifest, clamped to the
        // host's maxima (flows.md); a manifest without `limits` gets
        // the host's own values (the hints are optional).
        let effective_limits = match manifest.limits {
            Some(requested) => ExtensionLimits {
                memory_bytes: requested.memory_bytes.min(self.limits.memory_bytes),
                fuel_per_call: requested.fuel_per_call.min(self.limits.fuel_per_call),
                log_limit_bytes: self.limits.log_limit_bytes,
            },
            None => self.limits.clone(),
        };
        let component = wasmtime::component::Component::new(&self.engine, wasm)
            .map_err(|err| LoadError::Link(err.to_string()))?;
        let mut linker: HostLinker = Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).expect("wasi imports");
        Tool::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state).expect("world imports");
        // The compaction world always imports `completion`; like every
        // capability interface it links in a denied state until the
        // manifest declares it (FR-PERM-3), so this is unconditional
        // and the grant does the gating (`log` is already defined by
        // the tool world above). It must be defined before
        // `instantiate_pre` validates the component against the linker.
        lca_ext_abi::host::compaction::lca::host::completion::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state| state,
        )
        .expect("completion imports");
        // The provider world's remaining imports link unconditionally, in a
        // denied state until the manifest declares them - the same rule the
        // compaction world's `completion` follows (FR-PERM-3). A component
        // that references them keeps them in its import section even when
        // the manifest declares only the tool world (the conformance probe
        // is exactly that), so the grant does the gating, not the linker.
        {
            use lca_ext_abi::host::provider::lca::host as provider_host;
            provider_host::net::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
                .expect("net imports");
            provider_host::oauth::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
                .expect("oauth imports");
            provider_host::credentials::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
                .expect("credentials imports");
        }
        let declares_provider = manifest.worlds.iter().any(|world| world == "provider");
        let pre = linker
            .instantiate_pre(&component)
            .map_err(|err| LoadError::Link(err.to_string()))?;
        let tool = manifest
            .worlds
            .iter()
            .any(|world| world == "tool")
            .then(|| ToolPre::new(pre.clone()).map_err(|err| LoadError::Link(err.to_string())))
            .transpose()?;
        let command = manifest
            .worlds
            .iter()
            .any(|world| world == "command")
            .then(|| {
                lca_ext_abi::host::command::CommandPre::new(pre.clone())
                    .map_err(|err| LoadError::Link(err.to_string()))
            })
            .transpose()?;
        let provider = declares_provider
            .then(|| ProviderPre::new(pre.clone()).map_err(|err| LoadError::Link(err.to_string())))
            .transpose()?;
        let hooks = manifest
            .worlds
            .iter()
            .any(|world| world == "hooks")
            .then(|| {
                lca_ext_abi::host::hooks::HooksPre::new(pre.clone())
                    .map_err(|err| LoadError::Link(err.to_string()))
            })
            .transpose()?;
        let compaction = manifest
            .worlds
            .iter()
            .any(|world| world == "compaction")
            .then(|| {
                CompactionPre::new(pre.clone()).map_err(|err| LoadError::Link(err.to_string()))
            })
            .transpose()?;
        let context_transform = manifest
            .worlds
            .iter()
            .any(|world| world == "context-transform")
            .then(|| {
                ContextTransformPre::new(pre.clone())
                    .map_err(|err| LoadError::Link(err.to_string()))
            })
            .transpose()?;
        let ui = manifest
            .worlds
            .iter()
            .any(|world| world == "ui")
            .then(|| UiPre::new(pre).map_err(|err| LoadError::Link(err.to_string())))
            .transpose()?;

        Ok(WasmExtension {
            inner: Arc::new(Inner {
                engine: self.engine.clone(),
                name: manifest.name,
                worlds: manifest.worlds.clone(),
                tool,
                command,
                hooks,
                provider,
                compaction,
                context_transform,
                ui,
                ui_regions: manifest.ui_regions.clone(),
                limits: effective_limits,
                enabled: Arc::new(AtomicBool::new(true)),
                in_flight: AtomicU64::new(0),
                logs: Arc::new(Mutex::new(Vec::new())),
                cap,
            }),
        })
    }
}

/// All pieces one loaded extension needs; behind an `Arc` so every
/// component call can run on its own blocking thread with a cloned
/// handle (synchronous WASI only works where no runtime is polling,
/// ADR-0014).
struct Inner {
    engine: Engine,
    name: String,
    worlds: Vec<String>,
    tool: Option<ToolPre<HostState>>,
    command: Option<lca_ext_abi::host::command::CommandPre<HostState>>,
    hooks: Option<lca_ext_abi::host::hooks::HooksPre<HostState>>,
    provider: Option<ProviderPre<HostState>>,
    compaction: Option<CompactionPre<HostState>>,
    context_transform: Option<ContextTransformPre<HostState>>,
    ui: Option<UiPre<HostState>>,
    /// The manifest's granted ui regions (the host only asks these).
    ui_regions: Vec<String>,
    limits: ExtensionLimits,
    enabled: Arc<AtomicBool>,
    logs: Arc<Mutex<Vec<String>>>,
    cap: Arc<Capabilities>,
    /// Calls currently inside the guest: incremented the moment the
    /// component is instantiated and the run begins, decremented when
    /// it returns. Cancellation semantics are about interrupting a
    /// *running* call, so a test that must know the guest is running
    /// (rather than still building its store) watches this.
    in_flight: AtomicU64,
}

impl Inner {
    /// Count this call as inside the guest until the guard drops -
    /// including on the error and panic paths, which is exactly when
    /// the count must go back down.
    fn in_flight_guard(&self) -> InFlightGuard<'_> {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        InFlightGuard { inner: self }
    }

    fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
    }

    fn build_store(&self) -> Result<Store<HostState>, CallError> {
        if !self.enabled.load(Ordering::SeqCst) {
            return Err(CallError::Disabled);
        }
        let wasi = wasmtime_wasi::WasiCtx::builder().build();
        let mut store = Store::new(
            &self.engine,
            HostState {
                wasi,
                table: ResourceTable::new(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(self.limits.memory_bytes)
                    .build(),
                logs: self.logs.clone(),
                log_limit: self.limits.log_limit_bytes,
                cap: self.cap.clone(),
            },
        );
        store.limiter(|state| &mut state.limits);
        // One epoch tick of grace: with epoch interruption enabled a store
        // starts already past its deadline, so give every fresh store one
        // tick and let cancellation consume it (ADR-0014: the host, not
        // the guest, decides when the deadline passes).
        store.set_epoch_deadline(1);
        store
            .set_fuel(self.limits.fuel_per_call)
            .map_err(|err| CallError::InvalidArguments(err.to_string()))?;
        Ok(store)
    }

    /// Map a Wasmtime failure onto the host's contract: which failures
    /// disable the extension (FR-EXT-3, FR-EXT-5) and which merely answer
    /// the caller (FR-EXT-4, FR-CONC-1).
    fn classify(&self, err: wasmtime::Error) -> CallError {
        use wasmtime::Trap;
        if let Some(trap) = err.downcast_ref::<Trap>() {
            return match trap {
                Trap::OutOfFuel => CallError::FuelExhausted,
                Trap::Interrupt => CallError::Cancelled,
                other => {
                    self.disable();
                    CallError::Trap(other.to_string())
                }
            };
        }
        let text = err.to_string();
        if text.contains("allocation") || text.contains("memory ceiling") {
            self.disable();
            return CallError::MemoryLimit;
        }
        if text.contains("fuel") {
            return CallError::FuelExhausted;
        }
        if text.contains("epoch") || text.contains("interrupt") {
            return CallError::Cancelled;
        }
        self.disable();
        CallError::Trap(text)
    }
}

type DispatchCommandSpec = lca_protocol::CommandSpec;

fn schema_work(inner: &Inner) -> Result<ToolSpec, CallError> {
    let pre = inner
        .tool
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no tool world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let schema = instance
        .lca_ext_tool_schema()
        .call_get_schema(&mut store)
        .map_err(|err| inner.classify(err))?;
    let parameters = serde_json::from_str(&schema.parameters)
        .unwrap_or_else(|_| serde_json::json!({ "type": "object" }));
    Ok(ToolSpec {
        name: schema.name,
        description: schema.description,
        parameters,
        extras: Default::default(),
    })
}

struct InFlightGuard<'a> {
    inner: &'a Inner,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.inner.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

fn execute_work(inner: &Inner, call: ToolCall) -> Result<lca_protocol::ToolResult, CallError> {
    let pre = inner
        .tool
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no tool world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let guest_call = lca_ext_abi::host::tool::lca::ext::types::ToolCall {
        call_id: call.call_id,
        name: call.name,
        arguments: call.arguments,
        extras: Vec::new(),
    };
    let _inside_guest = inner.in_flight_guard();
    let guest_result = instance
        .lca_ext_execute()
        .call_run(&mut store, &guest_call)
        .map_err(|err| inner.classify(err))?;
    Ok(lca_protocol::ToolResult {
        call_id: guest_result.call_id,
        status: match guest_result.status.as_str() {
            "ok" => ToolResultStatus::Ok,
            "denied" => ToolResultStatus::Denied,
            "timeout" => ToolResultStatus::Timeout,
            _ => ToolResultStatus::Error,
        },
        content: guest_result.content.unwrap_or_default(),
        truncated: guest_result.truncated,
        extras: Default::default(),
    })
}

fn command_specs_work(inner: &Inner) -> Result<Vec<DispatchCommandSpec>, CallError> {
    let pre = inner
        .command
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no command world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let spec = instance
        .lca_ext_command_spec()
        .call_get_spec(&mut store)
        .map_err(|err| inner.classify(err))?;
    Ok(vec![DispatchCommandSpec {
        name: spec.name,
        hint: spec.hint,
        completion: spec.completion,
        extras: spec
            .extras
            .into_iter()
            .map(|pair| (pair.key, pair.value))
            .collect(),
    }])
}

fn invoke_work(inner: &Inner, leaf: &str, argument: &str) -> Result<CommandEffect, CallError> {
    let pre = inner
        .command
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no command world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let effect = instance
        .lca_ext_invoke()
        .call_run(&mut store, argument)
        .map_err(|err| inner.classify(err))?;
    let _ = leaf;
    use lca_ext_abi::host::command::exports::lca::ext::invoke::Effect;
    Ok(match effect {
        Effect::InsertText(text) => CommandEffect::InsertText(text),
        Effect::SubmitPrompt(text) => CommandEffect::SubmitPrompt(text),
        Effect::ShowWidget(text) => CommandEffect::ShowWidget(text),
        Effect::None => CommandEffect::None,
    })
}

fn pre_tool_work(inner: &Inner, call: ToolCall) -> Result<HookAction, CallError> {
    let pre = inner
        .hooks
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no hooks world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let action = instance
        .lca_ext_hook_pre_tool_use()
        .call_on_pre_tool_use(
            &mut store,
            &lca_ext_abi::host::hooks::exports::lca::ext::hook_pre_tool_use::ToolCall {
                call_id: call.call_id,
                name: call.name,
                arguments: call.arguments,
                extras: Vec::new(),
            },
        )
        .map_err(|err| inner.classify(err))?;
    use lca_ext_abi::host::hooks::exports::lca::ext::hook_pre_tool_use::Action;
    Ok(match action {
        Action::Allow => HookAction::Allow,
        Action::Deny(reason) => HookAction::Deny(reason),
        Action::Replace(replacement) => HookAction::Replace(ToolCall {
            call_id: replacement.call_id,
            name: replacement.name,
            arguments: replacement.arguments,
        }),
    })
}

fn observe_work(
    inner: &Inner,
    observation: Option<&PostToolObservation>,
    status: Option<&str>,
    attention: Option<&str>,
) -> Result<(), CallError> {
    let pre = inner
        .hooks
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no hooks world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    if let Some(observation) = observation {
        instance
            .lca_ext_hook_post_tool_use()
            .call_on_post_tool_use(
                &mut store,
                &lca_ext_abi::host::hooks::exports::lca::ext::hook_post_tool_use::ToolCall {
                    call_id: observation.call.call_id.clone(),
                    name: observation.call.name.clone(),
                    arguments: observation.call.arguments.clone(),
                    extras: Vec::new(),
                },
                &lca_ext_abi::host::hooks::exports::lca::ext::hook_post_tool_use::ToolResult {
                    call_id: observation.result.call_id.clone(),
                    status: match observation.result.status {
                        ToolResultStatus::Ok => "ok".to_string(),
                        ToolResultStatus::Error => "error".to_string(),
                        ToolResultStatus::Denied => "denied".to_string(),
                        ToolResultStatus::Timeout => "timeout".to_string(),
                    },
                    content: Some(observation.result.content.clone()),
                    truncated: observation.result.truncated,
                    extras: Vec::new(),
                },
            )
            .map_err(|err| inner.classify(err))?;
    } else if let Some(status) = status {
        instance
            .lca_ext_hook_post_turn_end()
            .call_on_post_turn_end(&mut store, status)
            .map_err(|err| inner.classify(err))?;
    } else if let Some(reason) = attention {
        instance
            .lca_ext_hook_attention_required()
            .call_on_attention_required(&mut store, reason)
            .map_err(|err| inner.classify(err))?;
    } else {
        instance
            .lca_ext_hook_pre_turn()
            .call_on_pre_turn(&mut store)
            .map_err(|err| inner.classify(err))?;
    }
    Ok(())
}

fn session_close_work(inner: &Inner) -> Result<(), CallError> {
    let pre = inner
        .hooks
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no hooks world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    instance
        .lca_ext_hook_session_close()
        .call_on_session_close(&mut store)
        .map_err(|err| inner.classify(err))
}

// ---------------------------------------------------------------------------
// The `completion` capability: the host's side of ADR-0008's star
// ---------------------------------------------------------------------------

use lca_ext_abi::host::compaction::exports::lca::ext as compaction_exports;
use lca_ext_abi::host::compaction::lca::host::completion as wit_cap_completion;
use lca_ext_abi::host::context_transform::exports::lca::ext as transform_exports;

fn completion_error(err: CapabilityError) -> wit_cap_completion::Error {
    use wit_cap_completion::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(_) | CapabilityError::Timeout(_) => E::Io("unavailable".into()),
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
    }
}

/// `lca:host/types.message` -> the protocol message (the host package
/// keeps its own copies of the records; this is the crossing).
fn from_host_message(
    message: lca_ext_abi::host::compaction::lca::host::types::Message,
) -> lca_protocol::ChatMessage {
    use lca_protocol::{ChatMessage, ContentBlock, MessageRole};
    ChatMessage {
        role: match message.role.as_str() {
            "system" => MessageRole::System,
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            _ => MessageRole::Tool,
        },
        content: if message.content.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock::Text {
                text: message.content,
            }]
        },
        tool_calls: message
            .tool_calls
            .iter()
            .map(|call| ToolCall {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect(),
        tool_call_id: message.tool_call_id,
        usage: None,
        extras: Default::default(),
    }
}

/// The protocol reply -> `lca:host/completion.response`.
fn to_host_response(text: String, usage: lca_protocol::Usage) -> wit_cap_completion::Response {
    let mut extras = usage
        .extras
        .iter()
        .map(
            |(key, value)| lca_ext_abi::host::compaction::lca::host::types::ExtraPair {
                key: key.clone(),
                value: value.clone(),
            },
        )
        .collect::<Vec<_>>();
    for (key, value) in [
        ("cost_input", usage.cost_input),
        ("cost_cache_read", usage.cost_cache_read),
        ("cost_cache_write", usage.cost_cache_write),
    ] {
        if value != 0.0 {
            extras.push(lca_ext_abi::host::compaction::lca::host::types::ExtraPair {
                key: key.to_string(),
                value: value.to_string(),
            });
        }
    }
    use lca_ext_abi::host::compaction::lca::host::types::Usage as HostUsage;
    wit_cap_completion::Response {
        text,
        usage: HostUsage {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cache_write_hour: usage.cache_write_1h,
            cost: usage.cost,
            extras,
        },
        extras: Vec::new(),
    }
}

impl wit_cap_completion::Host for HostState {
    fn request(
        &mut self,
        messages: Vec<lca_ext_abi::host::compaction::lca::host::types::Message>,
    ) -> Result<wit_cap_completion::Response, wit_cap_completion::Error> {
        let messages = messages.into_iter().map(from_host_message).collect();
        let (text, usage) = self.cap.complete(messages).map_err(completion_error)?;
        Ok(to_host_response(text, usage))
    }
}

// ---------------------------------------------------------------------------
// Provider-world work: model listing, the streaming completion, identity
// ---------------------------------------------------------------------------

fn provider_missing_world() -> CallError {
    CallError::InvalidArguments("no provider world".into())
}

fn provider_models_work(inner: &Inner) -> Result<Vec<ModelInfo>, CallError> {
    let pre = inner.provider.as_ref().ok_or_else(provider_missing_world)?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let models = instance
        .lca_ext_provider_models()
        .call_list_models(&mut store)
        .map_err(|err| inner.classify(err))?;
    Ok(models
        .into_iter()
        .map(|model| ModelInfo {
            id: model.id,
            name: model.name,
            context_window: model.context_window,
            max_tokens: model.max_tokens,
        })
        .collect())
}

/// One streaming completion, run on a blocking thread: instantiate, call
/// `stream-completion`, then poll the pull resource until it ends,
/// pushing every event into `bridge` (ADR-0004's host-driven shape).
fn provider_stream_work(
    inner: &Inner,
    request: CompletionRequest,
    bridge: Arc<dyn EventSink>,
) -> Result<(), CallError> {
    let pre = inner.provider.as_ref().ok_or_else(provider_missing_world)?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let wit_request = to_wit_request(&request);
    let created = instance
        .lca_ext_provider_completion()
        .call_stream_completion(&mut store, &wit_request)
        .map_err(|err| inner.classify(err))?;
    let stream = created
        .map_err(|detail| CallError::InvalidArguments(format!("stream-completion: {detail}")))?;
    loop {
        let next = instance
            .lca_ext_provider_completion()
            .completion_stream()
            .call_next(&mut store, stream)
            .map_err(|err| inner.classify(err))?;
        match next {
            Some(event) => {
                if !bridge.push(from_wit_event(event)) {
                    // The receiver is gone (FR-CONC-3): stop polling; the
                    // stream resource is dropped with the store.
                    break;
                }
            }
            None => break,
        }
    }
    // The handle indexes the guest's own table inside this store's
    // instance; both die together at the end of this call (a fresh store
    // per call), so nothing leaks even without an explicit delete.
    let _ = stream;
    Ok(())
}

enum IdentityOp {
    Login,
    Logout,
}

fn identity_simple_work(inner: &Inner, op: IdentityOp) -> Result<IdentityOutcome, CallError> {
    let pre = inner.provider.as_ref().ok_or_else(provider_missing_world)?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let outcome = match op {
        IdentityOp::Login => instance
            .lca_ext_provider_identity()
            .call_login(&mut store)
            .map_err(|err| inner.classify(err))?,
        IdentityOp::Logout => instance
            .lca_ext_provider_identity()
            .call_logout(&mut store)
            .map_err(|err| inner.classify(err))?,
    };
    Ok(from_wit_identity(outcome))
}

fn identity_usage_work(inner: &Inner) -> Result<Result<Usage, IdentityOutcome>, CallError> {
    let pre = inner.provider.as_ref().ok_or_else(provider_missing_world)?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let outcome = instance
        .lca_ext_provider_identity()
        .call_usage(&mut store)
        .map_err(|err| inner.classify(err))?;
    Ok(match outcome {
        Ok(usage) => Ok(from_wit_identity_usage(usage)),
        Err(fallback) => Err(from_wit_identity(fallback)),
    })
}

/// Identity `usage` returns the imported `types.usage` record
/// (`token-usage`); convert it through the same WIT usage shape.
fn from_wit_identity_usage(usage: wit_identity::TokenUsage) -> Usage {
    from_wit_usage(wit_completion::Usage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        cache_write_hour: usage.cache_write_hour,
        cost: usage.cost,
        extras: usage.extras,
    })
}

// ---------------------------------------------------------------------------
// Compaction and context-transform work
// ---------------------------------------------------------------------------

fn to_wit_session_record(
    record: &lca_protocol::Record,
) -> compaction_exports::compact::SessionRecord {
    compaction_exports::compact::SessionRecord {
        kind: record.type_tag().to_string(),
        id: record.id().unwrap_or_default().to_string(),
        body: serde_json::to_string(record).unwrap_or_default(),
        extras: Vec::new(),
    }
}

fn compact_work(inner: &Inner, records: Vec<lca_protocol::Record>) -> Result<String, CallError> {
    let pre = inner
        .compaction
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no compaction world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let wit_records: Vec<_> = records.iter().map(to_wit_session_record).collect();
    let summary = instance
        .lca_ext_compact()
        .call_compact(&mut store, &wit_records)
        .map_err(|err| inner.classify(err))?;
    summary.map_err(|reason| CallError::InvalidArguments(format!("compaction refused: {reason}")))
}

/// Protocol messages -> the `context-transform` world's WIT records.
fn to_wit_messages(
    messages: &[lca_protocol::ChatMessage],
) -> Vec<transform_exports::transform::Message> {
    use transform_exports::transform::Message as WitMessage;
    messages
        .iter()
        .map(|message| WitMessage {
            role: match message.role {
                lca_protocol::MessageRole::System => "system".to_string(),
                lca_protocol::MessageRole::User => "user".to_string(),
                lca_protocol::MessageRole::Assistant => "assistant".to_string(),
                lca_protocol::MessageRole::Tool => "tool".to_string(),
            },
            content: message
                .content
                .iter()
                .filter_map(|block| match block {
                    lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            tool_calls: message
                .tool_calls
                .iter()
                .map(
                    |call| lca_ext_abi::host::context_transform::lca::ext::types::ToolCall {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        extras: Vec::new(),
                    },
                )
                .collect(),
            tool_call_id: message.tool_call_id.clone(),
            extras: Vec::new(),
        })
        .collect()
}

fn from_wit_messages(
    messages: Vec<transform_exports::transform::Message>,
) -> Vec<lca_protocol::ChatMessage> {
    messages
        .into_iter()
        .map(|message| lca_protocol::ChatMessage {
            role: match message.role.as_str() {
                "system" => lca_protocol::MessageRole::System,
                "user" => lca_protocol::MessageRole::User,
                "assistant" => lca_protocol::MessageRole::Assistant,
                _ => lca_protocol::MessageRole::Tool,
            },
            content: if message.content.is_empty() {
                Vec::new()
            } else {
                vec![lca_protocol::ContentBlock::Text {
                    text: message.content,
                }]
            },
            tool_calls: message
                .tool_calls
                .iter()
                .map(|call| ToolCall {
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                })
                .collect(),
            tool_call_id: message.tool_call_id,
            usage: None,
            extras: Default::default(),
        })
        .collect()
}

/// One transform pass: the guest's `Err(reason)` is the rejection
/// (FR-CTX-3), not a host failure.
fn transform_work(
    inner: &Inner,
    messages: Vec<lca_protocol::ChatMessage>,
) -> Result<Result<Vec<lca_protocol::ChatMessage>, String>, CallError> {
    let pre = inner
        .context_transform
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no context-transform world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let wit_messages = to_wit_messages(&messages);
    let outcome = instance
        .lca_ext_transform()
        .call_transform(&mut store, &wit_messages)
        .map_err(|err| inner.classify(err))?;
    Ok(match outcome {
        Ok(list) => Ok(from_wit_messages(list)),
        Err(reason) => Err(reason),
    })
}

// ---------------------------------------------------------------------------
// The ui world: widget trees out, interactions in (ADR-0003)
// ---------------------------------------------------------------------------

use lca_ext_abi::host::ui::exports::lca::ext as ui_exports;

/// The WIT case -> protocol widget.
fn from_wit_widget(widget: ui_exports::render::Widget) -> lca_protocol::Widget {
    use lca_protocol::Widget as W;
    use ui_exports::render::Widget as Wit;
    match widget {
        Wit::Text((content, role)) => W::Text { content, role },
        Wit::Image((media_type, bytes)) => W::Image { media_type, bytes },
        Wit::Boxed((title, child)) => W::Boxed { title, child },
        Wit::Row(children) => W::Row(children),
        Wit::Column(children) => W::Column(children),
        Wit::Spinner(frames) => W::Spinner { frames },
        Wit::Progress((label, fill)) => W::Progress { label, fill },
        Wit::Keyvalue(pairs) => W::KeyValue(pairs),
        Wit::Vendor(kind) => W::Vendor(kind),
    }
}

fn render_work(inner: &Inner, region: &str) -> Result<Option<lca_protocol::WidgetTree>, CallError> {
    let pre = inner
        .ui
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no ui world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let tree = instance
        .lca_ext_render()
        .call_render(&mut store, region)
        .map_err(|err| inner.classify(err))?;
    Ok(tree.map(|nodes| lca_protocol::WidgetTree {
        nodes: nodes.into_iter().map(from_wit_widget).collect(),
    }))
}

fn event_work(
    inner: &Inner,
    region: &str,
    input: &lca_protocol::UiInput,
) -> Result<lca_protocol::UiEffect, CallError> {
    use lca_protocol::UiEffect;
    use ui_exports::interaction::Input as WasmInput;
    let pre = inner
        .ui
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no ui world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let wasm_input = match input {
        lca_protocol::UiInput::Key { key } => WasmInput::Key(key.clone()),
        lca_protocol::UiInput::Submit { text } => WasmInput::Submit(text.clone()),
        lca_protocol::UiInput::Cancel => WasmInput::Cancel,
    };
    let effect = instance
        .lca_ext_interaction()
        .call_handle(&mut store, region, &wasm_input)
        .map_err(|err| inner.classify(err))?;
    use ui_exports::interaction::Effect;
    let _ = region;
    Ok(match effect {
        Effect::None => UiEffect::None,
        Effect::CloseModal => UiEffect::CloseModal,
        Effect::OpenModal => UiEffect::OpenModal,
        Effect::ShowNotice(text) => UiEffect::ShowNotice(text),
        Effect::InsertText(text) => UiEffect::InsertText(text),
        Effect::SubmitPrompt(text) => UiEffect::SubmitPrompt(text),
    })
}

/// One loaded extension (ADR-0019's handle for the WASM mode).
#[derive(Clone)]
pub struct WasmExtension {
    inner: Arc<Inner>,
}

/// Run blocking component work on the runtime's blocking pool. A panic in the
/// host glue is caught here and disables the extension rather than the caller
/// on unwind builds; the release profile sets `panic = "abort"`, so there a
/// host panic is fatal and this guard is a test/debug safety net. Guest traps
/// are ordinary `Error`s and are handled regardless (FR-EXT-3).
async fn pool_call<T: Send + 'static>(
    inner: Arc<Inner>,
    work: impl FnOnce(&Inner) -> Result<T, CallError> + Send + 'static,
) -> Result<T, CallError> {
    let inner2 = inner.clone();
    match tokio::task::spawn_blocking(move || work(&inner2)).await {
        Ok(result) => result,
        Err(_) => {
            inner.disable();
            Err(CallError::Trap("host call panicked".to_string()))
        }
    }
}

impl WasmExtension {
    /// Run blocking component work on a dedicated thread: synchronous
    /// WASI blocks on the ambient handle, which only exists where no
    /// runtime is polling (ADR-0014's blocking-region rule). A panic in the
    /// host glue is caught on unwind builds (the release profile aborts, so
    /// there it is fatal) and disables the extension rather than the caller.
    fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Inner) -> Result<T, CallError> + Send + 'static,
    ) -> Result<T, CallError> {
        let inner = self.inner.clone();
        match std::thread::spawn(move || work(&inner)).join() {
            Ok(result) => result,
            Err(_) => {
                self.inner.disable();
                Err(CallError::Trap("host call panicked".to_string()))
            }
        }
    }

    /// The async twin: same work on the runtime's blocking pool.
    async fn on_blocking_pool<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Inner) -> Result<T, CallError> + Send + 'static,
    ) -> Result<T, CallError> {
        pool_call(self.inner.clone(), work).await
    }

    /// The extension's identity.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Whether it is still enabled for this session (FR-EXT-7's data, and
    /// the disable bit FR-EXT-3/5 set).
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::SeqCst)
    }

    /// Everything this extension logged through the always-granted import,
    /// already truncated at the configured limit (FR-EXT-10).
    pub fn captured_logs(&self) -> Vec<String> {
        self.inner.logs.lock().expect("log lock").clone()
    }

    /// Every capability attempt this extension was refused (FR-EXT-9's
    /// data; `lca ext info` prints the count in Phase 5).
    pub fn denials(&self) -> Vec<Denial> {
        self.inner.cap.denials()
    }

    /// How many capability attempts were refused (FR-EXT-9).
    pub fn denial_count(&self) -> usize {
        self.inner.cap.denial_count()
    }

    /// The shared capability engine, for native-mode twins of this
    /// extension's behavior (conformance identity).
    pub fn capabilities(&self) -> Arc<Capabilities> {
        self.inner.cap.clone()
    }

    /// The tool spec the extension registers (blocking; registration-time
    /// or test use).
    pub fn schema(&self) -> Result<ToolSpec, CallError> {
        self.blocking(schema_work)
    }

    /// How many calls are inside the guest right now: the component
    /// is instantiated and running, not still being built. Cancellation
    /// (NFR-29) acts on calls this counter covers.
    pub fn in_flight(&self) -> u64 {
        self.inner.in_flight.load(Ordering::SeqCst)
    }

    /// Execute one call (blocking; tests use this, the loop awaits
    /// `execute_tool`).
    pub fn execute(&self, call: &ToolCall) -> Result<lca_protocol::ToolResult, CallError> {
        let call = call.clone();
        self.blocking(move |inner| execute_work(inner, call))
    }
}

fn to_dispatch(call_err: CallError, extension: &str) -> DispatchError {
    match call_err {
        CallError::Disabled => DispatchError::Disabled,
        other => DispatchError::Failed(format!("{extension}: {other}")),
    }
}

impl lca_ext_abi::ExtensionDispatch for WasmExtension {
    fn name(&self) -> &str {
        &self.inner.name
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Wasm
    }

    fn worlds(&self) -> Vec<World> {
        self.inner
            .worlds
            .iter()
            .filter_map(|world| match world.as_str() {
                "tool" => Some(World::Tool),
                "command" => Some(World::Command),
                "hooks" => Some(World::Hooks),
                "provider" => Some(World::Provider),
                "compaction" => Some(World::Compaction),
                "context-transform" => Some(World::ContextTransform),
                "ui" => Some(World::Ui),
                _ => None,
            })
            .collect()
    }

    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        if !self.worlds().contains(&World::Tool) {
            return Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "tool",
            });
        }
        self.schema()
            .map(|spec| vec![spec])
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn execute_tool<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Tool) {
                return Err(DispatchError::MissingWorld {
                    extension: self.name().to_string(),
                    world: "tool",
                });
            }
            let call = call.clone();
            self.on_blocking_pool(move |inner| execute_work(inner, call))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn command_specs(&self) -> Result<Vec<DispatchCommandSpec>, DispatchError> {
        if !self.worlds().contains(&World::Command) {
            return Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "command",
            });
        }
        self.blocking(command_specs_work)
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn invoke_command(&self, _name: &str, argument: &str) -> Result<CommandEffect, DispatchError> {
        if !self.worlds().contains(&World::Command) {
            return Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "command",
            });
        }
        let leaf = _name.to_string();
        let argument = argument.to_string();
        self.blocking(move |inner| invoke_work(inner, &leaf, &argument))
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn on_pre_turn(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        let inner = self.inner.clone();
        let name = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"hooks".to_string()) {
                return Ok(());
            }
            pool_call(inner, |inner| observe_work(inner, None, None, None))
                .await
                .map_err(|err| to_dispatch(err, &name))
        })
    }

    fn on_pre_tool_use<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Hooks) {
                return Ok(HookAction::Allow);
            }
            let call = call.clone();
            self.on_blocking_pool(move |inner| pre_tool_work(inner, call))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_post_tool_use<'a>(
        &'a self,
        observation: &'a PostToolObservation,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Hooks) {
                return Ok(());
            }
            let observation = observation.clone();
            self.on_blocking_pool(move |inner| observe_work(inner, Some(&observation), None, None))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_post_turn_end<'a>(
        &'a self,
        status: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Hooks) {
                return Ok(());
            }
            let status = status.to_string();
            self.on_blocking_pool(move |inner| observe_work(inner, None, Some(&status), None))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_attention_required<'a>(
        &'a self,
        reason: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Hooks) {
                return Ok(());
            }
            let reason = reason.to_string();
            self.on_blocking_pool(move |inner| observe_work(inner, None, None, Some(&reason)))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_session_close(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        let inner = self.inner.clone();
        let name = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"hooks".to_string()) {
                return Ok(());
            }
            pool_call(inner, session_close_work)
                .await
                .map_err(|err| to_dispatch(err, &name))
        })
    }

    fn provider_models(&self) -> Result<Vec<ModelInfo>, DispatchError> {
        if !self.worlds().contains(&World::Provider) {
            return Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "provider",
            });
        }
        self.blocking(provider_models_work)
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn stream_completion<'a>(
        &'a self,
        request: CompletionRequest,
        sink: &'a dyn EventSink,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Provider) {
                return Err(DispatchError::MissingWorld {
                    extension: self.name().to_string(),
                    world: "provider",
                });
            }
            // The component call runs on the blocking pool (ADR-0014);
            // events cross back through an unbounded bridge the async
            // side drains into `sink` with real backpressure. A dropped
            // `sink` (or a dropped future) ends the stream: the bridge
            // send fails and the work loop stops (FR-CONC-3).
            let (stx, mut srx) = tokio::sync::mpsc::unbounded_channel();
            struct Bridge(tokio::sync::mpsc::UnboundedSender<lca_protocol::StreamEvent>);
            impl EventSink for Bridge {
                fn push(&self, event: lca_protocol::StreamEvent) -> bool {
                    self.0.send(event).is_ok()
                }
            }
            let engine = self.inner.engine.clone();
            let work_inner = self.inner.clone();
            let fail_inner = self.inner.clone();
            let extension = self.inner.name.clone();
            let mut join = tokio::task::spawn_blocking(move || {
                provider_stream_work(&work_inner, request, Arc::new(Bridge(stx)))
            });
            let mut joined: Option<Result<Result<(), CallError>, tokio::task::JoinError>> = None;
            loop {
                if joined.is_some() {
                    while let Some(event) = srx.recv().await {
                        let _ = sink.push(event);
                    }
                    break;
                }
                tokio::select! {
                    maybe = srx.recv() => match maybe {
                        None => break,
                        Some(event) => if !sink.push(event) {
                            // Receiver gone: trap the guest so the blocking
                            // work ends promptly, then drain the bridge.
                            engine.increment_epoch();
                            while srx.recv().await.is_some() {}
                            break;
                        },
                    },
                    result = &mut join => joined = Some(result),
                }
            }
            let result = match joined {
                Some(result) => result,
                None => join.await,
            };
            match result {
                Ok(inner_result) => inner_result.map_err(|err| to_dispatch(err, &extension)),
                Err(_) => {
                    fail_inner.disable();
                    Err(DispatchError::Failed(format!(
                        "{extension}: host call panicked"
                    )))
                }
            }
        })
    }

    fn identity_login(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"provider".to_string()) {
                return Err(DispatchError::MissingWorld {
                    extension,
                    world: "provider",
                });
            }
            pool_call(inner, |inner| {
                identity_simple_work(inner, IdentityOp::Login)
            })
            .await
            .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn identity_logout(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"provider".to_string()) {
                return Err(DispatchError::MissingWorld {
                    extension,
                    world: "provider",
                });
            }
            pool_call(inner, |inner| {
                identity_simple_work(inner, IdentityOp::Logout)
            })
            .await
            .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn identity_usage(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<Result<Usage, IdentityOutcome>, DispatchError>>
    {
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"provider".to_string()) {
                return Err(DispatchError::MissingWorld {
                    extension,
                    world: "provider",
                });
            }
            pool_call(inner, identity_usage_work)
                .await
                .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn compact(
        &self,
        records: &[lca_protocol::Record],
    ) -> lca_ext_abi::DispatchFuture<'static, Result<String, DispatchError>> {
        if !self.worlds().contains(&World::Compaction) {
            return Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "compaction",
            })));
        }
        let records = records.to_vec();
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            pool_call(inner, move |inner| compact_work(inner, records))
                .await
                .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn transform_messages(
        &self,
        messages: Vec<lca_protocol::ChatMessage>,
    ) -> lca_ext_abi::DispatchFuture<
        'static,
        Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
    > {
        if !self.worlds().contains(&World::ContextTransform) {
            return Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "context-transform",
            })));
        }
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            pool_call(inner, move |inner| transform_work(inner, messages))
                .await
                .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn ui_regions(&self) -> Vec<String> {
        self.inner.ui_regions.clone()
    }

    fn render(&self, region: &str) -> Result<Option<lca_protocol::WidgetTree>, DispatchError> {
        if !self.worlds().contains(&World::Ui) || !self.inner.ui_regions.iter().any(|r| r == region)
        {
            if self.worlds().contains(&World::Ui) {
                // Declared the world but not this region: the denial is
                // recorded, the export is never called (catalog `ui`).
                self.inner.cap.note_ui_denial(region);
            }
            return Ok(None);
        }
        let region = region.to_string();
        // Frame-time call: the same blocking thread a registration call
        // uses (Wasmtime's sync WASI needs no runtime poll). A small
        // wasm component answers within the frame budget; ponytail:
        // measure with NFR-4's numbers if a heavy extension ever
        // misses it.
        self.blocking(move |inner| render_work(inner, &region))
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn on_ui_event(
        &self,
        region: &str,
        input: &lca_protocol::UiInput,
    ) -> Result<lca_protocol::UiEffect, DispatchError> {
        if !self.worlds().contains(&World::Ui) || !self.inner.ui_regions.iter().any(|r| r == region)
        {
            return Ok(lca_protocol::UiEffect::None);
        }
        let region = region.to_string();
        let input = input.clone();
        self.blocking(move |inner| event_work(inner, &region, &input))
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn interrupt(&self) {
        // FR-CONC-1: epoch interruption, independent of the fuel budget.
        self.inner.engine.increment_epoch();
    }
}
