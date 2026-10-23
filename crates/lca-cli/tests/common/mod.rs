//! Shared e2e helpers: SSE builders, the loopback mock provider, the
//! sandboxed HOME/state fixture, the OCI mock registry helpers, and the
//! tmux/session-log readers.

//!
//! Verifies: NFR-22 (the suite runs with no network access: the only
//! server is a loopback mock started by the test itself).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
#![allow(dead_code)] // a shared helper module: each test crate uses a subset.
#![allow(unused_imports)] // ...and re-exports are per-binary for the same reason.

// gh #413: the socket-name and reaping rules live in their own module so the
// 1,200-line ceiling (gate 11) has room for the harness's other helpers.
#[cfg(unix)]
mod sockets;
#[cfg(unix)]
pub use sockets::reap_stale_sockets;
// gh #447: the readiness primitives (settle, input-ready probe, retry-once).
// Their own module for the same ceiling reason, and so the #447 family has one
// home rather than five rows each growing a `sleep`.
#[cfg(unix)]
mod readiness;
#[cfg(unix)]
pub use readiness::{retry_once, settle_stats};
// The contract rows read these; most e2e binaries use only `tmux_socket`, so
// the unused-in-this-binary warning is expected and silenced with the module's
// own `dead_code` allowance.
#[cfg(unix)]
use sockets::tmux_socket;
#[cfg(unix)]
pub use sockets::{socket_path_for_tests as tmux_socket_path_for_tests, tmux_socket_for_tests};

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;

#[derive(Clone)]
pub enum Reply {
    /// A complete SSE body.
    Sse(String),
    /// An SSE body the mock serves after a delay, so a test can cancel a
    /// turn while it is genuinely in flight.
    SseAfter(u64, String),
    /// An HTTP error status with a JSON error body.
    Status(u16),
}

pub fn sse_text(text: &str) -> String {
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\ndata: [DONE]\n\n")
}

pub fn sse_text_with_usage(text: &str, prompt: u64, cached: u64) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n\
         data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{prompt},\"completion_tokens\":5,\"prompt_tokens_details\":{{\"cached_tokens\":{cached}}}}}}}\n\n\
         data: [DONE]\n\n"
    )
}

pub fn sse_tool_call(name: &str, args: &str) -> String {
    let escaped = args.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"call-e2e\",\"function\":{{\"name\":\"{name}\",\"arguments\":\"\"}}}}]}}}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"function\":{{\"arguments\":\"{escaped}\"}}}}]}}}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\ndata: [DONE]\n\n"
    )
}

pub struct Mock {
    pub addr: SocketAddr,
    requests: std::sync::Arc<Mutex<Vec<String>>>,
    bodies: std::sync::Arc<Mutex<Vec<String>>>,
}

impl Mock {
    pub fn url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().expect("lock").len()
    }

    /// Every request body the mock received, in order (G2's receipt: the
    /// model id the provider call actually carries, not the picker label).
    pub fn bodies(&self) -> Vec<String> {
        self.bodies.lock().expect("lock").iter().cloned().collect()
    }
}

pub async fn start_mock(replies: Vec<Reply>) -> Mock {
    use tokio::net::TcpListener;

    let replies = std::sync::Arc::new(Mutex::new(VecDeque::from(replies)));
    let requests = std::sync::Arc::new(Mutex::new(Vec::new()));
    let bodies = std::sync::Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let recorded = requests.clone();
    let recorded_bodies = bodies.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let replies = replies.clone();
            let recorded = recorded.clone();
            let recorded_bodies = recorded_bodies.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                    let replies = replies.clone();
                    let recorded = recorded.clone();
                    let recorded_bodies = recorded_bodies.clone();
                    async move {
                        let (parts, mut body) = req.into_parts();
                        let mut bytes = Vec::new();
                        while let Some(Ok(frame)) = body.frame().await {
                            if let Ok(data) = frame.into_data() {
                                bytes.extend_from_slice(&data);
                            }
                        }
                        recorded.lock().expect("lock").push(format!(
                            "{} {}",
                            parts.method,
                            parts.uri.path()
                        ));
                        recorded_bodies
                            .lock()
                            .expect("lock")
                            .push(String::from_utf8_lossy(&bytes).into_owned());
                        // `GET /models` is the provider's model discovery
                        // (gh #31 review): answer it with a list, JSON like
                        // an OpenAI-shaped endpoint, and leave the reply
                        // queue alone so a turn's scripted answers stay in
                        // step.
                        if parts.method == hyper::Method::GET
                            && parts.uri.path().ends_with("/models")
                        {
                            let body = serde_json::to_vec(&serde_json::json!({
                                "data": [{"id": "zen-free"}, {"id": "zen-lite"}]
                            }))
                            .expect("serialize");
                            let response = Response::builder()
                                .status(200)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(body)))
                                .expect("response");
                            return Ok::<_, std::convert::Infallible>(response);
                        }
                        let reply = replies
                            .lock()
                            .expect("lock")
                            .pop_front()
                            .unwrap_or_else(|| Reply::Sse(sse_text("fallback")));
                        let response = match reply {
                            Reply::Sse(body) => Response::builder()
                                .status(200)
                                .header("content-type", "text/event-stream")
                                .body(Full::new(Bytes::from(body)))
                                .expect("response"),
                            Reply::SseAfter(delay_ms, body) => {
                                tokio::time::sleep(std::time::Duration::from_millis(delay_ms))
                                    .await;
                                Response::builder()
                                    .status(200)
                                    .header("content-type", "text/event-stream")
                                    .body(Full::new(Bytes::from(body)))
                                    .expect("response")
                            }
                            Reply::Status(status) => Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(
                                    r#"{"error":{"message":"mock failure"}}"#,
                                )))
                                .expect("response"),
                        };
                        Ok::<_, std::convert::Infallible>(response)
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            });
        }
    });
    Mock {
        addr,
        requests,
        bodies,
    }
}

