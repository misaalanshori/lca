//! The provider extension against the capability engine: credentials
//! through `credentials` only, transport through `net` only, and the
//! ADR-0012 identity trio (Phase 3 exit-test half: an API-key provider
//! that never touches a socket or a credential file directly).

use std::sync::{Arc, Mutex};

use lca_ext_abi::ExtensionDispatch;
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::{CompletionRequest, IdentityOutcome};

struct Always;
impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// One sandbox: temp roots, the manifest's own grants plus an ad hoc
/// grant for this test's loopback server (the Phase 5 install flow
/// attaches those in production; here the test stands in for the user's
/// consent, FR-PERM-16's shape).
fn sandbox(name: &str, adhoc_loopback: bool) -> Arc<lca_tools::Capabilities> {
    let root = std::env::temp_dir().join(format!("lca-openai-{name}-{}", std::process::id()));
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
    let mut grants = openai_compatible::manifest_grants();
    if adhoc_loopback {
        grants
            .adhoc_net
            .push(lca_permissions::parse_net_pattern("127.0.0.1").expect("loopback pattern"));
    }
    Arc::new(lca_tools::Capabilities::new(
        "openai-compatible",
        grants,
        roots,
        Arc::new(Mutex::new(Always)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    ))
}

fn settings_for(base_url: &str, key: Option<&str>) -> openai_compatible::Settings {
    openai_compatible::Settings {
        base_url: base_url.to_string(),
        api_key: key.map(str::to_string),
        model: "test-model".to_string(),
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

/// A loopback HTTP server that answers with one canned SSE body.
fn sse_server(body: &'static str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for stream in listener.incoming().flatten().take(1) {
            let mut stream = stream;
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    format!("http://{addr}")
}

// Verifies: FR-PERM-6 — login writes the key through the credential
// namespace only; logout clears it; a stored key counts as logged in
// before the environment is consulted again.
#[test]
fn login_and_logout_move_the_key_through_credentials_only() {
    let cap = sandbox("identity", false);
    let settings = settings_for("https://api.openai.com/v1", Some("sk-env-key"));

    // Nothing stored yet; the environment's key promotes on login.
    assert!(openai_compatible::run_login(cap.as_ref(), &settings) == IdentityOutcome::Ok);
    assert_eq!(
        cap.credentials_get("api_key").expect("credential store"),
        Some("sk-env-key".to_string())
    );

    // A second login with no environment key still reports success,
    // because the credential store already has one.
    let empty = settings_for("https://api.openai.com/v1", None);
    assert_eq!(
        openai_compatible::run_login(cap.as_ref(), &empty),
        IdentityOutcome::Ok
    );

    assert_eq!(
        openai_compatible::run_logout(cap.as_ref()),
        IdentityOutcome::Ok
    );
    assert!(
        cap.credentials_get("api_key")
            .expect("credential store")
            .is_none(),
        "key cleared"
    );
}

// Verifies: ADR-0012's optional-export rule for this provider — no
// environment key and nothing stored is an honest failure that says
// what to do, and `usage` is genuinely not supported.
#[test]
fn login_without_a_key_says_what_to_do_and_usage_is_not_supported() {
    let cap = sandbox("no-key", false);
    let settings = settings_for("https://api.openai.com/v1", None);
    match openai_compatible::run_login(cap.as_ref(), &settings) {
        IdentityOutcome::Failed(reason) => {
            assert!(reason.contains("OPENAI_API_KEY"), "{reason}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }

    let handle = Arc::new(openai_compatible::OpenAiCompat::new(cap.clone()));
    let usage = futures_lite_block_on(handle.identity_usage()).expect("usage call");
    assert_eq!(usage, Err(IdentityOutcome::NotSupported));
}

// Verifies: the Phase 3 exit test's transport clause — the provider's
// HTTP travels through the `net` capability, proven by a refusal:
// without a grant for the host, the request is denied and recorded and
// no socket is ever opened.
#[test]
fn transport_refuses_without_a_grant_and_records_the_denial() {
    // No ad hoc grant: nothing in the manifest covers the loopback test
    // server, exactly as an unconsented host would be uncovered.
    let cap = sandbox("denied", false);
    let handle = openai_compatible::OpenAiCompat::with_settings(
        cap.clone(),
        settings_for("http://127.0.0.1:9", None),
    );
    let sink = Collect::default();
    let outcome = run_stream(&handle, &sink);
    let err = outcome.expect_err("no grant, no request");
    assert!(
        err.contains("matches no granted") || err.contains("permission"),
        "the engine refused it: {err}"
    );
    assert_eq!(cap.denials().len(), 1, "{}", cap.denial_count());
    assert_eq!(cap.denials()[0].capability, "net");
    assert!(cap.denials()[0].reason.contains("127.0.0.1"));
}

// Verifies: FR-PROV-7's shape through the real transport — a canned SSE
// body decodes into the typed events, usage included (testing plan
// section 4's per-turn usage rule), over the granted loopback host.
#[test]
fn streams_a_canned_sse_body_through_the_granted_host() {
    let cap = sandbox("stream", true);
    let base = sse_server(concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\" there\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,",
        "\"completion_tokens\":2,\"prompt_tokens_details\":{\"cached_tokens\":6}}}\n\n",
        "data: [DONE]\n\n",
    ));
    let settings = settings_for(&base, Some("sk-test"));
    let mut seen = Vec::new();
    let stopped = std::sync::atomic::AtomicBool::new(false);
    openai_compatible::run_provider_stream(
        cap.as_ref(),
        &settings,
        &request("test-model"),
        &mut |event| {
            seen.push(event);
            !stopped.load(std::sync::atomic::Ordering::SeqCst)
        },
    )
    .expect("stream completes");
    let text: Vec<&str> = seen
        .iter()
        .filter_map(|event| match event {
            lca_protocol::StreamEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, vec!["Hi", " there"]);
    let usage = seen.iter().find_map(|event| match event {
        lca_protocol::StreamEvent::Usage { usage } => Some(usage),
        _ => None,
    });
    let usage = usage.expect("the body carried usage");
    assert_eq!(usage.cache_read, 6, "cache fields survive for FR-CACHE-1");
    assert_eq!(usage.input, 4, "billed input excludes the cache read");
    assert!(cap.denials().is_empty(), "granted host, nothing refused");
}

// Verifies: FR-PROV-9's dependency — the grants the native handle
// carries are exactly what extension.toml declares (one source, two
// spellings, checked).
#[test]
fn manifest_toml_and_native_grants_agree() {
    let manifest: toml::Value = openai_compatible::MANIFEST
        .parse()
        .expect("MANIFEST parses");
    let hosts = manifest["capabilities"]["net"]["hosts"]
        .as_array()
        .expect("hosts list");
    let names: Vec<&str> = hosts.iter().filter_map(|value| value.as_str()).collect();
    assert_eq!(names, vec!["api.openai.com"]);
    let namespace = manifest["capabilities"]["credentials"]["namespace"]
        .as_str()
        .expect("namespace");
    assert_eq!(namespace, "openai-compatible");
    let grants = openai_compatible::manifest_grants();
    assert!(
        grants
            .net
            .iter()
            .any(|pattern| pattern.matches("api.openai.com", 443))
    );
    assert!(grants.credentials);
    assert!(grants.fs.is_empty() && !grants.process && !grants.pty);
    for world in ["provider", "command"] {
        assert!(
            manifest["worlds"]
                .as_array()
                .expect("worlds")
                .iter()
                .any(|value| value.as_str() == Some(world)),
            "{world} declared"
        );
    }
}

// Helpers -----------------------------------------------------------------

use lca_protocol::StreamEvent;

#[derive(Default, Clone)]
struct Collect(Arc<Mutex<Vec<StreamEvent>>>);

impl lca_protocol::EventSink for Collect {
    fn push(&self, event: StreamEvent) -> bool {
        self.0.lock().expect("events").push(event);
        true
    }
}

/// Run one stream through the dispatch handle on a plain runtime.
fn run_stream(handle: &openai_compatible::OpenAiCompat, sink: &Collect) -> Result<(), String> {
    let sink: Collect = (*sink).clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async move {
            handle
                .stream_completion(request("test-model"), &sink)
                .await
                .map_err(|err| err.to_string())
        })
}

/// `identity_usage` returns a boxed future; poll it without pulling the
/// async-trait machinery into a sync test.
fn futures_lite_block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}
