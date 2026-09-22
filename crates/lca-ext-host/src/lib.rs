//! The WASM extension host: embeds Wasmtime, instantiates components,
//! wires host imports from the granted set, enforces resource limits, and
//! isolates traps (ADR-0001, ADR-0014, `docs/flows.md`).
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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lca_ext_abi::host::tool::{Tool, ToolPre};
use lca_protocol::{ToolCall, ToolResult, ToolResultStatus, ToolSpec};
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
}

impl Manifest {
    /// Parse and validate identity rules (the schema's `name` pattern is
    /// enforced here; full schema validation lands with Phase 5).
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
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || !name.starts_with(|c: char| c.is_ascii_lowercase())
            || name.len() < 2
            || name.len() > 64
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
        Ok(Manifest {
            name,
            version,
            abi,
            worlds,
        })
    }

    /// Whether the declared ABI line is inside the supported window
    /// (NFR-19, FR-EXT-8).
    pub fn abi_in_window(&self) -> bool {
        parse_abi(&self.abi).is_some_and(|declared| {
            parse_abi(lca_ext_abi::ABI_VERSION).is_some_and(|current| {
                declared <= current
                    && (current.1.saturating_sub(1), current.0) == (declared.1, declared.0)
                    || declared == current
                    || (declared.0 == current.0 && declared.1 + 1 == current.1)
            })
        })
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
    /// Linking or instantiation failed (a missing capability links out of
    /// the import table, so this is the deny-by-default failure path).
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
    /// Arguments failed the host-side shape check.
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
}

struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    logs: Arc<Mutex<Vec<String>>>,
    log_limit: usize,
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
        self.record(message);
    }
    fn debug(&mut self, message: String) {
        self.record(message);
    }
    fn info(&mut self, message: String) {
        self.record(message);
    }
    fn warn(&mut self, message: String) {
        self.record(message);
    }
    fn error(&mut self, message: String) {
        self.record(message);
    }
}

impl lca_ext_abi::host::tool::lca::ext::types::Host for HostState {}

impl HostState {
    fn record(&mut self, message: String) {
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

/// The host: one engine and linker shared by every extension load.
pub struct ExtHost {
    engine: Engine,
    linker: LinkerWithLimits,
    limits: ExtensionLimits,
}

// The generated linker type aliases the store data; keeping it behind one
// name keeps `load` readable.
type LinkerWithLimits = wasmtime::component::Linker<HostState>;

/// One loaded extension: either delivery mode exposes this shape, which is
/// the dispatch seam ADR-0013 names (FR-EXT-6's native twin registers
/// through the same calls).
pub struct WasmExtension {
    name: String,
    pre: ToolPre<HostState>,
    limits: ExtensionLimits,
    enabled: Arc<AtomicBool>,
    logs: Arc<Mutex<Vec<String>>>,
}

impl ExtHost {
    /// Build the host with its resource limits.
    pub fn new(limits: ExtensionLimits) -> ExtHost {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).expect("engine");
        let mut linker: LinkerWithLimits = Linker::<HostState>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).expect("wasi imports");
        Tool::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state).expect("world imports");
        ExtHost {
            engine,
            linker,
            limits,
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

    /// Load an extension: validate the manifest (identity and ABI window,
    /// FR-EXT-8), then link and probe-instantiate the component so a
    /// missing capability fails here, at load, instead of mid-session
    /// (FR-EXT-1, FR-EXT-2, `docs/flows.md`).
    pub fn load(&mut self, wasm: &[u8], manifest_text: &str) -> Result<WasmExtension, LoadError> {
        let manifest = Manifest::parse(manifest_text)?;
        if !manifest.abi_in_window() {
            return Err(LoadError::AbiUnsupported {
                declared: manifest.abi.clone(),
                window: SUPPORTED_ABI_WINDOW,
            });
        }
        let component = wasmtime::component::Component::new(&self.engine, wasm)
            .map_err(|err| LoadError::Link(err.to_string()))?;
        let pre = self
            .linker
            .instantiate_pre(&component)
            .map_err(|err| LoadError::Link(err.to_string()))?;
        let pre = ToolPre::new(pre).map_err(|err| LoadError::Link(err.to_string()))?;
        Ok(WasmExtension {
            name: manifest.name,
            pre,
            limits: self.limits.clone(),
            enabled: Arc::new(AtomicBool::new(true)),
            logs: Arc::new(Mutex::new(Vec::new())),
        })
    }
}

impl WasmExtension {
    /// The extension's identity.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether it is still enabled for this session (FR-EXT-7's data, and
    /// the disable bit FR-EXT-3/5 set).
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Everything this extension logged through the always-granted import,
    /// already truncated at the configured limit (FR-EXT-10).
    pub fn captured_logs(&self) -> Vec<String> {
        self.logs.lock().expect("log lock").clone()
    }

    /// Disable it for the session (the trap and limit paths).
    fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
    }

    /// The tool spec the extension registers.
    pub fn schema(&self) -> Result<ToolSpec, CallError> {
        let (mut store, instance) = self.instantiate()?;
        let schema = instance
            .lca_ext_tool_schema()
            .call_get_schema(&mut store)
            .map_err(|err| self.classify(err))?;
        let parameters = serde_json::from_str(&schema.parameters)
            .unwrap_or_else(|_| serde_json::json!({ "type": "object" }));
        Ok(ToolSpec {
            name: schema.name,
            description: schema.description,
            parameters,
            extras: Default::default(),
        })
    }

    /// Execute one call.
    pub fn execute(&self, call: &ToolCall) -> Result<ToolResult, CallError> {
        let (mut store, instance) = self.instantiate()?;
        let guest_call = lca_ext_abi::host::tool::lca::ext::types::ToolCall {
            call_id: call.call_id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            extras: Vec::new(),
        };
        let guest_result = instance
            .lca_ext_execute()
            .call_run(&mut store, &guest_call)
            .map_err(|err| self.classify(err))?;
        Ok(ToolResult {
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

    fn instantiate(&self) -> Result<(Store<HostState>, lca_ext_abi::host::tool::Tool), CallError> {
        if !self.is_enabled() {
            return Err(CallError::Disabled);
        }
        let mut store = Store::new(
            // The engine lives as long as the pre-instantiated component.
            self.pre.engine(),
            HostState {
                wasi: wasmtime_wasi::WasiCtx::builder().build(),
                table: ResourceTable::new(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(self.limits.memory_bytes)
                    .build(),
                logs: self.logs.clone(),
                log_limit: self.limits.log_limit_bytes,
            },
        );
        store.limiter(|state| &mut state.limits);
        // One epoch tick of grace: with epoch interruption enabled a store
        // starts already past its deadline, so give every fresh store one
        // tick and let cancellation consume it (ADR-0014: the host, not the
        // guest, decides when the deadline passes).
        store.set_epoch_deadline(1);
        store
            .set_fuel(self.limits.fuel_per_call)
            .map_err(|err| CallError::InvalidArguments(err.to_string()))?;
        let instance = self
            .pre
            .instantiate(&mut store)
            .map_err(|err| self.classify(err))?;
        Ok((store, instance))
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
