//! Shared e2e helpers: SSE builders, the loopback mock provider, the
//! sandboxed HOME/state fixture, the OCI mock registry helpers, and the
//! tmux/session-log readers.
//!
//! Verifies: NFR-22 (the suite runs with no network access: the only
//! server is a loopback mock started by the test itself).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
#![allow(dead_code)] // a shared helper module: each test crate uses a subset.

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
    Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(unix)]
pub struct Tmux {
    pub name: String,
}

// The real-terminal tests get their own private tmux socket: never the
// default socket (standing rule), and separate from the manual drive
// socket (`lca-tui`) so a test run cannot disturb a live drive.
#[cfg(unix)]
const TMUX_SOCKET: &str = "lca-tui-test";

#[cfg(unix)]
impl Tmux {
    pub fn new(tag: &str) -> Tmux {
        let name = format!("lca-smoke-{}-{tag}", std::process::id());
        let _ = Command::new("tmux")
            .args(["-L", TMUX_SOCKET, "kill-session", "-t", &name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        Tmux { name }
    }

    pub fn tmux(args: &[&str]) -> Output {
        let mut full = vec!["-L", TMUX_SOCKET];
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
            .args(["-L", TMUX_SOCKET, "load-buffer", "-b", &buffer, "-"])
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
        let out = Self::tmux(&[
            "new-session",
            "-d",
            "-s",
            &self.name,
            "-x",
            "140",
            "-y",
            "40",
            &command,
        ]);
        assert!(
            out.status.success(),
            "tmux new-session: {}",
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

    /// Poll the pane until `needle` appears (150 ms cadence). The timeout
    /// is wall-clock margin for a loaded machine, not a performance claim:
    /// a full parallel `cargo test -p lca-cli` blew a 10 s paste budget
    /// once while every focused run passed (2026-10-01).
    pub fn wait_for(&self, needle: &str, timeout: std::time::Duration) -> String {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let pane = self.capture();
            if pane.contains(needle) {
                return pane;
            }
            if std::time::Instant::now() > deadline {
                panic!("`{needle}` never appeared in the pane:\n{pane}");
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
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
            .args(["-L", TMUX_SOCKET, "kill-session", "-t", &self.name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
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
