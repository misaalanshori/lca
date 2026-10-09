//! Shared mock Copilot gateway + real capability engine for the
//! conformance journeys: every call through the engine, no real
//! network.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
#![allow(dead_code)] // shared harness: each consumer uses a subset.
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lca_permissions::{GrantStore, ScopeRoots};

/// One scripted response: the path needle, the status, and the payload
/// (`None` keeps the default payload for the path).
type Scripted = (String, u16, Option<String>);

/// The shared mock gateway both journeys hit.
pub struct Mock {
    /// The loopback base URL the sandbox points at.
    pub base: String,
    /// Every request: path, headers, body.
    pub requests: Arc<Mutex<Vec<(String, String, String)>>>,
    polls: Arc<AtomicUsize>,
    scripted: Arc<Mutex<Vec<Scripted>>>,
}

/// The recorded chat stream every turn replays.
pub const STREAM_FIXTURE: &str = include_str!("../fixtures/chat-stream.sse");

/// The mock catalog: a picker row, a policy-enabled row, a disabled
/// row, and a tool-less row.
pub const MODELS_FIXTURE: &str = r#"{"data":[
{"id":"gpt-4o","model_picker_enabled":true,"capabilities":{"supports":{"tool_calls":true}}},
{"id":"mock/policy-model","model_picker_enabled":false,"policy":{"state":"enabled"},"capabilities":{"supports":{"tool_calls":true}}},
{"id":"mock/disabled-model","model_picker_enabled":true,"policy":{"state":"disabled"}},
{"id":"mock/text-only","model_picker_enabled":true,"capabilities":{"supports":{"tool_calls":false}}}
]}"#;

/// Start the mock gateway on a loopback port.
pub fn mock_server() -> Mock {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let polls: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let poll_count = polls.clone();
    let scripted: Arc<Mutex<Vec<Scripted>>> = Arc::new(Mutex::new(Vec::new()));
    let script = scripted.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
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
            let headers = text.split("\r\n\r\n").next().unwrap_or("").to_string();
            recorded
                .lock()
                .expect("requests")
                .push((path.clone(), headers, body));

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

            let payload = if path.contains("/chat/completions") {
                STREAM_FIXTURE.to_string()
            } else if path.contains("/login/device/code") {
                r#"{"device_code":"dev-1","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","interval":1,"expires_in":30}"#.to_string()
            } else if path.contains("/login/oauth/access_token") {
                // The first poll waits; the second completes.
                if poll_count.fetch_add(1, Ordering::SeqCst) == 0 {
                    r#"{"error":"authorization_pending"}"#.to_string()
                } else {
                    r#"{"access_token":"gh-device-token"}"#.to_string()
                }
            } else if path.contains("/copilot_internal/v2/token") {
                r#"{"token":"tid=mock;exp=9999999999;proxy-ep=proxy.individual.githubcopilot.com;","expires_at":9999999999}"#.to_string()
            } else if path.ends_with("/models") {
                MODELS_FIXTURE.to_string()
            } else {
                "{}".to_string()
            };
            let payload = forced.unwrap_or(payload);
            let reason = match status {
                200 => "OK",
                400 => "Bad Request",
                401 => "Unauthorized",
                404 => "Not Found",
                _ => "Error",
            };
            let content_type = if path.contains("/chat/completions") {
                "text/event-stream"
            } else {
                "application/json"
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\n\
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
        polls,
        scripted,
    }
}

impl Mock {
    /// Answer the next request whose path contains `needle` with
    /// `status` (`None` keeps the default payload for the path).
    pub fn fail_once(&self, needle: &str, status: u16, payload: Option<&str>) {
        self.scripted.lock().expect("script").push((
            needle.to_string(),
            status,
            payload.map(str::to_string),
        ));
    }

    /// Requests whose path, headers, or body contain `needle`.
    pub fn requests_of(&self, needle: &str) -> Vec<String> {
        self.requests
            .lock()
            .expect("requests")
            .iter()
            .filter(|(path, headers, body)| {
                path.contains(needle) || headers.contains(needle) || body.contains(needle)
            })
            .map(|(path, headers, body)| format!("{path} | {headers} | {body}"))
            .collect()
    }
}

struct Always;
impl lca_permissions::PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

/// A real capability engine pointed at the mock, with the ad hoc
/// loopback grant the login modal would have attached (FR-PERM-16).
pub fn sandbox(
    namespace: &str,
    name: &str,
    mock: &Mock,
    grants: lca_tools::CapabilityGrants,
) -> Arc<lca_tools::Capabilities> {
    let root = lca_testkit::scratch_path(&format!("lca-{namespace}-{name}"));
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
    let mut grants = grants;
    grants
        .adhoc_net
        .push(lca_permissions::parse_net_pattern("127.0.0.1").expect("loopback pattern"));
    let cap = Arc::new(lca_tools::Capabilities::new(
        namespace,
        grants,
        roots,
        Arc::new(Mutex::new(Always)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    ));
    cap.credentials_set("api_base", &mock.base).expect("seed");
    cap.credentials_set(
        "device_code_endpoint",
        &format!("{}/login/device/code", mock.base),
    )
    .expect("seed");
    cap.credentials_set(
        "device_token_endpoint",
        &format!("{}/login/oauth/access_token", mock.base),
    )
    .expect("seed");
    cap.credentials_set(
        "copilot_token_endpoint",
        &format!("{}/copilot_internal/v2/token", mock.base),
    )
    .expect("seed");
    cap
}
