//! Shared mock remote MCP server + mock IdP for the phase-2
//! journeys (gh #53): every call through the real `net` engine, no
//! live servers. Mirrors the codex mock shape (raw TCP, scripted
//! one-shot failures, recorded requests).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
#![allow(dead_code)] // shared harness: each consumer uses a subset.
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use lca_permissions::{GrantStore, ScopeRoots};

/// One scripted response: the path needle, the status, the payload,
/// and optional extra headers (`None` keeps the default payload).
type Scripted = (String, u16, Option<String>, Vec<(String, String)>);

/// How the mock guards `/mcp`.
#[derive(Clone)]
pub struct AuthRule {
    /// The bearer token it accepts (`None` lets everything through).
    pub expect: Option<String>,
    /// The challenge it answers a rejection with.
    pub challenge: Option<String>,
}

/// One recorded request: method, path, headers, body.
type Recorded = (String, String, String, String);

/// The shared mock: one loopback origin serves the MCP endpoint, the
/// SSE variant, the slow route, and the IdP (`/register`, `/token`,
/// metadata).
pub struct Mock {
    /// The loopback base URL the bridge points at.
    pub base: String,
    /// Every request: method, path, headers, body.
    pub requests: Arc<Mutex<Vec<Recorded>>>,
    auth: Arc<Mutex<AuthRule>>,
    token: Arc<Mutex<String>>,
    scripted: Arc<Mutex<Vec<Scripted>>>,
}

/// Start the mock on a loopback port.
pub fn mock_server() -> Mock {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let base = format!("http://{addr}");
    let requests: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let auth: Arc<Mutex<AuthRule>> = Arc::new(Mutex::new(AuthRule {
        expect: None,
        challenge: None,
    }));
    let current_auth = auth.clone();
    let token: Arc<Mutex<String>> = Arc::new(Mutex::new(token_payload("mock-access")));
    let current_token = token.clone();
    let scripted: Arc<Mutex<Vec<Scripted>>> = Arc::new(Mutex::new(Vec::new()));
    let script = scripted.clone();
    let base_for_thread = base.clone();
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
            let mut parts = text.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("/").to_string();
            let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
            let headers = text.split("\r\n\r\n").next().unwrap_or("").to_string();
            recorded.lock().expect("requests").push((
                method,
                path.clone(),
                headers.clone(),
                body.clone(),
            ));

            // A slow route that never answers inside a test budget:
            // handled on its own thread so one hanging call never
            // holds up the accept loop for the other tests.
            if path.contains("/slow") {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                    );
                });
                continue;
            }

            let mut scripted = script.lock().expect("script");
            let hit = scripted
                .iter()
                .position(|(needle, _, _, _)| path.contains(needle) || body.contains(needle))
                .map(|index| scripted.remove(index));
            drop(scripted);
            let (status, forced, extra) = match hit {
                Some((_, status, payload, extra)) => (status, payload, extra),
                None => (200, None, Vec::new()),
            };

            // A scripted hit answers verbatim (status, payload, extra
            // headers); everything else routes by path below.
            let (status, payload, mut extra) = match (status, forced) {
                (status, Some(payload)) => (status, payload, extra),
                (scripted, None) if scripted != 200 => (scripted, "{}".to_string(), extra),
                _ => route(
                    &path,
                    &body,
                    &headers,
                    extra,
                    &current_auth,
                    &current_token,
                    &base_for_thread,
                ),
            };
            if path.contains("-sse") && status == 200 {
                extra.push(("content-type".to_string(), "text/event-stream".to_string()));
            }
            let reason = match status {
                200 => "OK",
                202 => "Accepted",
                401 => "Unauthorized",
                404 => "Not Found",
                _ => "Error",
            };
            let mut response =
                format!("HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\n");
            for (name, value) in &extra {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            // Session tracking pi's client keeps: the mock mints one id
            // on `initialize` so the round-trip test has something to
            // carry.
            if body.contains("\"method\":\"initialize\"") {
                response.push_str("Mcp-Session-Id: mock-session-1\r\n");
            }
            response.push_str(&format!(
                "content-length: {}\r\nconnection: close\r\n\r\n{}",
                payload.len(),
                payload
            ));
            let _ = stream.write_all(response.as_bytes());
        }
    });
    Mock {
        base,
        requests,
        auth,
        token,
        scripted,
    }
}

/// The token reply, overridable per journey (the mock cannot tell
/// callers apart; each test seeds the payload it wants).
fn token_payload(access: &str) -> String {
    format!(
        r#"{{"access_token":"{access}","refresh_token":"mock-refresh","expires_in":1800,"scope":"base"}}"#
    )
}

