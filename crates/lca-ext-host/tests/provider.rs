//! The provider world (Phase 3) in both delivery modes: identical
//! scripted results, case-per-case event mapping, FR-PROV-7/8 shapes,
//! and ADR-0012's three identity exports (NFR-25's Phase 3 half).

use std::sync::{Arc, Mutex};

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};
use lca_ext_native::NativeRegistry;
use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots,
};
use lca_protocol::{CompletionRequest, EventSink, IdentityOutcome, StreamEvent};
use lca_provider::ToolCallAccumulator;

const FIXTURE: &[u8] = include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm");
const MANIFEST: &str = include_str!("../../../extensions/conformance/extension.toml");

struct Always;
impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// Collects every event it is handed (the ordinary consumer).
#[derive(Clone, Default)]
struct Collect(Arc<Mutex<Vec<StreamEvent>>>);

impl EventSink for Collect {
    fn push(&self, event: StreamEvent) -> bool {
        self.0.lock().expect("events").push(event);
        true
    }
}

/// Closes after `budget` pushes: the receiver-gone case (FR-CONC-3).
struct CloseAfter {
    events: Arc<Mutex<Vec<StreamEvent>>>,
    budget: std::sync::atomic::AtomicUsize,
}

impl EventSink for CloseAfter {
    fn push(&self, event: StreamEvent) -> bool {
        use std::sync::atomic::Ordering;
        if self.budget.fetch_sub(1, Ordering::SeqCst) == 0 {
            return false;
        }
        self.events.lock().expect("events").push(event);
        true
    }
}

struct Fixture {
    root: std::path::PathBuf,
    prompt: Arc<Mutex<Always>>,
    store: Arc<Mutex<GrantStore>>,
    roots: ScopeRoots,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("lca-provider-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["project", "private", "config", "data", "tmp"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        let roots = ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        };
        Fixture {
            store: Arc::new(Mutex::new(
                GrantStore::open(&root.join("grants.json")).expect("store"),
            )),
            prompt: Arc::new(Mutex::new(Always)),
            roots,
            root,
        }
    }

    fn env(&self) -> Arc<HostEnvironment> {
        Arc::new(HostEnvironment {
            roots: self.roots.clone(),
            prompt: self.prompt.clone(),
            grant_store: self.store.clone(),
            project: self.root.join("project"),
            proposals: None,
        })
    }

    fn capabilities(&self) -> Arc<lca_tools::Capabilities> {
        Arc::new(lca_tools::Capabilities::new(
            "conformance",
            lca_tools::CapabilityGrants {
                fs: vec![
                    ScopeGrant::parse("workspace", lca_permissions::FsMode::ReadWrite)
                        .expect("grant"),
                ],
                fs_declared: true,
                process: true,
                pty: true,
                ..Default::default()
            },
            self.roots.clone(),
            self.prompt.clone(),
            self.store.clone(),
            self.root.join("project"),
            None,
        ))
    }

    fn wasm_mode(&self) -> lca_ext_host::WasmExtension {
        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            self.env(),
        );
        host.load(FIXTURE, MANIFEST).expect("load wasm mode")
    }

    fn both_modes(&self) -> (Arc<dyn ExtensionDispatch>, Arc<dyn ExtensionDispatch>) {
        let wasm = self.wasm_mode();
        let native = conformance::NativeConformance::new(self.capabilities());
        let mut registry = NativeRegistry::new();
        registry.register(Arc::new(wasm));
        registry.register(Arc::new(native));
        let handles = registry.into_handles();
        (handles[0].clone(), handles[1].clone())
    }
}

