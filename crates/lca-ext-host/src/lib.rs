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
use lca_tools::{Capabilities, CapabilityGrants, Denial, ResourceSource};
use wasmtime::component::{HasSelf, Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

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
    DispatchCommandSpec, InFlightGuard, command_specs_work, execute_work, invoke_work,
    observe_work, pre_tool_work, schema_work, session_close_work,
};
use transform::{compact_work, transform_work};
use ui::{event_work, render_work};

/// The ABI lines this host loads: the current minor and the one before it
/// (NFR-19).
pub const SUPPORTED_ABI_WINDOW: &str = "0.1..=0.2, plus the 1.0 freeze line";

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

    fn provider_models(
        &self,
        settings: &[(String, String)],
    ) -> Result<Vec<ModelInfo>, DispatchError> {
        if !self.worlds().contains(&World::Provider) {
            return Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "provider",
            });
        }
        let settings = settings.to_vec();
        self.blocking(move |inner| provider_models_work(inner, &settings))
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

    fn login_options(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<Vec<lca_protocol::LoginOption>, DispatchError>>
    {
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            // A provider without the login surface has no options; the host
            // still shows its own "Custom endpoint…" entry.
            if !inner.worlds.contains(&"provider".to_string()) {
                return Ok(Vec::new());
            }
            pool_call(inner, login_options_work)
                .await
                .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn login_submit(
        &self,
        answer: lca_protocol::LoginAnswer,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<Vec<(String, String)>, DispatchError>> {
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"provider".to_string()) {
                return Ok(Vec::new());
            }
            pool_call(inner, move |inner| login_submit_work(inner, answer))
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
        // An epoch bump only fires at a guest code point, so a host import
        // blocked in a long wait would never see it: flag the capability
        // engine too, and the wait polls its way out (FR-CONC-1, NFR-21).
        self.inner.cap.cancel();
    }
}
