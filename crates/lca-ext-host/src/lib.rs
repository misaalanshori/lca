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
    DialogPrompt, GrantStore, OAuthSettings, PermissionPrompt, Proposals, ScopeGrant, ScopeRoots,
    SharedDialogs,
};
use lca_protocol::{
    CapabilityError, CommandEffect, CompletionRequest, DispatchError, EventSink, HookAction,
    IdentityOutcome, ModelInfo, PostToolObservation, ToolCall, ToolResultStatus, ToolSpec, Usage,
};
use lca_tools::{Capabilities, CapabilityGrants, Denial, ResourceSource};
use wasmtime::component::{HasSelf, Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

mod dispatch_hooks;
mod host_imports;
mod manifest;
mod provider;
mod tool_hooks;
mod transform;
mod ui;

pub use manifest::{LoadError, Manifest};
use provider::{
    IdentityOp, identity_simple_work, identity_usage_work, login_options_work, login_submit_work,
    provider_models_work, provider_stream_work,
};
use tool_hooks::{
    DispatchCommandSpec, InFlightGuard, catalog_specs_work, command_specs_work,
    execute_catalog_work, execute_work, invoke_work, schema_work,
};
use transform::{compact_work, transform_work};
use ui::{event_work, render_work};

/// Lock a mutex, recovering a poisoned guard rather than panicking.
///
/// A panic while another thread held the lock leaves it poisoned; refusing
/// to recover would take the whole agent down with a lock that is still
/// perfectly usable (S3: one poison-tolerant style everywhere).
pub(crate) fn lock<T: ?Sized>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The ABI lines this host loads: the current minor and the one before it
/// (NFR-19).
pub const SUPPORTED_ABI_WINDOW: &str = "0.5..=0.6, plus the 1.0 freeze line";

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
    /// The dialog prompter (gh #124): host-rendered questions backing
    /// the `ui-dialogs` import. Empty denies (the headless contract);
    /// the interface installs the live one per session.
    pub dialogs: SharedDialogs,
    /// The user grant store (ADR-0006).
    pub grant_store: Arc<Mutex<GrantStore>>,
    /// The current project, keyed into the grant store by canonical path.
    pub project: PathBuf,
    /// Permission proposals from a trusted project file.
    pub proposals: Option<Proposals>,
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
    /// A provider login submission failed (ADR-0033).
    #[error("login failed: {0}")]
    LoginFailed(String),
}

type HostLinker = Linker<HostState>;

struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    logs: Arc<Mutex<Vec<String>>>,
    log_limit: usize,
    cap: Arc<Capabilities>,
    dialogs: SharedDialogs,
    /// The tool registry surface (gh #77): installed per handle by
    /// the running turn, read by the `tools` import from blocking
    /// threads. Empty outside a turn; the import refuses without it.
    tools_view: Arc<Mutex<Option<Arc<dyn lca_ext_abi::ToolsRegistryView>>>>,
    /// The tool call executing on this store, when a guest calls the
    /// `tools` import: its id parents the nested call (gh #77).
    executing_call: Option<String>,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
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
    #[allow(clippy::expect_used)] // startup-fatal: a wasmtime engine that cannot build leaves nothing to run.
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

    /// One opt-in hooks world (gh #45): instantiate its pre when the
    /// manifest declares it, so a guest that never heard of the world
    /// loads exactly as before.
    fn hooks_pre<T>(
        pre: &wasmtime::component::InstancePre<HostState>,
        worlds: &[String],
        world: &str,
        construct: impl FnOnce(
            wasmtime::component::InstancePre<HostState>,
        ) -> Result<T, wasmtime::Error>,
    ) -> Result<Option<T>, LoadError> {
        worlds
            .iter()
            .any(|declared| declared == world)
            .then(|| construct(pre.clone()).map_err(|err| LoadError::Link(err.to_string())))
            .transpose()
    }

    /// Load an extension: validate the manifest (identity, capabilities,
    /// ABI window), resolve its grants, then link and probe-instantiate
    /// so a broken component fails here, before the first input
    /// (FR-EXT-1, FR-EXT-2, FR-EXT-8, `docs/flows.md`).
    #[allow(clippy::expect_used)] // `add_to_linker` only fails on a duplicate definition, which is a programming error.
    pub fn load(&mut self, wasm: &[u8], manifest_text: &str) -> Result<WasmExtension, LoadError> {
        let manifest = Manifest::parse(manifest_text)?;
        if !manifest.abi_in_window() {
            return Err(LoadError::AbiUnsupported {
                declared: manifest.abi.clone(),
                window: SUPPORTED_ABI_WINDOW,
            });
        }

        let mut cap = Capabilities::new(
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
                tools: manifest.tools,
            },
            self.env.roots.clone(),
            self.env.prompt.clone(),
            self.env.grant_store.clone(),
            self.env.project.clone(),
            self.env.proposals.clone(),
        );
        // ADR-0030/0032: the extension's own `resources/` bag, served from
        // the installed package directory. `None` when the package ships
        // none, so an extension without a bag lists empty rather than
        // erroring on a missing root. The seam is the same one a compiled-in
        // extension's embedded table uses.
        let resource_dir = self
            .env
            .roots
            .state_dir
            .join("extensions")
            .join(&manifest.name)
            .join("resources");
        if resource_dir.is_dir() {
            cap.set_resources(ResourceSource::Dir(resource_dir));
        }
        let cap = Arc::new(cap);

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
        let hooks_message = Self::hooks_pre(
            &pre,
            &manifest.worlds,
            "hooks-message",
            lca_ext_abi::host::hooks_message::HooksMessagePre::new,
        )?;
        let hooks_tool_call = Self::hooks_pre(
            &pre,
            &manifest.worlds,
            "hooks-tool-call",
            lca_ext_abi::host::hooks_tool_call::HooksToolCallPre::new,
        )?;
        let hooks_tool_result = Self::hooks_pre(
            &pre,
            &manifest.worlds,
            "hooks-tool-result",
            lca_ext_abi::host::hooks_tool_result::HooksToolResultPre::new,
        )?;
        let hooks_stream = Self::hooks_pre(
            &pre,
            &manifest.worlds,
            "hooks-stream",
            lca_ext_abi::host::hooks_stream::HooksStreamPre::new,
        )?;
        let hooks_settle = Self::hooks_pre(
            &pre,
            &manifest.worlds,
            "hooks-settle",
            lca_ext_abi::host::hooks_settle::HooksSettlePre::new,
        )?;
        let hooks_compaction = Self::hooks_pre(
            &pre,
            &manifest.worlds,
            "hooks-compaction",
            lca_ext_abi::host::hooks_compaction::HooksCompactionPre::new,
        )?;
        let hooks_cache = Self::hooks_pre(
            &pre,
            &manifest.worlds,
            "hooks-cache",
            lca_ext_abi::host::hooks_cache::HooksCachePre::new,
        )?;
        let hooks_trust = Self::hooks_pre(
            &pre,
            &manifest.worlds,
            "hooks-trust",
            lca_ext_abi::host::hooks_trust::HooksTrustPre::new,
        )?;
        let tool = manifest
            .worlds
            .iter()
            .any(|world| world == "tool")
            .then(|| ToolPre::new(pre.clone()).map_err(|err| LoadError::Link(err.to_string())))
            .transpose()?;
        let tool_catalog = manifest
            .worlds
            .iter()
            .any(|world| world == "tool-catalog")
            .then(|| {
                lca_ext_abi::host::tool_catalog::ToolCatalogPre::new(pre.clone())
                    .map_err(|err| LoadError::Link(err.to_string()))
            })
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
                tool_catalog,
                command,
                hooks,
                provider,
                compaction,
                context_transform,
                ui,
                ui_regions: manifest.ui_regions.clone(),
                manifest_text: manifest_text.to_string(),
                limits: effective_limits,
                enabled: Arc::new(AtomicBool::new(true)),
                in_flight: AtomicU64::new(0),
                interrupted: AtomicBool::new(false),
                logs: Arc::new(Mutex::new(Vec::new())),
                cap,
                dialogs: self.env.dialogs.clone(),
                tools_view: Arc::new(Mutex::new(None)),
                hooks_message,
                hooks_tool_call,
                hooks_tool_result,
                hooks_stream,
                hooks_settle,
                hooks_compaction,
                hooks_cache,
                hooks_trust,
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
    /// The multi-tool suite (gh #77): instantiated when the manifest
    /// declares `tool-catalog`. A guest declaring both worlds serves
    /// every suite tool through the catalog; the single-tool world
    /// stays the legacy path.
    tool_catalog: Option<lca_ext_abi::host::tool_catalog::ToolCatalogPre<HostState>>,
    /// One pre-instance per new hooks world (gh #45), each opt-in by
    /// manifest declaration; the six-point `hooks` world is untouched.
    hooks_message: Option<lca_ext_abi::host::hooks_message::HooksMessagePre<HostState>>,
    hooks_tool_call: Option<lca_ext_abi::host::hooks_tool_call::HooksToolCallPre<HostState>>,
    hooks_tool_result: Option<lca_ext_abi::host::hooks_tool_result::HooksToolResultPre<HostState>>,
    hooks_stream: Option<lca_ext_abi::host::hooks_stream::HooksStreamPre<HostState>>,
    hooks_settle: Option<lca_ext_abi::host::hooks_settle::HooksSettlePre<HostState>>,
    hooks_compaction: Option<lca_ext_abi::host::hooks_compaction::HooksCompactionPre<HostState>>,
    hooks_cache: Option<lca_ext_abi::host::hooks_cache::HooksCachePre<HostState>>,
    hooks_trust: Option<lca_ext_abi::host::hooks_trust::HooksTrustPre<HostState>>,
    command: Option<lca_ext_abi::host::command::CommandPre<HostState>>,
    hooks: Option<lca_ext_abi::host::hooks::HooksPre<HostState>>,
    provider: Option<ProviderPre<HostState>>,
    compaction: Option<CompactionPre<HostState>>,
    context_transform: Option<ContextTransformPre<HostState>>,
    ui: Option<UiPre<HostState>>,
    /// The manifest's granted ui regions (the host only asks these).
    ui_regions: Vec<String>,
    /// The `extension.toml` text this handle loaded with (gh #157):
    /// the host reads manifest-declared provider needs off it.
    manifest_text: String,
    limits: ExtensionLimits,
    enabled: Arc<AtomicBool>,
    logs: Arc<Mutex<Vec<String>>>,
    cap: Arc<Capabilities>,
    /// The dialog prompter (gh #124): cloned into every store so the
    /// `ui-dialogs` import answers through the session's live slot.
    dialogs: SharedDialogs,
    /// The tool registry surface (gh #77): the running turn
    /// installs it per handle; every store shares the slot, so the
    /// `tools` import lists, activates, and nests through it.
    tools_view: Arc<Mutex<Option<Arc<dyn lca_ext_abi::ToolsRegistryView>>>>,
    /// Calls currently inside the guest: incremented the moment the
    /// component is instantiated and the run begins, decremented when
    /// it returns. Cancellation semantics are about interrupting a
    /// *running* call, so a test that must know the guest is running
    /// (rather than still building its store) watches this.
    in_flight: AtomicU64,
    /// Set when this extension is interrupted during a turn (FR-CONC-1).
    /// A bump that lands while the next store is still being built is
    /// absorbed by `set_epoch_deadline`, whose target is measured from
    /// the epoch at set time, so `build_store` re-arms it; the flag then
    /// holds for the rest of the turn and is cleared by
    /// `turn_started`. Without it, a cancel that races call setup is
    /// lost and the guest spins (the 2026-10-01 Windows CI hang).
    interrupted: AtomicBool,
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
                dialogs: self.dialogs.clone(),
                tools_view: self.tools_view.clone(),
                executing_call: None,
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
        // Re-arm an interrupt that landed while this store was being
        // built: its epoch bump was absorbed by the deadline set above
        // (`current_epoch + 1` from now), so without this the call would
        // never trap. The check after the set closes the race for any
        // interleaving with `interrupt` (FR-CONC-1).
        if self.interrupted.load(Ordering::SeqCst) {
            self.engine.increment_epoch();
        }
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
        lock(&self.inner.logs).clone()
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
