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
    let root = lca_testkit::scratch_path(&format!("lca-openai-{name}"));
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
        context_window: 0,
        prompt_cache_key: true,
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
    // The component exports only the provider world (the manifest schema
    // requires every declared world to be exported), so `command` is not
    // declared: this provider ships no slash commands of its own.
    let worlds: Vec<&str> = manifest["worlds"]
        .as_array()
        .expect("worlds")
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert_eq!(worlds, vec!["provider"]);
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

// Verifies: FR-CONC-1, NFR-21 (a native provider's `interrupt` reaches its
// capability engine, so a blocked `net` request is cancelled - the epoch
// bump only fires at a guest code point).
#[test]
fn interrupt_flags_the_capability_engine() {
    let cap = sandbox("interrupt", false);
    let ext = openai_compatible::OpenAiCompat::new(cap.clone());
    assert!(!cap.is_cancelled());
    ext.interrupt();
    assert!(cap.is_cancelled(), "the engine is flagged for the net wait");
}

/// A loopback server that captures the request body and answers with a
/// minimal SSE stream (for the V1 cache-pin test).
fn body_capture_server() -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        if let Some(mut stream) = listener.incoming().flatten().next() {
            // Read until the full body arrives (headers and body can be
            // separate packets).
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                    let len: usize = head
                        .lines()
                        .find(|line| line.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|line| line.split_once(':'))
                        .and_then(|(_, value)| value.trim().parse().ok())
                        .unwrap_or(0);
                    if buf.len() >= pos + 4 + len {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&buf).to_string();
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
            let _ = tx.send(body);
            let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://{addr}"), rx)
}

// Verifies: V1 / ADR-0031 (the request body carries the clamped session
// cache pin, and the preset opt-out drops it).
#[test]
fn the_body_carries_a_clamped_prompt_cache_key() {
    let cap = sandbox("cache-key", true);
    let (base, rx) = body_capture_server();
    let settings = settings_for(&base, Some("sk-test"));
    let mut req = request("test-model");
    req.extras.insert("session-id".to_string(), "s".repeat(80));
    openai_compatible::run_provider_stream(cap.as_ref(), &settings, &req, &mut |_| true)
        .expect("stream completes");
    let body = rx.recv().expect("body captured");
    let json: serde_json::Value = serde_json::from_str(&body).expect("json body");
    let key = json
        .get("prompt_cache_key")
        .and_then(|value| value.as_str());
    assert!(key.is_some(), "cache key present in body: {body}");
    let key = key.expect("key");
    assert_eq!(key.len(), 64, "clamped to 64 chars");
    assert_eq!(key, "s".repeat(64));

    // The preset opt-out drops it.
    let cap = sandbox("cache-key-off", true);
    let (base, rx) = body_capture_server();
    let mut settings = settings_for(&base, Some("sk-test"));
    settings.prompt_cache_key = false;
    let mut req = request("test-model");
    req.extras
        .insert("session-id".to_string(), "abc".to_string());
    openai_compatible::run_provider_stream(cap.as_ref(), &settings, &req, &mut |_| true)
        .expect("stream completes");
    let body = rx.recv().expect("body captured");
    let json: serde_json::Value = serde_json::from_str(&body).expect("json body");
    assert!(
        json.get("prompt_cache_key").is_none(),
        "the opt-out drops the pin"
    );
}
