//! The OAuth reference provider against mocked Google endpoints: the
//! full loopback login, FR-PROV-5's refresh, the streamed completion,
//! the quota usage shape, and logout with its revoke - every call
//! through the capability engine, no real network (testing plan §4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
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
/// One scripted response: the path needle, the status, and the payload
/// (`None` keeps the default payload for the path).
type Scripted = (String, u16, Option<String>);

struct Mock {
    base: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
    /// Scripted one-shot responses, FIFO: the first entry whose needle
    /// matches the request path answers with its status and payload,
    /// then leaves the queue.
    scripted: Arc<Mutex<Vec<Scripted>>>,
}

fn mock_server() -> Mock {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let scripted: Arc<Mutex<Vec<Scripted>>> = Arc::new(Mutex::new(Vec::new()));
    let script = scripted.clone();
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

            let mut scripted = script.lock().expect("script");
            let hit = scripted
                .iter()
                .position(|(needle, _, _)| path.contains(needle))
                .map(|index| scripted.remove(index));
            drop(scripted);
            let (status, forced) = match hit {
                Some((_, status, payload)) => (status, payload),
                None => (200, None),
            };

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
            let payload = forced.unwrap_or(payload);
            let reason = match status {
                200 => "OK",
                401 => "Unauthorized",
                404 => "Not Found",
                _ => "Error",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\n\
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
        scripted,
    }
}

impl Mock {
    /// Answer the next request whose path contains `needle` with
    /// `status` (`None` keeps the default payload for the path).
    fn fail_once(&self, needle: &str, status: u16, payload: Option<&str>) {
        self.scripted.lock().expect("script").push((
            needle.to_string(),
            status,
            payload.map(str::to_string),
        ));
    }
}