pub struct Sandbox {
    pub root: PathBuf,
    pub home: PathBuf,
    pub data: PathBuf,
}

pub fn sandbox(name: &str) -> Sandbox {
    let root = lca_testkit::scratch_path(&format!("lca-cli-e2e-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let data = root.join("data");
    std::fs::create_dir_all(&home).expect("mkdir");
    std::fs::create_dir_all(&data).expect("mkdir");
    std::fs::create_dir_all(root.join("project")).expect("mkdir");
    Sandbox { root, home, data }
}

impl Sandbox {
    /// Where the binary under test keeps its state: `$HOME/.lca` on every
    /// platform (R7). `HOME` is the only knob that matters now, and the
    /// sandbox sets it; the XDG/APPDATA variables the spawn still carries
    /// are inert for the data directory and kept only because some
    /// dependency may read them.
    pub fn state_dir(&self) -> PathBuf {
        self.home.join(".lca")
    }
}

/// Consent the loopback mock host would get from the login modal in
/// production (FR-PERM-16); here the test stands in for the user's
/// approval, recorded in the grant store exactly as the real flow
/// writes it.
impl Sandbox {
    pub fn approve_loopback_net(&self, extra: serde_json::Value) {
        let project = std::fs::canonicalize(self.project()).expect("canonical project");
        let mut entry = serde_json::json!({
            "trusted": true,
            "net_patterns": ["127.0.0.1"],
        });
        if let (Some(object), Some(more)) = (entry.as_object_mut(), extra.as_object()) {
            for (key, value) in more {
                object.insert(key.clone(), value.clone());
            }
        }
        let grants = serde_json::json!({
            "version": 1,
            "projects": { project.to_string_lossy(): entry },
        });
        std::fs::create_dir_all(self.state_dir()).expect("mkdir lca");
        std::fs::write(
            self.state_dir().join("grants.json"),
            serde_json::to_vec_pretty(&grants).expect("grants serialize"),
        )
        .expect("write grants");
    }
}

impl Sandbox {
    pub fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    /// Seed a provider's credential namespace, the way `/login` would for a
    /// sandboxed (WASM) extension: it cannot see the host environment, so
    /// its endpoint and key live in its own store.
    pub fn write_credentials(&self, namespace: &str, values: serde_json::Value) {
        let dir = self.state_dir().join("credentials");
        std::fs::create_dir_all(&dir).expect("mkdir credentials");
        std::fs::write(
            dir.join(format!("{namespace}.json")),
            serde_json::to_vec_pretty(&values).expect("credentials serialize"),
        )
        .expect("write credentials");
    }

    pub fn run(&self, mock: Option<&Mock>, args: &[&str]) -> Output {
        self.run_env(mock, args, &[])
    }

    pub fn run_env(&self, mock: Option<&Mock>, args: &[&str], extra: &[(&str, &str)]) -> Output {
        // Default consent for the loopback mock, unless the test
        // already wrote its own store (a disable test, say).
        if mock.is_some() && !self.state_dir().join("grants.json").exists() {
            self.approve_loopback_net(serde_json::json!({}));
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_lca"));
        command
            .args(args)
            .current_dir(self.project())
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_DATA_HOME", &self.data)
            .env("APPDATA", &self.data)
            .env("LOCALAPPDATA", &self.data)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            // The suite asks for no outbound update check (FR-CFG-6's
            // knob): spawned sessions must not phone home from CI.
            .env("LCA_UPDATE_CHECK", "false")
            .env_remove("OPENAI_BASE_URL")
            .env_remove("OPENAI_API_KEY")
            .env_remove("OPENAI_MODEL")
            .env_remove("LCA_MODEL")
            .env_remove("LCA_PROVIDER")
            .env_remove("LCA_TOOL_MAX_ITERATIONS")
            .stdin(Stdio::null());
        if let Some(mock) = mock {
            command
                .env("OPENAI_BASE_URL", mock.url())
                .env("OPENAI_API_KEY", "test-key")
                // Issue #3: there is no implicit default model, so the
                // harness picks one explicitly (a test may still remove it).
                .env("OPENAI_MODEL", "test-model");
        }
        for (key, value) in extra {
            command.env(key, value);
        }
        command.output().expect("spawn lca")
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn json_lines(output: &Output) -> Vec<serde_json::Value> {
    stdout(output)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|err| panic!("bad json {line:?}: {err}"))
        })
        .collect()
}

pub fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

// Verifies: FR-CORE-3 (a prompt flag runs one turn headless and writes the
// result to standard output), FR-CORE-1 (one executable, no separate
// language runtime: the subprocess is the only program this test starts),
// FR-CFG-3 and FR-CFG-4 (no telemetry exists: the process makes exactly one
// outbound request, the one the script asked for)
pub const OPENAI_COMPONENT: &[u8] =
    include_bytes!("../../../../extensions/openai-compatible/fixtures/component.wasm");
pub const OPENAI_MANIFEST: &str =
    include_str!("../../../../extensions/openai-compatible/extension.toml");
pub const SKILLS_COMPONENT: &[u8] =
    include_bytes!("../../../../extensions/skills/fixtures/component.wasm");
pub const SKILLS_MANIFEST: &str = include_str!("../../../../extensions/skills/extension.toml");
pub const CODEX_COMPONENT: &[u8] =
    include_bytes!("../../../../extensions/codex/fixtures/component.wasm");
pub const CODEX_MANIFEST: &str = include_str!("../../../../extensions/codex/extension.toml");

/// An anonymous OCI registry serving the two-layer convention (config
impl Sandbox {
    /// Run with piped stdin (the consent prompt reads it; EOF declines).
    pub fn run_with_stdin(&self, mock: Option<&Mock>, args: &[&str], stdin: &str) -> Output {
        use std::process::Stdio as Std;
        let mut command = Command::new(env!("CARGO_BIN_EXE_lca"));
        command
            .args(args)
            .current_dir(self.project())
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_DATA_HOME", &self.data)
            // The same Windows pin `run_env` carries: without it the
            // child resolves data_dir() to the runner's real profile
            // and the install lands outside the sandbox.
            .env("APPDATA", &self.data)
            .env("LOCALAPPDATA", &self.data)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            // The suite asks for no outbound update check (FR-CFG-6's
            // knob): spawned sessions must not phone home from CI.
            .env("LCA_UPDATE_CHECK", "false")
            .env_remove("OPENAI_MODEL")
            .env_remove("LCA_PROVIDER")
            .stdin(Std::piped())
            .stdout(Std::piped())
            .stderr(Std::piped());
        if let Some(mock) = mock {
            command
                .env("OPENAI_BASE_URL", mock.url())
                .env("OPENAI_API_KEY", "test-key")
                // Issue #3: there is no implicit default model, so the
                // harness picks one explicitly (a test may still remove it).
                .env("OPENAI_MODEL", "test-model");
        }
        let mut child = command.spawn().expect("spawn lca");
        {
            use std::io::Write as _;
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(stdin.as_bytes())
                .expect("write consent");
        }
        child.wait_with_output().expect("run lca")
    }

    /// The installed-extension tree under this sandbox's data dir.
    pub fn extensions_root(&self) -> PathBuf {
        self.state_dir().join("extensions")
    }

    /// Hand-install a fixture component (gh #177): the manifest, the
    /// digest-named component, and the lockfile record - the same tree
    /// `ext install` writes, without the registry round-trip. The
    /// component bytes ride `include_bytes!` (rebuilt at publish time
    /// like every fixture component).
    pub fn install_component(&self, name: &str, manifest: &str, component: &[u8]) {
        use sha2::{Digest, Sha256};
        let dir = self.extensions_root().join(name);
        std::fs::create_dir_all(&dir).expect("mkdir extension");
        std::fs::write(dir.join("extension.toml"), manifest).expect("write manifest");
        let digest = format!("sha256:{:x}", Sha256::digest(component));
        let file = digest.replace(':', "-");
        std::fs::write(dir.join(format!("{file}.wasm")), component).expect("write component");
        let field = |key: &str| {
            manifest
                .lines()
                .find_map(|line| line.trim().strip_prefix(key))
                .map(str::trim)
                .and_then(|rest| rest.strip_prefix('='))
                .map(str::trim)
                .and_then(|value| value.strip_prefix('"'))
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or_default()
                .to_string()
        };
        let lock = serde_json::json!({
            "version": 1,
            "extensions": {
                name: {
                    "digest": digest,
                    "source": "test-fixture",
                    "grant_hash": "",
                    "version": field("version"),
                    "abi": field("abi"),
                }
            }
        });
        std::fs::write(
            self.extensions_root().join("lockfile.json"),
            serde_json::to_vec_pretty(&lock).expect("lockfile serialize"),
        )
        .expect("write lockfile");
    }

    /// Write the grant store (ad hoc loopback net consent, the stand-in
    /// for FR-PERM-16's modal in this offline exit test). An empty file
    /// first, so no consent exists until the test says so.
    pub fn write_grants(&self, with_loopback_net: bool) {
        let project = std::fs::canonicalize(self.project()).expect("canonical project");
        let mut entry = serde_json::json!({ "trusted": true });
        if with_loopback_net {
            entry["net_patterns"] = serde_json::json!(["127.0.0.1"]);
        }
        let grants = serde_json::json!({
            "version": 1,
            "projects": { project.to_string_lossy(): entry },
        });
        std::fs::create_dir_all(self.state_dir()).expect("mkdir lca");
        std::fs::write(
            self.state_dir().join("grants.json"),
            serde_json::to_vec_pretty(&grants).expect("grants serialize"),
        )
        .expect("write grants");
    }
}

// Verifies: the Phase 5 exit test end to end - OCI install with the
// consent screen shown before anything is written and a decline
// writing nothing (FR-PERM-2), HTTPS-archive install with its own
// consent, both recorded in the lockfile (FR-DIST-6), a turn that runs
// against the INSTALLED provider loaded by its recorded digest with no
// moving tag consulted (FR-DIST-8), the denial journal behind
// `ext info` (FR-EXT-9), an update that finds itself up to date, and a
// remove (FR-DIST-1/2/5/9).
#[cfg(unix)]
pub fn tmux_available() -> bool {
    let present = Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    // #104: CI sets `LCA_REQUIRE_TMUX`, which turns a silent skip into
    // a named failure — a leg that exercises none of the terminal
    // contract must go red, never green-by-absence. Local runs leave
    // it unset and keep the soft skip.
    if !present && std::env::var("LCA_REQUIRE_TMUX").is_ok() {
        panic!("LCA_REQUIRE_TMUX is set but tmux is not installed");
    }
    present
}

#[cfg(unix)]
pub struct Tmux {
    pub name: String,
    /// The exact `new-session` command this pane was spawned with, kept so a
    /// pane that never painted can be re-spawned (gh #413's class).
    spawn_command: std::sync::Mutex<Option<String>>,
}

// The real-terminal tests get their own private tmux socket: never the
// default socket (standing rule), and separate from the manual drive
// socket (`lca-tui`) so a test run cannot disturb a live drive.
//
// **Per test-binary process** (gh #413's class): one fixed name was shared by
// every real-terminal test *and* by any second suite on the same machine, so
// two concurrent runs drove each other's servers. The pid gives each
// `cargo nextest run -p lca-cli` its own server; the `OnceLock` means every
// `Tmux` created in one binary still shares that one server, which is what
// the tests expect.
#[cfg(unix)]
impl Tmux {
    pub fn new(tag: &str) -> Tmux {
        reap_stale_sockets();
        let name = format!("lca-smoke-{}-{tag}", std::process::id());
        let _ = Command::new("tmux")
            .args(["-L", tmux_socket(), "kill-session", "-t", &name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        Tmux {
            name,
            spawn_command: std::sync::Mutex::new(None),
        }
    }

    pub fn tmux(args: &[&str]) -> Output {
        let mut full = vec!["-L", tmux_socket()];
        full.extend_from_slice(args);
        Command::new("tmux").args(&full).output().expect("run tmux")
    }

    /// The pane's own cursor column (R5's receipt: the terminal, not the
    /// app, says where the caret is).
    pub fn cursor_x(&self) -> i64 {
        let out = Self::tmux(&["display-message", "-p", "-t", &self.name, "#{cursor_x}"]);
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .expect("tmux reports a cursor column")
    }

    /// The cursor column once it has reached `expected`, or the last
    /// reading when `timeout` passes. `send-keys` injects input and the
    /// app paints asynchronously, so a single immediate read races the
    /// render on a loaded runner (this row flaked twice in full-suite
    /// runs before the wait existed, 2026-10-02). The expected value and
    /// the assertion around it are unchanged - only the wait is new, and
    /// a column that never arrives still fails the caller's equality.
    pub fn cursor_x_settled(&self, expected: i64, timeout: std::time::Duration) -> i64 {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let x = self.cursor_x();
            if x == expected || std::time::Instant::now() >= deadline {
                return x;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }

    /// A genuine paste through tmux's own paste machinery (R6). The pane
    /// enabled bracketed-paste mode (`?2004h`), so tmux wraps the buffer
    /// in `ESC[200~…ESC[201~` before writing it. `send-keys` cannot do
    /// this: it synthesizes key events, which is typing by another name.
    pub fn paste_text(&self, text: &str) {
        use std::io::Write as _;
        let buffer = format!("lca-paste-{}", std::process::id());
        let mut child = Command::new("tmux")
            .args(["-L", tmux_socket(), "load-buffer", "-b", &buffer, "-"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("spawn tmux load-buffer");
        child
            .stdin
            .as_mut()
            .expect("load-buffer stdin")
            .write_all(text.as_bytes())
            .expect("write paste buffer");
        assert!(child.wait().expect("wait load-buffer").success());
        let out = Self::tmux(&["paste-buffer", "-p", "-b", &buffer, "-t", &self.name, "-d"]);
        assert!(
            out.status.success(),
            "tmux paste-buffer: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Inject raw bytes in fragments, with a small sleep between each, so
    /// `stdin_buffer`'s reassembly and its dual timeouts are exercised at
    /// the byte level (R6). `send-keys -l` sends the string's bytes
    /// literally, escape bytes included.
    pub fn send_raw_fragmented(&self, fragments: &[&str]) {
        for fragment in fragments {
            let out = Self::tmux(&["send-keys", "-t", &self.name, "-l", fragment]);
            assert!(
                out.status.success(),
                "tmux send-keys -l: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    pub fn capture(&self) -> String {
        let out = Self::tmux(&["capture-pane", "-t", &self.name, "-p"]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Capture pane including full scrollback history (`capture-pane -S -`).
    pub fn capture_with_history(&self) -> String {
        let out = Self::tmux(&["capture-pane", "-t", &self.name, "-p", "-S", "-"]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// The pane with its SGR intact (`capture-pane -e`): the receipt the
    /// color work asserts against (R5).
    pub fn capture_e(&self) -> String {
        let out = Self::tmux(&["capture-pane", "-t", &self.name, "-p", "-e"]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Poll the SGR pane until `needle` appears (for receipts that must be
    /// read while a state is on screen).
    pub fn wait_for_e(&self, needle: &str, timeout: std::time::Duration) -> String {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let pane = self.capture_e();
            if pane.contains(needle) || std::time::Instant::now() >= deadline {
                return pane;
            }
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }

    pub fn spawn(
        &self,
        sandbox: &Sandbox,
        mock: Option<&Mock>,
        with_key: bool,
        extra_env: &[(&str, &str)],
        args: &[&str],
    ) {
        let key = if with_key {
            " OPENAI_API_KEY=test-key"
        } else {
            ""
        };
        let endpoint = mock
            .map(|mock| format!(" OPENAI_BASE_URL={}", mock.url()))
            .unwrap_or_default();
        // Issue #3: no implicit default model, so a mock-backed session picks
        // one explicitly. `extra_env` (appended last) can still override it.
        let model = if mock.is_some() {
            " OPENAI_MODEL=test-model"
        } else {
            ""
        };
        let extra: String = extra_env
            .iter()
            .map(|(key, value)| format!(" {key}={value}"))
            .collect();
        let arguments: String = args.iter().map(|arg| format!(" {arg}")).collect();
        let command = format!(
            "cd {project} && HOME={home} USERPROFILE={home} XDG_DATA_HOME={data} \
             APPDATA={data} LOCALAPPDATA={data} XDG_CONFIG_HOME={config} \
             LCA_UPDATE_CHECK=false{endpoint}{key}{model}{extra} {bin}{arguments}",
            project = sandbox.project().display(),
            home = sandbox.home.display(),
            data = sandbox.data.display(),
            config = sandbox.home.join(".config").display(),
            bin = env!("CARGO_BIN_EXE_lca"),
        );
        self.spawn_raw(&command);
    }

    /// Start this session with an arbitrary shell command.
    ///
    /// The harness's own seam: `spawn` builds the `lca` command line, and the
    /// contract row drives a fake one to prove `wait_for`'s empty-pane
    /// self-healing (gh #413). The command is remembered either way, so a
    /// blank pane can be re-spawned.
    pub fn spawn_raw(&self, command: &str) {
        *self
            .spawn_command
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(command.to_string());
        let out = Self::new_session(&self.name, command);
        assert!(
            out.status.success(),
            "tmux new-session: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `new-session -d` for one named session at the harness's standard size.
    fn new_session(name: &str, command: &str) -> Output {
        Self::tmux(&[
            "new-session",
            "-d",
            "-s",
            name,
            "-x",
            "140",
            "-y",
            "40",
            command,
        ])
    }

    /// Keep the pane (and its scrollback) after the process exits.
    ///
    /// tmux reaps a dead pane and with it the scrollback, so a row that reads
    /// **what the process printed on the way out** - gh #403's exit hint - must
    /// ask for this *before* sending the quit. Rows that assert a pane is gone
    /// (`wait_for_pane_end`) deliberately do not.
    pub fn remain_on_exit(&self) {
        let out = Self::tmux(&["set-option", "-t", &self.name, "remain-on-exit", "on"]);
        assert!(
            out.status.success(),
            "tmux set-option remain-on-exit: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub fn send(&self, keys: &[&str]) {
        let mut args = vec!["send-keys", "-t", &self.name];
        args.extend_from_slice(keys);
        let out = Self::tmux(&args);
        assert!(
            out.status.success(),
            "tmux send-keys: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Send literal bytes (SGR-1006 mouse synthesis: tmux key names
    /// cannot spell `ESC[<`, so the bytes go literally).
    pub fn send_literal(&self, bytes: &str) {
        let out = Self::tmux(&["send-keys", "-t", &self.name, "-l", bytes]);
        assert!(
            out.status.success(),
            "tmux send-keys -l: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// How long a pane gets before the harness treats it as not-yet-painted.
    ///
    /// The caller's `timeout` stays the contract for *product* assertions; this
    /// is the extra the harness allows a blank pane, because a blank pane says
    /// nothing about the product. Measured on this box: a WASM-bearing session
    /// takes 21-23 s to first paint on an idle 4-core machine (the component
    /// JIT), against the 30 s budgets the rows carry — so two suites at once
    /// halve the CPU and blow the budget on rows that are perfectly correct.
    ///
    /// Scaling by the machine's load average is the honest knob: it is the
    /// quantity that actually rose, and it costs nothing when the box is idle.
    ///
    /// `pub(crate)` since gh #447: the readiness primitives in the sibling
    /// `readiness` module apply the same allowance, and two budget rules that
    /// disagreed would be worse than either.
    pub(crate) fn load_factor() -> u32 {
        let load = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok())
            .unwrap_or(0.0);
        // 1.0x below light load, then +1x per core-ish of pressure, capped so a
        // pathological runner cannot extend a suite for hours.
        let factor = (load / 4.0).floor() as u32 + 1;
        factor.clamp(1, 4)
    }

    /// Poll the pane until `needle` appears (150 ms cadence). The timeout
    /// is wall-clock margin for a loaded machine, not a performance claim:
    /// a full parallel `cargo test -p lca-cli` blew a 10 s paste budget
    /// once while every focused run passed (2026-10-01).
    ///
    /// **Self-healing on an empty pane** (gh #413's class). If the budget
    /// expires and the pane is *blank*, the session never painted - the
    /// server could not start the process under load, which is a harness
    /// condition, not a product assertion. That case re-spawns the same named
    /// session with the same command **once** and re-polls a bounded second
    /// time, saying so on stderr so the run stays honest about what happened.
    ///
    /// A pane with *content* that lacks the needle keeps the panic: a painted
    /// screen missing its expected text is a real failure.
    pub fn wait_for(&self, needle: &str, timeout: std::time::Duration) -> String {
        // A loaded machine slows first paint; the caller's budget is about the
        // product, so the harness adds its own allowance for a pane that has
        // not painted at all. A painted-but-wrong pane is checked below and is
        // unaffected.
        let budget = timeout * Self::load_factor();
        let pane = self.poll_for(needle, budget);
        if let Some(pane) = pane {
            return pane;
        }
        let first = self.capture();
        if !first.trim().is_empty() {
            // Painted but wrong: a genuine product failure, unchanged.
            panic!("`{needle}` never appeared in the pane:\n{first}");
        }
        // Blank pane after a whole budget: the terminal never came up.
        match self.respawn_blank(needle, budget) {
            Some(_) => {}
            None => panic!("`{needle}` never appeared and the re-spawn failed; see stderr"),
        }
        // The second attempt starts its own deadline (a re-spawned process
        // begins from zero). Note what this cannot fix: a WASM-bearing
        // session takes 26-28 s to first paint on an idle box (the component
        // JIT), so two concurrent suites on four cores push it past *any*
        // fixed per-attempt budget. The harness heals a terminal that did not
        // come up; it cannot manufacture CPU.
        match self.poll_for(needle, budget) {
            Some(pane) => pane,
            // The second chance is bounded and final: a pane that still lacks
            // the needle after a re-spawn is a failure either way.
            None => panic!(
                "`{needle}` never appeared in the pane after a re-spawn:\n{}",
                self.capture()
            ),
        }
    }

    /// Poll until `needle` appears, or `None` when the budget spends.
    fn poll_for(&self, needle: &str, timeout: std::time::Duration) -> Option<String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let pane = self.capture();
            if pane.contains(needle) {
                return Some(pane);
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }

    /// Re-spawn this session once, because a **blank** pane at the end of a
    /// whole budget is a terminal that never came up (gh #413) — a harness
    /// condition, never a product one. Shared by `wait_for` and the settle
    /// loop so there is exactly one healing path (gh #447).
    ///
    /// `None` means the re-spawn itself failed; the caller decides how to say
    /// so, because the message that matters includes what *it* was waiting for.
    fn respawn_blank(&self, what: &str, budget: std::time::Duration) -> Option<String> {
        let command = self
            .spawn_command
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(command) = command else {
            panic!(
                "`{what}` never appeared and the pane is blank; this session was not \
                 spawned through `Tmux::spawn`, so the harness cannot re-spawn it"
            );
        };
        eprintln!(
            "harness: the pane for `{}` was blank after {budget:?}; re-spawning the \
             session once (empty pane = the terminal did not come up, gh #413)",
            self.name
        );
        let _ = Command::new("tmux")
            .args(["-L", tmux_socket(), "kill-session", "-t", &self.name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let out = Self::new_session(&self.name, &command);
        if !out.status.success() {
            // `kill-session` on the last session stops the server, so the
            // re-spawn can race a socket that is going away ("server exited
            // unexpectedly"). One more attempt is the honest repair: the
            // second `new-session` starts a fresh server.
            let retry = Self::new_session(&self.name, &command);
            if !retry.status.success() {
                panic!(
                    "`{what}` never appeared and the re-spawn failed: {}",
                    String::from_utf8_lossy(&retry.stderr)
                );
            }
        }
        Some(self.capture())
    }

    pub fn resize(&self, cols: u32, rows: u32) -> String {
        let out = Self::tmux(&[
            "resize-window",
            "-t",
            &self.name,
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
        ]);
        assert!(
            out.status.success(),
            "tmux resize-window: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        std::thread::sleep(std::time::Duration::from_millis(500));
        self.capture()
    }
}

#[cfg(unix)]
impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", tmux_socket(), "kill-session", "-t", &self.name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        // The server is reaped by the next binary's `reap_stale_sockets`:
        // a test process is not unwound when nextest finishes with it, so a
        // counter here would never reach zero and 134 sockets accumulated.
    }
}

/// One tmux session on its own socket name; dropping it kills only that
/// session, never the server or anyone else's panes.
pub fn find_session_log(state: &std::path::Path) -> Option<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, found: &mut Option<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.file_name().is_some_and(|name| name == "log.jsonl") {
                *found = Some(path);
            }
        }
    }
    let mut found = None;
    walk(state, &mut found);
    found
}

#[cfg(unix)]
pub fn find_session_end(state: &std::path::Path) -> Option<String> {
    let path = find_session_log(state)?;
    let text = std::fs::read_to_string(&path).ok()?;
    text.contains("\"t\":\"session-end\"").then_some(text)
}

/// Wait for the interface to exit when no session log exists to
/// carry a session-end marker (gh #122: a quit without messages
/// writes nothing, so there is no marker to wait for - the dead
/// pane is the receipt).
#[cfg(unix)]
pub fn wait_for_pane_end(session: &Tmux, timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if session.capture().trim().is_empty() {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("the interface never exited");
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

#[cfg(unix)]
pub fn wait_for_session_end(state: &std::path::Path, timeout: std::time::Duration) -> String {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(text) = find_session_end(state) {
            return text;
        }
        if std::time::Instant::now() > deadline {
            panic!("no session-end record under {}", state.display());
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

// Verifies: the real-terminal checklist (docs/testing-plan.md section 14):
// startup renders, a scripted turn streams and renders, the permission modal
// asks and answers, a resize re-renders, and a clean quit writes
// `session-end`.

// ---------------------------------------------------------------------
// gh #418 Phase F: a two-host OAuth identity provider for the `lca mcp
// login` row.
//
// The same shape `crates/lca-mcp/tests/oauth_multihost.rs` established (two
// distinct loopback hosts, because a `NetPattern` names a host and not a
// host:port pair), reduced to what a CLI row needs. It lives here rather than
// being forked so both suites drive one harness.
// ---------------------------------------------------------------------

/// One loopback host serving scripted JSON by path, on an address that the
/// platform will actually bind.
pub struct ScriptedHost {
    /// `http://<host>:<port>`
    pub base: String,
    /// Paths this host was asked for, in order.
    pub paths: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

/// The second loopback host to use.
///
/// macOS binds `127.0.0.1` and `localhost` only (`EADDRNOTAVAIL` for
/// `127.0.0.2`), so the candidate list is probed rather than assumed.
pub fn second_loopback_host() -> &'static str {
    for candidate in ["127.0.0.2", "localhost"] {
        if std::net::TcpListener::bind((candidate, 0)).is_ok() {
            return candidate;
        }
    }
    "localhost"
}

/// Serve `answers(path) -> (status, body)` on one host.
pub fn scripted_host(
    ip: &str,
    answers: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
) -> ScriptedHost {
    let listener = std::net::TcpListener::bind((ip, 0)).expect("bind");
    let addr = listener.local_addr().expect("addr");
    // The requested host *name*, not the resolved address: on macOS
    // `localhost` resolves to `::1`, and a base built from `addr` would
    // collapse the two hosts into one.
    let base = format!("http://{ip}:{}", addr.port());
    serve(listener, base, answers)
}

/// [`scripted_host`] on an address the caller already bound, with the base URL
/// the caller already chose (an identity provider must name itself in its own
/// metadata).
pub fn scripted_host_on(
    addr: std::net::SocketAddr,
    base: &str,
    answers: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
) -> ScriptedHost {
    let ip = base
        .trim_start_matches("http://")
        .rsplit_once(':')
        .map(|(host, _)| host.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let listener = std::net::TcpListener::bind(addr).expect("bind");
    let base = base.to_string();
    let host = ScriptedHost {
        base: base.clone(),
        paths: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    let recorded = host.paths.clone();
    serve_with(listener, recorded, answers);
    let _ = ip;
    host
}

/// Serve `answers` on an already-bound listener, recording each path.
fn serve(
    listener: std::net::TcpListener,
    base: String,
    answers: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
) -> ScriptedHost {
    let paths: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = paths.clone();
    serve_with(listener, recorded.clone(), answers);
    ScriptedHost { base, paths }
}

/// The challenge header a 401 carries, when the row needs one.
///
/// Set once per test binary: the scripted host answers statically, and the
/// login row is the only consumer of a `WWW-Authenticate`.
pub static CHALLENGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn serve_with(
    listener: std::net::TcpListener,
    recorded: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    answers: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
) {
    use std::io::{Read, Write};
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buffer = vec![0u8; 64 * 1024];
            let read = match stream.read(&mut buffer) {
                Ok(read) if read > 0 => read,
                _ => continue,
            };
            let text = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let head = text.split("\r\n\r\n").next().unwrap_or(&text);
            let path = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            recorded.lock().expect("record").push(path.clone());
            let (status, payload) = answers(&path);
            let reason = if status == 200 { "OK" } else { "Error" };
            let mut response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                payload.len()
            );
            // A 401 carries the challenge the sign-in follows (gh #418
            // Phase D's threading); a 200 never does.
            if status == 401
                && let Some(challenge) = CHALLENGE.get()
            {
                response.push_str(&format!("WWW-Authenticate: {challenge}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&payload);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
}

/// An identity provider answering RFC 9728, RFC 8414, registration, and the
/// token endpoint on its own host.
pub fn oauth_idp_host() -> ScriptedHost {
    let host = second_loopback_host();
    // Bind first so the metadata can name this host: an authorization server
    // that named a *third* host would just move the problem.
    let listener = std::net::TcpListener::bind((host, 0)).expect("bind");
    let addr = listener.local_addr().expect("addr");
    let base = format!("http://{host}:{}", addr.port());
    let named_base = base.clone();
    drop(listener);
    scripted_host_on(addr, &base, move |path| {
        let named = named_base.as_str();
        if path.contains("oauth-protected-resource") {
            (
                200,
                format!(r#"{{"resource":"{named}/mcp","authorization_servers":["{named}"]}}"#),
            )
        } else if path.contains("oauth-authorization-server") {
            (
                200,
                format!(
                    r#"{{"issuer":"{named}","authorization_endpoint":"{named}/authorize","token_endpoint":"{named}/token","registration_endpoint":"{named}/register"}}"#
                ),
            )
        } else if path.contains("/register") {
            (
                200,
                r#"{"client_id":"cli-client","client_secret":"cli-secret"}"#.to_string(),
            )
        } else if path.contains("/token") {
            (
                200,
                r#"{"access_token":"cli-access","refresh_token":"cli-refresh","expires_in":1800,"scope":"resources:read"}"#
                    .to_string(),
            )
        } else {
            (404, "{}".to_string())
        }
    })
}

/// Write the `mcp.json` an OAuth CLI row needs: one HTTP server whose
/// authorization server is named explicitly by `authServerMetadataUrl`.
///
/// gh #467: discovery has three steps (the configured document, the 401
/// challenge's RFC 9728 pointer, then the MCP origin as its own issuer), and
/// this row used to depend on **which one happened to win** - it reached the
/// third on macOS and the second on Linux. That made it a platform-divergent
/// row, which is how a dead fallback stayed invisible. Naming the document here
/// pins the documented first step, so the row takes the same path on every OS.
///
/// The 401 still carries its challenge (the row's scope merge reads it); the
/// configured document simply wins, which is the documented precedence.
pub fn write_oauth_mcp_config(home: &std::path::Path, url: &str, idp_base: &str) {
    let path = home.join(".lca/mcp.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &path,
        format!(
            r#"{{"mcpServers": {{"remote": {{"url": "{url}", "oauth": {{"authServerMetadataUrl": "{idp_base}/.well-known/oauth-authorization-server"}}}}}}}}"#
        ),
    )
    .expect("write mcp.json");
}
