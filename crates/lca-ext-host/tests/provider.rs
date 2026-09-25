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
// every scripted event kind, FR-PROV-7's start-before-delta shape, and
// NFR-21 - the committed component's scripted output is a golden fixture, so
// a pinned-runtime upgrade that changes extension behavior fails here even
// inside one ABI version (the Wasmtime version is pinned in Cargo.lock).
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

// Verifies: ADR-0012 — logout and usage export in both modes with
// identical outcomes, including the not-supported path. (`login` is
// exercised with callback injection in `oauth_and_credentials_match_across_modes`.)
#[tokio::test]
async fn identity_exports_are_identical_across_modes() {
    let fixture = Fixture::new("identity");
    let (wasm, native) = fixture.both_modes();

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

// Verifies: NFR-25, FR-PERM-6/FR-PERM-7 (credentials), FR-PROV-3/FR-PROV-4
// (the loopback oauth flow) at the WASM boundary: the conformance probe's
// `login` runs `credentials.set/get/delete` and
// `oauth.begin/open/await-callback/end-flow` through the host imports, and
// the native twin reaches the same outcome. The test injects the callback
// itself (the guest cannot reach the loopback port).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_and_credentials_match_across_modes() {
    let fixture = Fixture::new("identity-oauth");
    let wasm = fixture.wasm_mode();
    let cap = wasm.capabilities();
    // The browser launcher is a recorder, so the host opens nothing.
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorder = opened.clone();
    cap.set_browser_opener(Some(Arc::new(move |url| {
        recorder.lock().expect("opened").push(url.to_string());
        Ok(())
    })));
    let native = conformance::NativeConformance::new(cap.clone());

    let wasm_login = login_with_callback(&wasm, &cap).await;
    assert_eq!(wasm_login, IdentityOutcome::Ok, "wasm login");
    let native_login = login_with_callback(&native, &cap).await;
    assert_eq!(native_login, IdentityOutcome::Ok, "native login");
    assert_eq!(wasm_login, native_login);

    // The probe set then deleted its credential: the namespace file has no
    // trace of it (FR-PERM-6/FR-PERM-7).
    let creds = fixture.root.join("data/credentials/conformance.json");
    let text = std::fs::read_to_string(&creds).unwrap_or_default();
    assert!(
        !text.contains("probe"),
        "credential survived delete: {text}"
    );

    // `oauth.open` recorded the authorization URL (FR-PROV-3's browser half).
    let opened = opened.lock().expect("opened");
    assert_eq!(opened.len(), 2, "one open per login: {opened:?}");
    assert!(opened.iter().all(|url| url == conformance::AUTHORIZE_URL));
    assert_eq!(
        cap.oauth_opened(),
        vec![
            conformance::AUTHORIZE_URL.to_string(),
            conformance::AUTHORIZE_URL.to_string()
        ]
    );
}

// Verifies: NFR-21 (cancellation reaches a running extension call) at the
// blocking host boundary: an extension waiting in `oauth.await-callback`
// returns promptly when the host interrupts it, instead of sitting until the
// callback window elapses. An epoch bump alone cannot reach blocked host
// code, so this is the case that closes NFR-21 for host waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupting_a_blocked_oauth_wait_returns_promptly() {
    let fixture = Fixture::new("oauth-cancel");
    let wasm = fixture.wasm_mode();
    let cap = wasm.capabilities();

    let before = cap.oauth_begun().len();
    let task = tokio::spawn(wasm.identity_login());
    // Wait until the login is actually blocked in `oauth_await` (its flow has
    // bound the listener); interrupting earlier would trap at the call site
    // and prove nothing about the blocked wait.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while cap.oauth_begun().len() == before {
        assert!(
            std::time::Instant::now() < deadline,
            "the login never bound an oauth flow"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let start = std::time::Instant::now();
    wasm.interrupt();
    let joined = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("the cancelled login returns in under 5s, not the 30s window")
        .expect("the login task joins");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "cancellation returns within the NFR-21 neighbourhood, took {:?}",
        start.elapsed()
    );
    // Either the guest handled the cancelled host call (`Failed`) or the
    // epoch trap surfaced as a dispatch error; both are prompt returns.
    assert!(
        matches!(joined, Err(_) | Ok(IdentityOutcome::Failed(_))),
        "the cancelled login did not report a cancellation: {joined:?}"
    );
}

