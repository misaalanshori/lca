//! The one `Fixture` every provider row shares: its seeded sandbox, the two
//! delivery modes built over it, the event sinks, and the conformance artifacts.
//!
//! Gh #552 split `tests/provider.rs` (it was **at** gate 11's 1,200-line ceiling)
//! and this is the part a copy would have duplicated. Every `ExtHost::load` costs
//! ~6 s (`Component::new` dominates) and nextest forks a process per row, so a
//! fixture copy per file would multiply the suite's wall time rather than share
//! it - the defect `tests/regressions/449` exists to prevent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_ext_abi::ExtensionDispatch;
use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};
use lca_ext_native::NativeRegistry;
use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots,
};
use lca_protocol::{CompletionRequest, EventSink, StreamEvent};

/// The conformance fixture both delivery modes load. It moved into `support.rs`
/// with the `Fixture` that seeds the resource bag it reads (gh #552).
pub(super) const FIXTURE: &[u8] =
    include_bytes!("../../../../extensions/conformance/fixtures/tool-world.wasm");
pub(super) const MANIFEST: &str = include_str!("../../../../extensions/conformance/extension.toml");

pub(super) struct Always;
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
pub(super) struct Collect(Arc<Mutex<Vec<StreamEvent>>>);

impl EventSink for Collect {
    fn push(&self, event: StreamEvent) -> bool {
        self.0.lock().expect("events").push(event);
        true
    }
}

/// Closes after `budget` pushes: the receiver-gone case (FR-CONC-3).
pub(super) struct CloseAfter {
    pub(super) events: Arc<Mutex<Vec<StreamEvent>>>,
    pub(super) budget: std::sync::atomic::AtomicUsize,
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

pub(super) struct Fixture {
    pub(super) root: std::path::PathBuf,
    pub(super) prompt: Arc<Mutex<Always>>,
    pub(super) store: Arc<Mutex<GrantStore>>,
    pub(super) roots: ScopeRoots,
}

impl Fixture {
    pub(super) fn new(name: &str) -> Fixture {
        let root = lca_testkit::scratch_path(&format!("lca-provider-{name}"));
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
        // Gh #314: the provider probe reads its own resource bag in both
        // modes, so the fixture seeds the directory the host serves
        // (`<state>/extensions/<name>/resources`, ADR-0030).
        let resources = root.join("data/extensions/conformance/resources/probe");
        std::fs::create_dir_all(&resources).expect("mkdir probe resources");
        std::fs::write(
            resources.join("provider.txt"),
            conformance::PROBE_RESOURCE_BODY.as_bytes(),
        )
        .expect("seed the probe resource");
        Fixture {
            store: Arc::new(Mutex::new(
                GrantStore::open(&root.join("grants.json")).expect("store"),
            )),
            prompt: Arc::new(Mutex::new(Always)),
            roots,
            root,
        }
    }

    pub(super) fn env(&self) -> Arc<HostEnvironment> {
        Arc::new(HostEnvironment {
            roots: self.roots.clone(),
            prompt: self.prompt.clone(),
            grant_store: self.store.clone(),
            project: self.root.join("project"),
            proposals: None,
            dialogs: lca_permissions::SharedDialogs::default(),
        })
    }

    pub(super) fn capabilities(&self) -> Arc<lca_tools::Capabilities> {
        let mut cap = lca_tools::Capabilities::new(
            "conformance",
            lca_tools::CapabilityGrants {
                fs: vec![
                    ScopeGrant::parse("workspace", lca_permissions::FsMode::ReadWrite)
                        .expect("grant"),
                ],
                fs_declared: true,
                process: true,
                pty: true,
                net: vec![
                    lca_permissions::parse_net_pattern("conformance.example.com")
                        .expect("net pattern"),
                ],
                oauth: Some(lca_permissions::OAuthSettings {
                    redirect_path: "/callback".to_string(),
                    timeout_seconds: 30,
                }),
                credentials: true,
                ..Default::default()
            },
            self.roots.clone(),
            self.prompt.clone(),
            self.store.clone(),
            self.root.join("project"),
            None,
        );
        // Gh #314: the native twin reads the SAME bag the WASM host serves,
        // so the two modes' capability probe compares like with like.
        cap.set_resources(lca_tools::ResourceSource::Dir(
            self.root.join("data/extensions/conformance/resources"),
        ));
        Arc::new(cap)
    }

    pub(super) fn wasm_mode(&self) -> lca_ext_host::WasmExtension {
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

    pub(super) fn both_modes(&self) -> (Arc<dyn ExtensionDispatch>, Arc<dyn ExtensionDispatch>) {
        let wasm = self.wasm_mode();
        let native = conformance::NativeConformance::new(self.capabilities());
        let mut registry = NativeRegistry::new();
        registry.register(Arc::new(wasm));
        registry.register(Arc::new(native));
        let handles = registry.into_handles();
        (handles[0].clone(), handles[1].clone())
    }
}

pub(super) fn request(model: &str) -> CompletionRequest {
    CompletionRequest {
        messages: Vec::new(),
        tools: Vec::new(),
        model: model.to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

pub(super) async fn collect(
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
