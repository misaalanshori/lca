//! The OAuth reference provider against mocked Google endpoints: the
//! full loopback login, FR-PROV-5's refresh, the streamed completion,
//! the quota usage shape, and logout with its revoke - every call
//! through the capability engine, no real network (testing plan §4).

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::IdentityOutcome;

struct Always;
impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// One mock Google: token, code-assist, catalog, quota, stream, and
/// revoke behind a single loopback origin, with every request recorded.
struct Mock {
    base: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

fn mock_server() -> Mock {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            // Read until the request head + the Content-Length body.
            let mut expected = None;
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if expected.is_none()
                    && let Ok(text) = std::str::from_utf8(&buf)
                    && let Some(start) = text.find("\r\n\r\n")
                {
                    let head = &text[..start];
                    let length: usize = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    expected = Some(start + 4 + length);
                }
                if expected == Some(buf.len()) {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&buf).into_owned();
            let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
            let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
            recorded
                .lock()
                .expect("requests")
                .push((path.clone(), body));

            let payload = if path.contains("streamGenerateContent") {
                concat!(
                    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hi\"}]}}],",
                    "\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":2,",
                    "\"cachedContentTokenCount\":4}}\n\n",
                    "data: {\"candidates\":[{\"content\":{\"parts\":[",
                    "{\"functionCall\":{\"name\":\"read\",\"id\":\"call-1\",",
                    "\"args\":{\"path\":\"a.txt\"}}}]}}],",
                    "\"usageMetadata\":{\"promptTokenCount\":10,",
                    "\"candidatesTokenCount\":5}}\n\n",
                    "data: [DONE]\n\n",
                )
                .to_string()
            } else if path.contains("fetchAvailableModels") {
                r#"{"models":{"gemini-2.5-pro":{"displayName":"Gemini 2.5 Pro"},"gemini-2.5-flash":{"displayName":"Gemini 2.5 Flash"}}}"#
                    .to_string()
            } else if path.contains("retrieveUserQuotaSummary") {
                r#"{"tiers":{"free":{"remainingPercent":87.5}}}"#.to_string()
            } else if path.contains("loadCodeAssist") {
                r#"{"response":{"project":"mock-project-123"}}"#.to_string()
            } else if path.contains("/token") || path.ends_with("token") {
                r#"{"access_token":"mock-access","refresh_token":"mock-refresh","expires_in":1800}"#
                    .to_string()
            } else {
                "{}".to_string()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    Mock {
        base: format!("http://{addr}"),
        requests,
    }
}

/// A sandbox whose credential namespace points every endpoint at the
/// mock (the gateway/mirror override), with the ad hoc loopback grant
/// the login modal would have attached (FR-PERM-16).
fn sandbox(name: &str, mock: &Mock) -> Arc<lca_tools::Capabilities> {
    let root = std::env::temp_dir().join(format!("lca-ag-{name}-{}", std::process::id()));
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
    let mut grants = antigravity::manifest_grants();
    grants
        .adhoc_net
        .push(lca_permissions::parse_net_pattern("127.0.0.1").expect("loopback pattern"));
    let cap = Arc::new(lca_tools::Capabilities::new(
        "antigravity",
        grants,
        roots,
        Arc::new(Mutex::new(Always)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    ));
    // Endpoint overrides live in this extension's own namespace, which
    // is what lets the tests (or a gateway) redirect every call.
    cap.credentials_set("api_base", &mock.base).expect("seed");
    cap.credentials_set("token_endpoint", &format!("{}/token", mock.base))
        .expect("seed");
    cap.credentials_set("auth_endpoint", &format!("{}/auth", mock.base))
        .expect("seed");
    cap.credentials_set("revoke_endpoint", &format!("{}/revoke", mock.base))
        .expect("seed");
    // A test client pair (what a user's ANTIGRAVITY_CLIENT_ID/SECRET
    // would provide); nothing real is embedded anywhere in the repo.
    cap.credentials_set("client_id", "test-client.apps.googleusercontent.com")
        .expect("seed");
    cap.credentials_set("client_secret", "test-secret")
        .expect("seed");
    cap
}

// Verifies: the client pair is configuration the user supplies (the
// names pi uses for its own override), never source - a login with
// nothing configured says exactly that before any flow starts.
#[test]
fn login_without_a_client_pair_says_what_to_set() {
    let mock = mock_server();
    let cap = sandbox("no-client", &mock);
    lca_protocol::ProviderCap::credentials_delete(cap.as_ref(), "client_id").expect("clear");
    lca_protocol::ProviderCap::credentials_delete(cap.as_ref(), "client_secret").expect("clear");
    let err = antigravity::run_login(cap.as_ref(), cap.as_ref()).expect_err("no client, no flow");
    assert!(err.0.contains("ANTIGRAVITY_CLIENT_ID"), "{}", err.0);
    assert!(cap.oauth_opened().is_empty(), "no flow was started");
    assert!(cap.denials().is_empty());
}

/// Run the full login on a thread, watch the engine for the
/// authorization URL it opened, and answer the loopback callback the
/// way the browser would (FR-PROV-3: the host's listener delivers the
/// parsed parameters to the extension).
fn drive_login(cap: &Arc<lca_tools::Capabilities>) -> Result<IdentityOutcome, String> {
    let for_thread = cap.clone();
    let result: Arc<Mutex<Option<Result<IdentityOutcome, String>>>> = Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let outcome =
            antigravity::run_login(for_thread.as_ref(), for_thread.as_ref()).map_err(|err| err.0);
        *slot.lock().expect("slot") = Some(outcome);
    });

    // The engine records the redirect URL inside `oauth.begin` (before the
    // extension asks the host to open the authorization URL), so both are
    // available once `oauth_open` lands; `oauth_begun` gives the redirect
    // without the auth URL's percent-encoding to undo.
    let deadline = Instant::now() + Duration::from_secs(15);
    let (redirect, auth_url) = loop {
        if let (Some(redirect), Some(auth)) = (
            cap.oauth_begun().last().cloned(),
            cap.oauth_opened().last().cloned(),
        ) {
            break (redirect, auth);
        }
        if Instant::now() > deadline {
            return Err("the login never bound a redirect URL".to_string());
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let state = auth_url
        .split('&')
        .find_map(|pair| pair.strip_prefix("state="))
        .ok_or_else(|| "the authorization URL carries no state".to_string())?
        .to_string();

    // Send the callback, retrying while the login is still running: the
    // loopback listener occasionally misses the first connection on a busy
    // runner (the macOS CI flake this guards). Once the listener delivers its
    // one callback it stops accepting, so the extra connects are no-ops.
    let callback = format!("{redirect}?code=mock-code&state={state}");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut attempts = 0;
    loop {
        if let Some(outcome) = result.lock().expect("slot").take() {
            return outcome;
        }
        if attempts < 5 {
            let _ = send_callback(&callback);
            attempts += 1;
        }
        if Instant::now() > deadline {
            return Err("the login never finished after the callback was sent".to_string());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// One loopback GET to the flow's redirect URL. Errors are the caller's to
/// ignore; a retry follows.
fn send_callback(callback: &str) -> Result<(), String> {
    let host_path = callback.trim_start_matches("http://");
    let (hostport, path) = host_path.split_once('/').ok_or("no redirect path")?;
    let mut stream = std::net::TcpStream::connect(hostport).map_err(|err| err.to_string())?;
    write!(
        stream,
        "GET /{path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|err| err.to_string())
}

/// The engine's inherent `credentials_get` returns a `Result`; this
/// reads it the way the trait's absence-as-`None` contract does.
fn stored(cap: &Arc<lca_tools::Capabilities>, key: &str) -> Option<String> {
    lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key)
}

fn requests_of(mock: &Mock, needle: &str) -> Vec<String> {
    mock.requests
        .lock()
        .expect("requests")
        .iter()
        .filter(|(path, body)| path.contains(needle) || body.contains(needle))
        .map(|(path, body)| format!("{path} {body}"))
        .collect()
}

// Verifies: the Phase 3 exit test's OAuth half - a subscription login
// that binds no port itself (FR-PROV-4), completes the loopback flow
// through the host (FR-PROV-3), exchanges the code, learns the project,
// and stores everything in its own credential namespace (FR-PERM-6).
#[test]
fn login_runs_the_loopback_flow_and_stores_tokens_in_its_namespace() {
    let mock = mock_server();
    let cap = sandbox("login", &mock);
    let outcome = drive_login(&cap).expect("login runs");
    assert_eq!(outcome, IdentityOutcome::Ok);
    assert_eq!(stored(&cap, "access"), Some("mock-access".to_string()));
    assert_eq!(stored(&cap, "refresh"), Some("mock-refresh".to_string()));
    assert_eq!(
        stored(&cap, "project"),
        Some("mock-project-123".to_string())
    );
    // The exchange and the handshake both went to the mock over the
    // capability engine; the auth URL was opened by the host.
    assert!(
        !requests_of(&mock, "mock-code").is_empty(),
        "code exchanged"
    );
    assert!(
        !requests_of(&mock, "loadCodeAssist").is_empty(),
        "handshake ran"
    );
    let opened = cap.oauth_opened();
    assert_eq!(opened.len(), 1, "one authorization URL: {opened:?}");
    assert!(
        opened[0].contains("accounts.google.com") || opened[0].contains(&mock.base),
        "auth URL: {}",
        opened[0]
    );
    assert!(opened[0].contains("code_challenge="), "PKCE challenge sent");
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: FR-PROV-5 - an expired stored token with a refresh token
// present is refreshed on the next call, before the request goes out.
#[test]
fn an_expired_token_refreshes_before_the_next_call() {
    let mock = mock_server();
    let cap = sandbox("refresh", &mock);
    cap.credentials_set("access", "stale-access").expect("seed");
    cap.credentials_set("refresh", "mock-refresh")
        .expect("seed");
    cap.credentials_set("expires", "1").expect("seed"); // long expired
    cap.credentials_set("project", "p").expect("seed");

    // Quota goes through the same path: access_token() refreshes first.
    let outcome = antigravity::run_usage(cap.as_ref()).map(|_| ());
    assert!(outcome.is_ok(), "{outcome:?}");
    // The refresh body is the only one that carries a refresh_token.
    let refreshes = requests_of(&mock, "refresh_token");
    assert_eq!(refreshes.len(), 1, "one refresh: {refreshes:?}");
    assert!(refreshes[0].contains("mock-refresh"));
    // The refreshed access replaced the stale one.
    assert_eq!(stored(&cap, "access"), Some("mock-access".to_string()));
}

// Verifies: the Phase 3 exit test's completion half - a streamed
// response decodes into the typed events with usage carrying the cache
// fields (FR-CACHE-1's source) and the call shape FR-PROV-7 requires.
#[test]
fn the_completion_streams_typed_events_with_cache_carrying_usage() {
    let mock = mock_server();
    let cap = sandbox("stream", &mock);
    cap.credentials_set("access", "live-access").expect("seed");
    cap.credentials_set("expires", &format!("{}", u64::MAX - 60))
        .expect("seed");
    cap.credentials_set("project", "p").expect("seed");

    let request = lca_protocol::CompletionRequest {
        messages: vec![
            lca_protocol::ChatMessage {
                role: lca_protocol::MessageRole::System,
                content: vec![lca_protocol::ContentBlock::Text {
                    text: "be brief".to_string(),
                }],
                tool_calls: Vec::new(),
                tool_call_id: None,
                usage: None,
                extras: Default::default(),
            },
            lca_protocol::ChatMessage {
                role: lca_protocol::MessageRole::User,
                content: vec![lca_protocol::ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                tool_calls: Vec::new(),
                tool_call_id: None,
                usage: None,
                extras: Default::default(),
            },
        ],
        tools: vec![lca_protocol::ToolSpec {
            name: "read".to_string(),
            description: "read a file".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
            extras: Default::default(),
        }],
        model: "gemini-2.5-pro".to_string(),
        stable_prefix: 1,
        extras: Default::default(),
    };

    let mut events = Vec::new();
    antigravity::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        events.push(event);
        true
    })
    .expect("stream completes");

    let text: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            lca_protocol::StreamEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, vec!["Hi"]);
    let usage = events
        .iter()
        .find_map(|event| match event {
            lca_protocol::StreamEvent::Usage { usage } => Some(usage),
            _ => None,
        })
        .expect("usage arrives");
    assert_eq!(usage.cache_read, 4);
    assert_eq!(usage.input, 6, "billed input excludes the cache read");
    // FR-PROV-7: the call opens before its arguments, and closes.
    let order: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            lca_protocol::StreamEvent::ToolCallStart { name, .. } => {
                Some(Box::leak(format!("start:{name}").into_boxed_str()) as &str)
            }
            lca_protocol::StreamEvent::ToolCallArgDelta { .. } => Some("delta"),
            lca_protocol::StreamEvent::ToolCallEnd { .. } => Some("end"),
            _ => None,
        })
        .collect();
    assert_eq!(order, vec!["start:read", "delta", "end"]);
    assert!(
        requests_of(&mock, "streamGenerateContent").len() == 1,
        "one completion call"
    );
    assert!(
        requests_of(&mock, "functionDeclarations").len() == 1,
        "the tool list crossed"
    );
    assert!(cap.denials().is_empty());
}

// Verifies: FR-PROV-2's data through the real catalog call, with the
// static pair as the honest fallback when the catalog is unreachable.
#[test]
fn model_listing_reads_the_catalog_or_falls_back() {
    let mock = mock_server();
    let cap = sandbox("models", &mock);
    cap.credentials_set("access", "live-access").expect("seed");
    cap.credentials_set("expires", &format!("{}", u64::MAX - 60))
        .expect("seed");
    cap.credentials_set("project", "p").expect("seed");
    let models = antigravity::list_models(cap.as_ref());
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, vec!["gemini-2.5-flash", "gemini-2.5-pro"]);
    assert_eq!(models[1].name, "Gemini 2.5 Pro");
    assert!(!requests_of(&mock, "fetchAvailableModels").is_empty());

    // No login: the static fallback, no panic (the picker still works).
    let empty = sandbox("models-empty", &mock);
    let models = antigravity::list_models(empty.as_ref());
    assert_eq!(models.len(), 2, "fallback list");
}

// Verifies: the generic and namespaced `/usage` (ADR-0012, FR-PROV-10)
// answer in the standard shape, with the raw quota summary preserved.
#[test]
fn usage_answers_in_the_standard_shape() {
    let mock = mock_server();
    let cap = sandbox("usage", &mock);
    cap.credentials_set("access", "live-access").expect("seed");
    cap.credentials_set("expires", &format!("{}", u64::MAX - 60))
        .expect("seed");
    cap.credentials_set("project", "p").expect("seed");
    let usage = antigravity::run_usage(cap.as_ref()).expect("usage");
    let summary = usage.extras.get("quota-summary").expect("summary carried");
    assert!(summary.contains("remainingPercent"), "{summary}");
    assert!(!requests_of(&mock, "retrieveUserQuotaSummary").is_empty());
}

// Verifies: `logout` revokes server-side where the API supports it and
// clears this namespace either way (docs/providers/antigravity.md).
#[test]
fn logout_revokes_and_clears_the_namespace() {
    let mock = mock_server();
    let cap = sandbox("logout", &mock);
    cap.credentials_set("access", "doomed-access")
        .expect("seed");
    let outcome = antigravity::run_logout(cap.as_ref(), cap.as_ref());
    assert_eq!(outcome, IdentityOutcome::Ok);
    assert!(stored(&cap, "access").is_none());
    assert!(stored(&cap, "refresh").is_none());
    assert!(
        !requests_of(&mock, "revoke").is_empty(),
        "the revoke went out"
    );
}

// Verifies: docs/providers/antigravity.md's manifest, spelled the same
// way in extension.toml and in the grants the native form carries.
#[test]
fn manifest_toml_and_native_grants_agree() {
    let manifest: toml::Value = antigravity::MANIFEST.parse().expect("MANIFEST parses");
    let hosts = manifest["capabilities"]["net"]["hosts"]
        .as_array()
        .expect("hosts list");
    let names: Vec<&str> = hosts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        names,
        vec!["generativelanguage.googleapis.com", "*.googleapis.com"]
    );
    assert_eq!(
        manifest["capabilities"]["oauth"]["redirect_path"].as_str(),
        Some("/callback")
    );
    assert_eq!(
        manifest["capabilities"]["credentials"]["namespace"].as_str(),
        Some("antigravity")
    );
    let grants = antigravity::manifest_grants();
    assert!(
        grants
            .net
            .iter()
            .any(|p| p.matches("generativelanguage.googleapis.com", 443))
    );
    assert!(
        grants
            .net
            .iter()
            .any(|p| p.matches("daily-cloudcode-pa.googleapis.com", 443))
    );
    assert!(grants.oauth.is_some() && grants.credentials);
    assert!(grants.fs.is_empty() && !grants.process && !grants.pty);
}