/// Drive one identity-login call to completion: spawn it (the call blocks in
/// `oauth.await-callback`), wait for the flow to bind, inject the loopback
/// callback, and return the outcome.
async fn login_with_callback(
    handle: &dyn ExtensionDispatch,
    cap: &lca_tools::Capabilities,
) -> IdentityOutcome {
    let before = cap.oauth_begun().len();
    let login = handle.identity_login();
    let task = tokio::spawn(login);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let url = loop {
        if let Some(url) = cap.oauth_begun().get(before) {
            break url.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the oauth flow never bound a callback URL"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    inject_callback(&url, conformance::FIXTURE_CODE, "fixture-state");
    task.await.expect("login task").expect("login dispatch")
}

/// One loopback GET with no dependency: the host's listener only reads the
/// request line, so no response read is needed.
fn inject_callback(url: &str, code: &str, state: &str) {
    use std::io::Write;
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .expect("the flow binds a loopback URL");
    let (port, path) = rest
        .split_once('/')
        .map(|(port, tail)| (port, format!("/{tail}")))
        .expect("a redirect path");
    let port = port.parse::<u16>().expect("a bound port");
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect the callback listener");
    let request = format!(
        "GET {path}?code={code}&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .expect("write the callback request");
    let _ = stream.flush();
}

/// A native capability engine with exactly the identity grants asked for.
fn capabilities_with(
    fixture: &Fixture,
    credentials: bool,
    oauth: bool,
) -> Arc<lca_tools::Capabilities> {
    Arc::new(lca_tools::Capabilities::new(
        "conformance",
        lca_tools::CapabilityGrants {
            credentials,
            oauth: oauth.then(|| lca_permissions::OAuthSettings {
                redirect_path: "/callback".to_string(),
                timeout_seconds: 30,
            }),
            ..Default::default()
        },
        fixture.roots.clone(),
        fixture.prompt.clone(),
        fixture.store.clone(),
        fixture.root.join("project"),
        None,
    ))
}

/// The conformance manifest with the identity grants stripped: the denied
/// state ADR-0026 links in.
fn manifest_without_identity_grants() -> String {
    let mut text = MANIFEST.to_string();
    for block in [
        "[capabilities.net]\nhosts = [\"conformance.example.com\"]\n\n",
        "[capabilities.oauth]\nredirect_path = \"/callback\"\ntimeout_seconds = 30\n\n",
        "[capabilities.credentials]\nnamespace = \"conformance\"\n\n",
    ] {
        text = text.replace(block, "");
    }
    text
}

// Verifies: ADR-0026 + NFR-25 — the denied state is a refusal, not a crash,
// at both boundaries, and each refusal is recorded as a denial. Closes the
// last NFR-25 surface (the credentials and oauth host imports).
#[tokio::test]
async fn denied_identity_capabilities_refuse_at_both_boundaries() {
    let fixture = Fixture::new("identity-denied");

    // Native: credentials undeclared refuses at the first call.
    let no_creds = conformance::NativeConformance::new(capabilities_with(&fixture, false, true));
    assert!(
        matches!(
            no_creds.identity_login().await.expect("dispatch"),
            IdentityOutcome::Failed(_)
        ),
        "an undeclared credentials capability refuses"
    );

    // Native: credentials declared but oauth undeclared refuses at the flow.
    let no_oauth = conformance::NativeConformance::new(capabilities_with(&fixture, true, false));
    assert!(
        matches!(
            no_oauth.identity_login().await.expect("dispatch"),
            IdentityOutcome::Failed(_)
        ),
        "an undeclared oauth capability refuses"
    );

    // WASM: the same manifest without the grants refuses through the imports
    // and records a denial.
    let wasm = {
        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            fixture.env(),
        );
        host.load(FIXTURE, &manifest_without_identity_grants())
            .expect("load denied mode")
    };
    let outcome = wasm.identity_login().await.expect("wasm dispatch");
    assert!(
        matches!(outcome, IdentityOutcome::Failed(_)),
        "the denied WASM call refuses: {outcome:?}"
    );
    assert!(
        wasm.denial_count() > 0,
        "the refusal is recorded as a denial (FR-EXT-9)"
    );
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

/// A committed first-party component must export every world its manifest
/// declares: the manifest schema says the host verifies each one, and a
/// drift is exactly the defect that shipped an `antigravity` artifact the
/// host refused to link (`no exported instance named
/// lca:ext/command-spec@1.0.0`). The loader's own world-link step is the
/// assertion - a component missing a declared world fails to load.
#[test]
fn every_first_party_component_exports_the_worlds_its_manifest_declares() {
    let cases: [(&str, &[u8], &str); 2] = [
        (
            "openai-compatible",
            include_bytes!("../../../extensions/openai-compatible/fixtures/component.wasm"),
            include_str!("../../../extensions/openai-compatible/extension.toml"),
        ),
        (
            "antigravity",
            include_bytes!("../../../extensions/antigravity/fixtures/component.wasm"),
            include_str!("../../../extensions/antigravity/extension.toml"),
        ),
    ];
    for (name, bytes, manifest) in cases {
        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            Fixture::new(&format!("manifest-{name}")).env(),
        );
        let handle = host
            .load(bytes, manifest)
            .unwrap_or_else(|err| panic!("`{name}` must link against its manifest: {err}"));
        assert!(
            handle.worlds().contains(&World::Provider),
            "`{name}` exports the provider world"
        );
    }
}