/// A sandbox whose credential namespace points every endpoint at the
/// mock (the gateway/mirror override), with the ad hoc loopback grant
/// the login modal would have attached (FR-PERM-16).
fn sandbox(name: &str, mock: &Mock) -> Arc<lca_tools::Capabilities> {
    let root = lca_testkit::scratch_path(&format!("lca-ag-{name}"));
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

// Verifies: issue #1 - a login with no stored/env client pair uses the
// embedded default Antigravity client and drives the flow (owner override
// of the earlier "configuration only" policy).
#[test]
fn login_without_a_stored_client_pair_uses_the_embedded_default() {
    let mock = mock_server();
    let cap = sandbox("no-client", &mock);
    lca_protocol::ProviderCap::credentials_delete(cap.as_ref(), "client_id").expect("clear");
    lca_protocol::ProviderCap::credentials_delete(cap.as_ref(), "client_secret").expect("clear");
    let outcome = drive_login(&cap).expect("the embedded default drives the flow");
    assert_eq!(outcome, IdentityOutcome::Ok);
    let opened = cap.oauth_opened();
    assert!(!opened.is_empty(), "a flow was started");
    assert!(
        opened
            .iter()
            .any(|u| u.contains("apps.googleusercontent.com")),
        "the embedded client id is in the auth URL: {opened:?}"
    );
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
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
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
    // Uses pi-antigravity's 8 ANTIGRAVITY_MODELS.
    let empty = sandbox("models-empty", &mock);
    let models = antigravity::list_models(empty.as_ref());
    assert_eq!(models.len(), 8, "fallback list matching pi-antigravity");
    assert_eq!(models[0].id, "gemini-3.8-flash");
}

// Verifies: F2 - the parity guard asserting FALLBACK_MODELS matches
// pi-antigravity's ANTIGRAVITY_MODELS at commit a3d8caba1b10263420060406de57112ce16490d0
// byte-for-byte on IDs, display names, context windows, and max output tokens.
#[test]
fn antigravity_fallback_models_match_pi_antigravity_ground_truth() {
    // Expected ground truth transcribed from ~/gits/pi-antigravity/src/models/models.ts:216
    // at commit a3d8caba1b10263420060406de57112ce16490d0 (release 0.9.0).
    let expected = [
        (
            "gemini-3.8-flash",
            "Gemini 3.8 Flash (Antigravity)",
            1048576,
            65536,
        ),
        (
            "gemini-3.7-flash",
            "Gemini 3.7 Flash (Antigravity)",
            1048576,
            65536,
        ),
        (
            "gemini-3.6-flash",
            "Gemini 3.6 Flash (Antigravity)",
            1048576,
            65536,
        ),
        (
            "claude-opus-4-6",
            "Claude Opus 4.6 (Antigravity)",
            250000,
            64000,
        ),
        (
            "claude-sonnet-4-6",
            "Claude Sonnet 4.6 (Antigravity)",
            200000,
            64000,
        ),
        (
            "gemini-3.1-pro",
            "Gemini 3.1 Pro (Antigravity)",
            1048576,
            65535,
        ),
        (
            "gemini-3.5-flash",
            "Gemini 3.5 Flash (Antigravity)",
            1048576,
            65536,
        ),
        ("gpt-oss-120b", "GPT-OSS 120B (Antigravity)", 131072, 32768),
    ];

    let mock = mock_server();
    let empty = sandbox("models-parity-guard", &mock);
    let models = antigravity::list_models(empty.as_ref());
    assert_eq!(models.len(), expected.len(), "model count mismatch");

    for (actual, (exp_id, exp_name, exp_ctx, exp_max)) in models.iter().zip(expected.iter()) {
        assert_eq!(&actual.id, exp_id, "model id must match exactly");
        assert_eq!(
            &actual.name, exp_name,
            "model display name must match exactly"
        );
        assert_eq!(
            actual.context_window, *exp_ctx,
            "context window for {exp_id} must match"
        );
        assert_eq!(
            actual.max_tokens, *exp_max,
            "max output tokens for {exp_id} must match"
        );
    }
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

// Verifies: gh #179 - a login with stored (even expired) tokens runs
// the OAuth flow again instead of short-circuiting: the invocation is
// the overwrite prompt, and a revoked token repairs rather than
// deadlocking.
#[test]
fn a_login_with_stored_tokens_re_runs_the_flow() {
    let mock = mock_server();
    let cap = sandbox("relogin", &mock);
    cap.credentials_set("access", "stale-access").expect("seed");
    cap.credentials_set("refresh", "stale-refresh")
        .expect("seed");
    cap.credentials_set("expires", "1").expect("seed"); // long expired
    cap.credentials_set("project", "p").expect("seed");

    let outcome = drive_login(&cap).expect("login runs");
    assert_eq!(outcome, IdentityOutcome::Ok);
    assert_eq!(cap.oauth_opened().len(), 1, "the flow re-opened");
    assert_eq!(stored(&cap, "access"), Some("mock-access".to_string()));
    assert_eq!(stored(&cap, "refresh"), Some("mock-refresh".to_string()));
    // The PKCE verifier crossed on the exchange (the receipt's pin).
    assert!(!requests_of(&mock, "code_verifier").is_empty());
}

// Verifies: gh #179 - a 401 on the stream purges the stored tokens, so
// the next call reports "no login" and re-authenticates instead of
// looping on the dead token.
#[test]
fn a_401_purges_the_tokens_and_the_next_call_reauthenticates() {
    let mock = mock_server();
    let cap = sandbox("purge", &mock);
    cap.credentials_set("access", "revoked-access")
        .expect("seed");
    cap.credentials_set("refresh", "revoked-refresh")
        .expect("seed");
    cap.credentials_set("expires", &format!("{}", u64::MAX - 60))
        .expect("seed");
    cap.credentials_set("project", "p").expect("seed");
    mock.fail_once(
        "streamGenerateContent",
        401,
        Some(r#"{"error":{"message":"Request had invalid credentials."}}"#),
    );

    let request = lca_protocol::CompletionRequest {
        messages: vec![lca_protocol::ChatMessage {
            role: lca_protocol::MessageRole::User,
            content: vec![lca_protocol::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        }],
        tools: Vec::new(),
        model: "gemini-3.8-flash-low".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let failed = antigravity::run_provider_stream(cap.as_ref(), &request, &mut |_| true);
    assert!(failed.is_err(), "the rejected call fails");

    assert_eq!(stored(&cap, "access"), None, "access purged");
    assert_eq!(stored(&cap, "refresh"), None, "refresh purged");
    assert_eq!(stored(&cap, "expires"), None, "expiry purged");
    // The next call no longer loops on the dead token: it reports that
    // no login exists, which is what sends the user to re-authenticate.
    let usage = antigravity::run_usage(cap.as_ref());
    assert!(
        format!("{usage:?}").contains("no Antigravity login yet"),
        "re-authenticates, not loops: {usage:?}"
    );
}

// Verifies: gh #179 - a 404 on a preview id falls back to the mapped
// backend model, which serves: two stream calls, the fallback's model
// id on the second body.
#[test]
fn a_404_on_a_preview_id_falls_back_to_the_mapped_model() {
    let mock = mock_server();
    let cap = sandbox("fallback", &mock);
    cap.credentials_set("access", "live-access").expect("seed");
    cap.credentials_set("expires", &format!("{}", u64::MAX - 60))
        .expect("seed");
    cap.credentials_set("project", "p").expect("seed");
    mock.fail_once("streamGenerateContent", 404, Some("{}"));

    let request = lca_protocol::CompletionRequest {
        messages: vec![lca_protocol::ChatMessage {
            role: lca_protocol::MessageRole::User,
            content: vec![lca_protocol::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        }],
        tools: Vec::new(),
        model: "gemini-3.8-flash".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let mut events = Vec::new();
    antigravity::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        events.push(event);
        true
    })
    .expect("the fallback serves");

    let streams = requests_of(&mock, "streamGenerateContent");
    assert_eq!(streams.len(), 2, "initial plus fallback: {streams:?}");
    assert!(
        streams[0].contains("\"model\":\"gemini-3.8-flash-low\""),
        "first the requested preview: {}",
        streams[0]
    );
    assert!(
        streams[1].contains("\"model\":\"gemini-3.7-flash-low\""),
        "then the mapped backend model: {}",
        streams[1]
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, lca_protocol::StreamEvent::TextDelta { .. })),
        "the fallback's text arrives"
    );
}

// Verifies: gh #179 - the hostile schema crosses the wire normalized
// per model class: Claude's legacy `parameters` carry the allowlist
// only (no `parametersJsonSchema`), Gemini's `parametersJsonSchema`
// keeps the full schema minus metadata.
#[test]
fn the_hostile_schema_crosses_normalized_per_model_class() {
    let hostile = serde_json::json!({
        "type": "object",
        "nullable": true,
        "properties": {
            "path": {"type": "string", "format": "uri"},
            "choice": {"anyOf": [{"type": "string"}]},
        },
        "$defs": {"x": {"type": "string"}},
    });
    let request = |model: &str| lca_protocol::CompletionRequest {
        messages: vec![lca_protocol::ChatMessage {
            role: lca_protocol::MessageRole::User,
            content: vec![lca_protocol::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        }],
        tools: vec![lca_protocol::ToolSpec {
            name: "read".to_string(),
            description: "read".to_string(),
            parameters: hostile.clone(),
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        }],
        model: model.to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let stream = |cap: &Arc<lca_tools::Capabilities>, model: &str| {
        antigravity::run_provider_stream(cap.as_ref(), &request(model), &mut |_| true)
            .expect("streams");
    };

    let mock = mock_server();
    let claude = sandbox("hostile-claude", &mock);
    let fresh = format!("{}", u64::MAX - 60);
    for key in ["access", "expires", "project"] {
        claude
            .credentials_set(key, if key == "expires" { &fresh } else { "x" })
            .expect("seed");
    }
    stream(&claude, "claude-sonnet-4-6");
    let bodies = requests_of(&mock, "streamGenerateContent");
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].contains(
            r#""parameters":{"properties":{"choice":{},"path":{"type":"string"}},"type":"object"}"#
        ),
        "allowlist only: {}",
        bodies[0]
    );
    assert!(
        !bodies[0].contains("parametersJsonSchema"),
        "no JsonSchema field on the legacy channel"
    );
    assert!(
        !bodies[0].contains("nullable") && !bodies[0].contains("anyOf"),
        "rejected keywords gone: {}",
        bodies[0]
    );

    let gemini = sandbox("hostile-gemini", &mock);
    for key in ["access", "expires", "project"] {
        gemini
            .credentials_set(key, if key == "expires" { &fresh } else { "x" })
            .expect("seed");
    }
    stream(&gemini, "gemini-3.8-flash-low");
    let bodies = requests_of(&mock, "streamGenerateContent");
    assert_eq!(bodies.len(), 2, "one more call crossed");
    assert!(
        bodies[1].contains("parametersJsonSchema"),
        "gemini keeps the JsonSchema channel"
    );
    assert!(
        !bodies[1].contains("$defs"),
        "metadata stripped even on the JsonSchema channel"
    );
}

// Verifies: gh #179 - multi-turn labels cross the wire: a three-message
// request carries `last_execution_id`, a single-message one does not.
#[test]
fn multi_turn_labels_carry_last_execution_id() {
    let mock = mock_server();
    let cap = sandbox("labels", &mock);
    cap.credentials_set("access", "live-access").expect("seed");
    cap.credentials_set("expires", &format!("{}", u64::MAX - 60))
        .expect("seed");
    cap.credentials_set("project", "p").expect("seed");
    let message = |text: &str| lca_protocol::ChatMessage {
        role: lca_protocol::MessageRole::User,
        content: vec![lca_protocol::ContentBlock::Text {
            text: text.to_string(),
        }],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    };
    let request = |messages: Vec<lca_protocol::ChatMessage>| lca_protocol::CompletionRequest {
        messages,
        tools: Vec::new(),
        model: "gemini-3.8-flash-low".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    for messages in [
        vec![message("hi")],
        vec![message("a"), message("b"), message("c")],
    ] {
        antigravity::run_provider_stream(cap.as_ref(), &request(messages), &mut |_| true)
            .expect("streams");
    }
    let bodies = requests_of(&mock, "streamGenerateContent");
    assert_eq!(bodies.len(), 2);
    assert!(
        !bodies[0].contains("last_execution_id"),
        "first step carries none"
    );
    assert!(
        bodies[1].contains("last_execution_id"),
        "later steps carry one: {}",
        bodies[1]
    );
}