fn request(model: &str) -> CompletionRequest {
    CompletionRequest {
        messages: Vec::new(),
        tools: Vec::new(),
        model: model.to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

async fn collect(
    handle: &dyn ExtensionDispatch,
    model: &str,
) -> Result<Vec<StreamEvent>, lca_protocol::DispatchError> {
    let sink = Collect::default();
    let events = sink.0.clone();
    handle
        .stream_completion(request(model), &sink)
        .await
        .map(|()| events.lock().expect("events").clone())
}

// Verifies: FR-PROV-2 — both modes list the same models, and they are
// the shared script's models (NFR-25 identity for model listing).
#[tokio::test]
async fn model_listing_is_identical_across_modes() {
    let fixture = Fixture::new("models");
    let (wasm, native) = fixture.both_modes();

    assert_eq!(wasm.worlds(), native.worlds());
    assert!(wasm.worlds().contains(&World::Provider));
    assert_eq!(wasm.delivery(), DeliveryMode::Wasm);
    assert_eq!(native.delivery(), DeliveryMode::Native);

    let wasm_models = wasm.provider_models().expect("wasm models");
    let native_models = native.provider_models().expect("native models");
    assert_eq!(wasm_models, native_models);
    assert_eq!(wasm_models, conformance::provider_models());
    assert_eq!(wasm_models.len(), 2);
    assert_eq!(wasm_models[0].id, "conformance-a");
    assert_eq!(wasm_models[1].context_window, 8192);
}

// Verifies: ADR-0004's typed stream, NFR-25's exact-equality promise for
// every scripted event kind, and FR-PROV-7's start-before-delta shape.
#[tokio::test]
async fn streamed_events_match_the_script_and_each_other() {
    let fixture = Fixture::new("stream");
    let (wasm, native) = fixture.both_modes();

    for model in [
        "",
        "conformance-defaults",
        "conformance-tool",
        "conformance-vendor",
        "conformance-error",
    ] {
        let wasm_events = collect(wasm.as_ref(), model).await.expect("wasm stream");
        let native_events = collect(native.as_ref(), model)
            .await
            .expect("native stream");
        assert_eq!(
            wasm_events, native_events,
            "dual-mode divergence for model {model:?}"
        );
        assert_eq!(
            wasm_events,
            conformance::scripted_events(model),
            "host mapping lost fidelity for model {model:?}"
        );
    }

    // FR-PROV-7: the start event precedes every argument delta.
    let events = collect(wasm.as_ref(), "conformance-tool")
        .await
        .expect("tool stream");
    let mut saw_start = false;
    for event in &events {
        match event {
            StreamEvent::ToolCallStart { .. } => saw_start = true,
            StreamEvent::ToolCallArgDelta { .. } => assert!(saw_start, "delta before start"),
            _ => {}
        }
    }
    assert!(saw_start, "the tool script opened a call");
}

// Verifies: FR-PROV-8 — an argument delta with no open start is
// discarded by the host's accumulator and recorded as a protocol error.
#[tokio::test]
async fn orphan_deltas_become_protocol_errors() {
    let fixture = Fixture::new("orphan");
    let (wasm, _native) = fixture.both_modes();

    let events = collect(wasm.as_ref(), "conformance-orphan-delta")
        .await
        .expect("orphan stream");
    let mut accumulator = ToolCallAccumulator::default();
    for event in events {
        accumulator.handle(event);
    }
    let (calls, errors) = accumulator.finish(true);
    assert!(calls.is_empty(), "the orphan call never materializes");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        matches!(
            errors[0],
            lca_provider::ProtocolError::DeltaWithoutStart { ref call_id } if call_id == "ghost"
        ),
        "{errors:?}"
    );
}

// Verifies: ADR-0012 — login, logout, and usage export in both modes
// with identical outcomes, including the not-supported path.
#[tokio::test]
async fn identity_exports_are_identical_across_modes() {
    let fixture = Fixture::new("identity");
    let (wasm, native) = fixture.both_modes();

    let wasm_login = wasm.identity_login().await.expect("wasm login");
    let native_login = native.identity_login().await.expect("native login");
    assert_eq!(wasm_login, native_login);
    assert_eq!(wasm_login, IdentityOutcome::Ok);

    let wasm_logout = wasm.identity_logout().await.expect("wasm logout");
    let native_logout = native.identity_logout().await.expect("native logout");
    assert_eq!(wasm_logout, native_logout);
    assert_eq!(
        wasm_logout,
        IdentityOutcome::NotSupported,
        "ADR-0012's optional-export path"
    );

    let wasm_usage = wasm.identity_usage().await.expect("wasm usage");
    let native_usage = native.identity_usage().await.expect("native usage");
    assert_eq!(wasm_usage, native_usage);
    let usage = wasm_usage.expect("conformance reports usage");
    let scripted = conformance::scripted_usage_report();
    assert_eq!(usage.input, scripted.input);
    assert_eq!(usage.cache_read, scripted.cache_read);
    assert_eq!(usage.cost, scripted.cost);
}

// Verifies: FR-CONC-3 — closing the receiver ends the stream without
// disabling the extension (cancellation is not a defect by the
// extension, FR-EXT-3 does not fire).
#[tokio::test]
async fn a_closed_receiver_ends_the_stream_and_keeps_the_extension() {
    let fixture = Fixture::new("cancel");

    let wasm = fixture.wasm_mode();
    let sink = CloseAfter {
        events: Arc::new(Mutex::new(Vec::new())),
        budget: std::sync::atomic::AtomicUsize::new(1),
    };
    let _ = wasm
        .stream_completion(request("conformance-defaults"), &sink)
        .await;
    assert!(
        wasm.is_enabled(),
        "a receiver that walked away must not disable the extension"
    );

    let native = conformance::NativeConformance::new(fixture.capabilities());
    let sink = CloseAfter {
        events: Arc::new(Mutex::new(Vec::new())),
        budget: std::sync::atomic::AtomicUsize::new(1),
    };
    let _ = native
        .stream_completion(request("conformance-defaults"), &sink)
        .await;
}