/// Route one unscripted request by path: the MCP endpoints behind
/// the auth gate, the IdP, and metadata. Returns status, payload,
/// and extra headers.
fn route(
    path: &str,
    body: &str,
    headers: &str,
    extra: Vec<(String, String)>,
    current_auth: &Arc<Mutex<AuthRule>>,
    current_token: &Arc<Mutex<String>>,
    base: &str,
) -> (u16, String, Vec<(String, String)>) {
    if path.contains("/mcp") {
        // The auth gate lives here: a rejection carries the
        // configured challenge for the bridge to act on.
        let rule = current_auth.lock().expect("auth").clone();
        let bearer = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("authorization:")
                    .map(|v| v.trim().to_string())
            })
            .unwrap_or_default();
        let allowed = match &rule.expect {
            None => true,
            Some(token) => bearer == format!("bearer {token}"),
        };
        if allowed {
            return (200, mcp_payload(body, path.contains("-sse")), extra);
        }
        let mut challenge = vec![(
            "WWW-Authenticate".to_string(),
            rule.challenge.unwrap_or_else(|| "Bearer".to_string()),
        )];
        challenge.extend(extra.clone());
        return (
            401,
            r#"{"jsonrpc":"2.0","id":0,"error":{"code":-32001,"message":"unauthorized"}}"#
                .to_string(),
            challenge,
        );
    }
    if path.contains("/register") {
        return (
            200,
            r#"{"client_id":"mock-client","client_secret":"mock-secret"}"#.to_string(),
            extra,
        );
    }
    if path.contains("/token") {
        let payload = current_token.lock().expect("token").clone();
        // An error payload answers like a real IdP: 400, so the
        // bridge reads the grant failure off the status.
        let status = if payload.contains("\"error\"") {
            400
        } else {
            200
        };
        return (status, payload, extra);
    }
    if path.contains("oauth-protected-resource") {
        return (
            200,
            format!(r#"{{"resource":"{base}/mcp","authorization_servers":["{base}/idp"]}}"#,),
            extra,
        );
    }
    if path.contains("oauth-authorization-server") {
        return (
            200,
            format!(
                r#"{{"issuer":"{base}/idp","authorization_endpoint":"{base}/authorize","token_endpoint":"{base}/token","registration_endpoint":"{base}/register"}}"#,
            ),
            extra,
        );
    }
    (200, "{}".to_string(), extra)
}

/// Answer one JSON-RPC call the way a remote echo server would (the
/// SSE variant wraps the same message as an event).
fn mcp_payload(body: &str, sse: bool) -> String {
    let request: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let id = request.get("id").cloned().unwrap_or_default();
    let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let message = match method {
        "initialize" => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-11-25",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "remote-echo", "version": "0.1.0"},
            },
        }),
        "tools/list" => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [{
                    "name": "echo",
                    "description": "Echoes its text argument back.",
                    "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
                    "annotations": {"readOnlyHint": true},
                }],
            },
        }),
        "tools/call" => {
            let text = request
                .pointer("/params/arguments/text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"content": [{"type": "text", "text": format!("echo: {text}")}]},
            })
        }
        _ if request.get("id").is_some() => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": "unknown method"},
        }),
        _ => return String::new(),
    };
    let line = serde_json::to_string(&message).expect("mock message serializes");
    if sse {
        format!("event: message\ndata: {line}\n\n")
    } else {
        line
    }
}

impl Mock {
    /// Answer the next request whose path contains `needle` with
    /// `status` (`None` keeps the default payload for the path).
    pub fn fail_once(
        &self,
        needle: &str,
        status: u16,
        payload: Option<&str>,
        extra: Vec<(String, String)>,
    ) {
        self.scripted.lock().expect("script").push((
            needle.to_string(),
            status,
            payload.map(str::to_string),
            extra,
        ));
    }

    /// Guard `/mcp` with a bearer token and an optional challenge.
    pub fn guard(&self, expect: Option<&str>, challenge: Option<&str>) {
        *self.auth.lock().expect("auth") = AuthRule {
            expect: expect.map(str::to_string),
            challenge: challenge.map(str::to_string),
        };
    }

    /// Serve this token payload on `/token`.
    pub fn set_token(&self, payload: &str) {
        *self.token.lock().expect("token") = payload.to_string();
    }

    /// Requests whose method, path, headers, or body contain `needle`.
    pub fn requests_of(&self, needle: &str) -> Vec<String> {
        self.requests
            .lock()
            .expect("requests")
            .iter()
            .filter(|(method, path, headers, body)| {
                method.contains(needle)
                    || path.contains(needle)
                    || headers.contains(needle)
                    || body.contains(needle)
            })
            .map(|(method, path, headers, body)| format!("{method} {path} | {headers} | {body}"))
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

/// A real capability engine pointed at the mock: workspace `fs` for
/// the scope, the ad hoc loopback grant for `net`, the loopback OAuth
/// flow, and the credentials namespace.
pub fn sandbox(
    name: &str,
    mut grants: lca_tools::CapabilityGrants,
) -> Arc<lca_tools::Capabilities> {
    let root = lca_testkit::scratch_path(&format!("lca-mcp-{name}"));
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
    grants
        .adhoc_net
        .push(lca_permissions::parse_net_pattern("127.0.0.1").expect("loopback pattern"));
    grants.oauth = Some(lca_permissions::OAuthSettings {
        redirect_path: "/callback".to_string(),
        timeout_seconds: 30,
    });
    grants.credentials = true;
    Arc::new(lca_tools::Capabilities::new(
        "mcp",
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

/// One remote server pointed at the mock.
pub fn remote_server(mock: &Mock, path: &str) -> mcp::HttpServerConfig {
    mcp::HttpServerConfig {
        name: "remote".to_string(),
        url: format!("{}{path}", mock.base),
        headers: Default::default(),
        timeout_secs: 10,
        oauth: None,
    }
}
