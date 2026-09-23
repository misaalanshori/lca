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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lca_ext_abi::host::tool::{Tool, ToolPre};
use lca_ext_abi::{DeliveryMode, World};
use lca_permissions::{GrantStore, PermissionPrompt, Proposals, ScopeGrant, ScopeRoots};
use lca_protocol::{
    CapabilityError, CommandEffect, DispatchError, HookAction, PostToolObservation, ToolCall,
    ToolResultStatus, ToolSpec,
};
use lca_tools::{Capabilities, CapabilityGrants, Denial};
use wasmtime::component::{HasSelf, Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

/// The ABI lines this host loads: the current minor and the one before it
/// (NFR-19).
pub const SUPPORTED_ABI_WINDOW: &str = "0.0..=0.1";

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
}

/// The `oauth` capability's manifest parameters (capability catalog).
#[derive(Debug, Clone)]
pub struct OAuthSettings {
    /// The redirect path the authorization server may use.
    pub redirect_path: String,
    /// Seconds to wait for the loopback callback (300 default, the
    /// catalog's value).
    pub timeout_seconds: u64,
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
        if let Some(capabilities) = value.get("capabilities") {
            let table = capabilities.as_table().ok_or_else(|| {
                LoadError::InvalidManifest("`capabilities` must be a table".into())
            })?;
            for key in table.keys() {
                match key.as_str() {
                    "fs" | "process" | "pty" | "net" | "net-local" | "oauth" | "credentials" => {}
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
        declared_major == current_major
            && (declared_minor == current_minor
                || (current_minor > 0 && declared_minor == current_minor - 1))
    }
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
            },
            self.env.roots.clone(),
            self.env.prompt.clone(),
            self.env.grant_store.clone(),
            self.env.project.clone(),
            self.env.proposals.clone(),
        ));

        let component = wasmtime::component::Component::new(&self.engine, wasm)
            .map_err(|err| LoadError::Link(err.to_string()))?;
        let mut linker: HostLinker = Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).expect("wasi imports");
        Tool::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state).expect("world imports");
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
        let hooks = manifest
            .worlds
            .iter()
            .any(|world| world == "hooks")
            .then(|| {
                lca_ext_abi::host::hooks::HooksPre::new(pre)
                    .map_err(|err| LoadError::Link(err.to_string()))
            })
            .transpose()?;

        Ok(WasmExtension {
            inner: Arc::new(Inner {
                engine: self.engine.clone(),
                name: manifest.name,
                worlds: manifest.worlds.clone(),
                tool,
                command,
                hooks,
                limits: self.limits.clone(),
                enabled: Arc::new(AtomicBool::new(true)),
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
    limits: ExtensionLimits,
    enabled: Arc<AtomicBool>,
    logs: Arc<Mutex<Vec<String>>>,
    cap: Arc<Capabilities>,
}

impl Inner {
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

/// One loaded extension (ADR-0019's handle for the WASM mode).
pub struct WasmExtension {
    inner: Arc<Inner>,
}

/// Run blocking component work on the runtime's blocking pool; a panic
/// inside the call disables the extension instead of the caller
/// (FR-EXT-3).
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
    /// runtime is polling (ADR-0014's blocking-region rule). A panic
    /// inside the call disables the extension instead of unwinding the
    /// caller (FR-EXT-3).
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

    fn interrupt(&self) {
        // FR-CONC-1: epoch interruption, independent of the fuel budget.
        self.inner.engine.increment_epoch();
    }
}
