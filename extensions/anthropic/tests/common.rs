//! Shared mock Anthropic gateway + real capability engine for the
//! conformance journeys: every call through the engine, no real
//! network.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
#![allow(dead_code)] // shared harness: each consumer uses a subset.
use std::io::{Read, Write};
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
    token: Arc<Mutex<String>>,
    scripted: Arc<Mutex<Vec<Scripted>>>,
}

/// The recorded Messages stream every turn replays.
pub const STREAM_FIXTURE: &str = include_str!("../fixtures/messages-stream.sse");

/// Start the mock gateway on a loopback port.
pub fn mock_server() -> Mock {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let token: Arc<Mutex<String>> = Arc::new(Mutex::new(token_payload()));
    let current_token = token.clone();
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

            let payload = if path.contains("/v1/messages") {
                STREAM_FIXTURE.to_string()
            } else if path.contains("/token") {
                current_token.lock().expect("token").clone()
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
            let content_type = if path.contains("/v1/messages") {
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
        token,
        scripted,
    }
}

/// The token reply, overridable per journey.
fn token_payload() -> String {
    r#"{"access_token":"mock-access","refresh_token":"mock-refresh","expires_in":1800,"scope":"user:inference"}"#.to_string()
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

    /// Serve this token payload on the next `/token` calls.
    pub fn set_token(&self, payload: &str) {
        *self.token.lock().expect("token") = payload.to_string();
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
    cap.credentials_set("token_endpoint", &format!("{}/token", mock.base))
        .expect("seed");
    cap.credentials_set("auth_endpoint", &format!("{}/auth", mock.base))
        .expect("seed");
    cap
}
