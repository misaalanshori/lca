//! End-to-end tests: the real `lca` binary against a local mock
//! OpenAI-compatible server. Offline, no credentials, deterministic
//! (testing plan sections4 and5: per-test `HOME`/`XDG_DATA_HOME` sandboxes
//! the subprocess).
//!
//! Verifies: NFR-22 (the suite runs with no network access: the only
//! server is a loopback mock started by the test itself).
//!
//! The mock runs on its own multi-thread runtime so the blocking
//! `Command::output()` calls cannot starve it.

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
enum Reply {
    /// A complete SSE body.
    Sse(String),
    /// An HTTP error status with a JSON error body.
    Status(u16),
}

fn sse_text(text: &str) -> String {
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\ndata: [DONE]\n\n")
}

fn sse_text_with_usage(text: &str, prompt: u64, cached: u64) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n\
         data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{prompt},\"completion_tokens\":5,\"prompt_tokens_details\":{{\"cached_tokens\":{cached}}}}}}}\n\n\
         data: [DONE]\n\n"
    )
}

fn sse_tool_call(name: &str, args: &str) -> String {
    let escaped = args.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"call-e2e\",\"function\":{{\"name\":\"{name}\",\"arguments\":\"\"}}}}]}}}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"function\":{{\"arguments\":\"{escaped}\"}}}}]}}}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\ndata: [DONE]\n\n"
    )
}

struct Mock {
    addr: SocketAddr,
    requests: std::sync::Arc<Mutex<Vec<String>>>,
}

impl Mock {
    fn url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn request_count(&self) -> usize {
        self.requests.lock().expect("lock").len()
    }
}

async fn start_mock(replies: Vec<Reply>) -> Mock {
    use tokio::net::TcpListener;

    let replies = std::sync::Arc::new(Mutex::new(VecDeque::from(replies)));
    let requests = std::sync::Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let recorded = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let replies = replies.clone();
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                    let replies = replies.clone();
                    let recorded = recorded.clone();
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
    Mock { addr, requests }
}

struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    data: PathBuf,
}

fn sandbox(name: &str) -> Sandbox {
    let root = std::env::temp_dir().join(format!("lca-cli-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let data = root.join("data");
    std::fs::create_dir_all(&home).expect("mkdir");
    std::fs::create_dir_all(&data).expect("mkdir");
    std::fs::create_dir_all(root.join("project")).expect("mkdir");
    Sandbox { root, home, data }
}

impl Sandbox {
    /// Where the binary under test keeps its state on this platform -
    /// `default_data_dir()` honored the spawn environment (XDG_DATA_HOME
    /// on Linux, HOME on macOS where Library is the documented home by
    /// convention, APPDATA on Windows), and every grants/extension path
    /// here must land in the same place or macOS silently reads an empty
    /// grant store (docs/platform-notes.md: macOS state lives under
    /// ~/Library/Application Support/lca).
    fn state_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home.join("Library/Application Support/lca")
        } else {
            self.data.join("lca")
        }
    }
}

/// Consent the loopback mock host would get from the login modal in
/// production (FR-PERM-16); here the test stands in for the user's
/// approval, recorded in the grant store exactly as the real flow
/// writes it.
impl Sandbox {
    fn approve_loopback_net(&self, extra: serde_json::Value) {
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
    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    /// Seed a provider's credential namespace, the way `/login` would for a
    /// sandboxed (WASM) extension: it cannot see the host environment, so
    /// its endpoint and key live in its own store.
    fn write_credentials(&self, namespace: &str, values: serde_json::Value) {
        let dir = self.state_dir().join("credentials");
        std::fs::create_dir_all(&dir).expect("mkdir credentials");
        std::fs::write(
            dir.join(format!("{namespace}.json")),
            serde_json::to_vec_pretty(&values).expect("credentials serialize"),
        )
        .expect("write credentials");
    }

    fn run(&self, mock: Option<&Mock>, args: &[&str]) -> Output {
        self.run_env(mock, args, &[])
    }

    fn run_env(&self, mock: Option<&Mock>, args: &[&str], extra: &[(&str, &str)]) -> Output {
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
                .env("OPENAI_API_KEY", "test-key");
        }
        for (key, value) in extra {
            command.env(key, value);
        }
        command.output().expect("spawn lca")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json_lines(output: &Output) -> Vec<serde_json::Value> {
    stdout(output)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|err| panic!("bad json {line:?}: {err}"))
        })
        .collect()
}

fn rt() -> tokio::runtime::Runtime {
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
#[test]
fn headless_prompt_writes_the_result_to_stdout() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text_with_usage(
        "Hello from mock",
        1200,
        1000,
    ))]));
    let box_ = sandbox("headless");
    let output = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("Hello from mock"),
        "stdout: {}",
        stdout(&output)
    );
    assert_eq!(mock.request_count(), 1, "exactly one completion request");
}

// The --json envelope contract (docs/headless.md): one object per line,
// each with a type; the last line is turn-end.
#[test]
fn json_mode_emits_one_typed_object_per_line() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text_with_usage(
        "done", 1200, 1000,
    ))]));
    let box_ = sandbox("json");
    let output = box_.run(Some(&mock), &["-p", "hi", "--json"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));

    let lines = json_lines(&output);
    assert!(!lines.is_empty());
    let allowed = [
        "text",
        "tool-call",
        "tool-result",
        "usage",
        "extension-event",
        "error",
        "turn-end",
    ];
    for line in &lines {
        let kind = line["type"].as_str().expect("type field");
        assert!(
            allowed.contains(&kind),
            "unknown envelope type {kind}: {line}"
        );
    }
    let last = lines.last().expect("turn-end line");
    assert_eq!(last["type"], "turn-end");
    assert_eq!(last["status"], "ok");
    let usage = lines
        .iter()
        .find(|l| l["type"] == "usage")
        .expect("usage line");
    for field in [
        "input",
        "output",
        "cache_read",
        "cache_write",
        "cache_write_1h",
        "cost",
    ] {
        assert!(usage.get(field).is_some(), "{field} present: {usage}");
    }
    assert_eq!(usage["cache_read"], 1000);
}

// Verifies: FR-PROV-6 (with no provider extension enabled the agent reports
// that no model is available and offers the install command), FR-PROV-9
// (disabling the default provider leaves zero providers, an ordinary
// state, not a crash)
#[test]
fn disabled_provider_reports_the_install_command() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("never called"))]));
    let box_ = sandbox("no-provider");
    let output = box_.run_env(
        Some(&mock),
        &["-p", "hi"],
        &[("LCA_PROVIDER", "disabled-extension")],
    );
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("No model is available"), "{text}");
    assert!(
        text.contains("lca ext install"),
        "offers the install command: {text}"
    );
    assert_eq!(mock.request_count(), 0, "no request without a provider");
}

// Verifies: FR-PROV-9's disable path proper (FR-PERM-19's storage) -
// the default provider is registered but the grant store has it
// disabled for this project, so zero providers are enabled, the agent
// reports FR-PROV-6's message, and no socket opens.
#[test]
fn a_grant_store_disable_leaves_zero_enabled_providers() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("never called"))]));
    let box_ = sandbox("provider-off");
    box_.approve_loopback_net(serde_json::json!({
        "extensions": { "openai-compatible": false },
    }));
    let output = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("No model is available"), "{text}");
    assert!(text.contains("lca ext install"), "{text}");
    assert_eq!(
        mock.request_count(),
        0,
        "a disabled provider opens no socket"
    );
}

// Exit code table, docs/headless.md:2 = usage error.
#[test]
fn bad_flags_exit_two() {
    let box_ = sandbox("usage");
    let output = box_.run(None, &["--definitely-not-a-flag"]);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
}

// Exit3: provider error (after the retry limit for retryable classes).
#[test]
fn provider_error_exits_three() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Status(401)]));
    let box_ = sandbox("provider-error");
    let output = box_.run(Some(&mock), &["-p", "hi", "--json"]);
    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let error = lines
        .iter()
        .find(|l| l["type"] == "error")
        .expect("error envelope");
    assert_eq!(error["class"], "auth");
    assert_eq!(error["retryable"], false);
    let last = lines.last().expect("turn-end");
    assert_eq!(last["type"], "turn-end");
    assert_eq!(last["status"], "error");
}

// Exit4: an action needed approval and headless mode cannot prompt
// (FR-TOOL-3's headless path).
#[test]
fn unapprovable_action_exits_four() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("shell", r#"{"command":"rm -rf /"}"#)),
        Reply::Sse(sse_text("understood")),
    ]));
    let box_ = sandbox("denied");
    let output = box_.run(Some(&mock), &["-p", "clean up", "--json"]);
    assert_eq!(output.status.code(), Some(4), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let result = lines
        .iter()
        .find(|l| l["type"] == "tool-result")
        .expect("tool-result envelope");
    assert_eq!(result["status"], "denied");
    assert_eq!(
        mock.request_count(),
        2,
        "the model continued after the denial"
    );
}

// Exit5: the iteration limit aborted the turn (FR-CORE-9).
#[test]
fn iteration_limit_exits_five() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("write", r#"{"path":"a.txt","content":"A"}"#)),
        Reply::Sse(sse_tool_call("write", r#"{"path":"b.txt","content":"B"}"#)),
        Reply::Sse(sse_text("never")),
    ]));
    let box_ = sandbox("iteration");
    let output = box_.run_env(
        Some(&mock),
        &["-p", "loop", "--json"],
        &[("LCA_TOOL_MAX_ITERATIONS", "1")],
    );
    assert_eq!(output.status.code(), Some(5), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let last = lines.last().expect("turn-end");
    assert_eq!(last["stop_reason"], "iteration-limit");
    assert!(
        !box_.project().join("b.txt").exists(),
        "second round never ran"
    );
}

// Exit6: the named session is missing (docs/headless.md).
#[test]
fn missing_session_exits_six() {
    let box_ = sandbox("session-missing");
    let output = box_.run(None, &["export", "no-such-session"]);
    assert_eq!(output.status.code(), Some(6), "stderr: {}", stderr(&output));
}

// A run writes its session, `resume` lists it (FR-SESS-2), and rename shows
// up in that list.
#[test]
fn sessions_persist_and_resume_lists_them() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("stored"))]));
    let box_ = sandbox("resume");
    let output = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));

    let output = box_.run(None, &["resume"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let listing = stdout(&output);
    assert!(!listing.is_empty(), "resume lists sessions: {listing:?}");

    let id = listing
        .lines()
        .next()
        .expect("one session at least")
        .split_whitespace()
        .next()
        .expect("session id")
        .to_string();

    let output = box_.run(None, &["rename", &id, "renamed title"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let output = box_.run(None, &["resume"]);
    assert!(
        stdout(&output).contains("renamed title"),
        "listing: {}",
        stdout(&output)
    );
}

// Verifies: D5 — `lca session gc <id>` runs against a real session and
// reports a clean tree when nothing is orphaned.
#[test]
fn session_gc_reports_when_nothing_is_orphaned() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("stored"))]));
    let box_ = sandbox("session-gc");
    let output = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));

    let listing = stdout(&box_.run(None, &["resume"]));
    let id = listing
        .lines()
        .next()
        .expect("one session at least")
        .split_whitespace()
        .next()
        .expect("session id")
        .to_string();
    let output = box_.run(None, &["session", "gc", &id]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("no unreferenced attachments"),
        "stdout: {}",
        stdout(&output)
    );
}

// Verifies: ADR-0029 - `--attach` stages an image into the session store, the
// user record carries the content hash, and the model-visible stub rides in
// the message text (so a provider without vision still sees the image exists).
#[test]
fn attach_flag_stages_an_image_on_the_user_record() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("seen"))]));
    let box_ = sandbox("attach");
    let png = box_.project().join("shot.png");
    std::fs::write(
        &png,
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3],
    )
    .expect("write image");
    let output = box_.run(
        Some(&mock),
        &["-p", "look", "--attach", png.to_str().expect("utf-8 path")],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let log = find_session_log(&box_.state_dir()).expect("a session log");
    let text = std::fs::read_to_string(log).expect("read log");
    assert!(
        text.contains("[image attachment"),
        "the stub text is on the user record: {text}"
    );
    assert!(
        text.contains("\"attachments\":[\""),
        "the content hash is on the user record: {text}"
    );
}

// Verifies: FR-CFG-2 (the config command prints each resolved value and the
// source that set it)
#[test]
fn config_prints_values_with_sources() {
    let box_ = sandbox("config");
    let output = box_.run(None, &["config"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("provider"), "{text}");
    assert!(text.contains("default"), "sources are named: {text}");

    let output = box_.run_env(None, &["config"], &[("LCA_TOOL_TIMEOUT_SECONDS", "7")]);
    let text = stdout(&output);
    assert!(text.contains("tool.timeout_seconds = 7"), "{text}");
    assert!(text.contains("environment"), "source named: {text}");
}

// Release policy + ABI policy: --version prints the agent version, the ABI
// version, the crate version, and the build target.
#[test]
fn version_prints_all_four_facts() {
    let box_ = sandbox("version");
    let output = box_.run(None, &["--version"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(
        text.contains("abi 0.2"),
        "the window's live ABI line: {text}"
    );
    assert!(text.contains("target"), "{text}");
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
}

// ---------------------------------------------------------------------------
// Phase 5 exit: a clean machine installs from an OCI reference and a
// plain HTTPS archive, sees each consent screen, approves both, and
// runs a turn (FR-DIST-1/2/5/6/9, the capability catalog's consent
// surface, FR-EXT-9's denial count along the way).
//
// Fixtures: extensions/openai-compatible/fixtures/component.wasm and
// extensions/skills/fixtures/component.wasm, rebuilt with
// `cargo build -p <crate> --target wasm32-wasip2 --release` and copied
// into place (same convention as conformance's).
// ---------------------------------------------------------------------------

const OPENAI_COMPONENT: &[u8] =
    include_bytes!("../../../extensions/openai-compatible/fixtures/component.wasm");
const OPENAI_MANIFEST: &str = include_str!("../../../extensions/openai-compatible/extension.toml");
const SKILLS_COMPONENT: &[u8] =
    include_bytes!("../../../extensions/skills/fixtures/component.wasm");
const SKILLS_MANIFEST: &str = include_str!("../../../extensions/skills/extension.toml");

/// An anonymous OCI registry serving the two-layer convention (config
/// blob = extension.toml, layer0 = the component).
async fn mock_registry() -> SocketAddr {
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let config_digest = format!("sha256:{:x}", Sha256::digest(OPENAI_MANIFEST.as_bytes()));
    let layer_digest = format!("sha256:{:x}", Sha256::digest(OPENAI_COMPONENT));
    let image_manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": { "mediaType": "text/plain", "digest": config_digest, "size": OPENAI_MANIFEST.len() },
        "layers": [{ "mediaType": "application/wasm", "digest": layer_digest, "size": OPENAI_COMPONENT.len() }],
    })
    .to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 16384];
            let mut read = 0;
            loop {
                match stream.read(&mut buf[read..]).await {
                    Ok(0) => break,
                    Ok(n) => {
                        read += n;
                        if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let request = String::from_utf8_lossy(&buf[..read]).into_owned();
            let target = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (body, content_type) = if target.contains("/manifests/") {
                (
                    image_manifest.clone().into_bytes(),
                    "application/vnd.oci.image.manifest.v1+json",
                )
            } else if target.contains(&config_digest) {
                (OPENAI_MANIFEST.as_bytes().to_vec(), "text/plain")
            } else {
                (OPENAI_COMPONENT.to_vec(), "application/wasm")
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(&body).await;
        }
    });
    addr
}

/// Any HTTPS-style host serving the skills zip (packed here from the
/// committed component fixture).
async fn mock_archive(zip: Vec<u8>) -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 8192];
            let mut read = 0;
            loop {
                match stream.read(&mut buf[read..]).await {
                    Ok(0) => break,
                    Ok(n) => {
                        read += n;
                        if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/zip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                zip.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(&zip).await;
        }
    });
    addr
}

impl Sandbox {
    /// Run with piped stdin (the consent prompt reads it; EOF declines).
    fn run_with_stdin(&self, mock: Option<&Mock>, args: &[&str], stdin: &str) -> Output {
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
                .env("OPENAI_API_KEY", "test-key");
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
    fn extensions_root(&self) -> PathBuf {
        self.state_dir().join("extensions")
    }

    /// Write the grant store (ad hoc loopback net consent, the stand-in
    /// for FR-PERM-16's modal in this offline exit test). An empty file
    /// first, so no consent exists until the test says so.
    fn write_grants(&self, with_loopback_net: bool) {
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
#[test]
fn a_clean_machine_installs_from_oci_and_https_then_runs_a_turn() {
    let runtime = rt();
    let registry_addr = runtime.block_on(mock_registry());
    let zip = lca_registry::pack_archive(SKILLS_MANIFEST, SKILLS_COMPONENT).expect("pack zip");
    let archive_addr = runtime.block_on(mock_archive(zip));
    let model = runtime.block_on(start_mock(vec![Reply::Sse(sse_text(
        "installed and chatting",
    ))]));

    let sandbox = sandbox("clean-machine");
    // The grant store exists but consents to nothing yet: the first
    // turn must be denied by the capability engine (no ad hoc net),
    // which is what writes the journal `ext info` counts.
    sandbox.write_grants(false);

    // --- OCI install, consent shown and approved (FR-DIST-1).
    // `registry_addr` already renders host:port.
    let reference = format!("{registry_addr}/library/openai-compatible:abi-0.1");
    let output = sandbox.run_with_stdin(Some(&model), &["ext", "install", &reference], "y\n");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("Connect to api.openai.com"),
        "the net consent sentence, verbatim: {text}"
    );
    assert!(
        text.contains("Store and read its own saved credentials"),
        "the credentials consent sentence: {text}"
    );
    assert!(text.contains("Allow these capabilities?"), "{text}");
    assert!(text.contains("installed openai-compatible"), "{text}");

    // The lockfile records digest + source (FR-DIST-6); the component
    // sits beside its manifest, named by that digest.
    let lock = sandbox.extensions_root().join("lockfile.json");
    let root = sandbox.extensions_root();
    let lock_text = std::fs::read_to_string(&lock).unwrap_or_else(|err| {
        panic!(
            "lockfile at {}: {err} (root exists: {}, entries: {:?})",
            lock.display(),
            root.exists(),
            std::fs::read_dir(&root)
                .map(|entries| entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name())
                    .collect::<Vec<_>>())
                .unwrap_or_default()
        )
    });
    assert!(lock_text.contains(&reference), "{lock_text}");
    assert!(lock_text.contains("sha256:"), "{lock_text}");
    assert!(
        sandbox
            .extensions_root()
            .join("openai-compatible")
            .join("extension.toml")
            .exists()
    );
    let component_dir: Vec<_> =
        std::fs::read_dir(sandbox.extensions_root().join("openai-compatible"))
            .expect("dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".wasm"))
            .collect();
    assert_eq!(component_dir.len(), 1, "one component, digest-named");

    // --- HTTPS archive install with its own consent (FR-DIST-9).
    let url = format!("http://{archive_addr}/skills-abi-0.1.zip");
    let output = sandbox.run_with_stdin(Some(&model), &["ext", "install", &url], "yes\n");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("Files: workspace (read)"),
        "the fs consent sentence: {text}"
    );
    assert!(text.contains("installed skills"), "{text}");

    // --- ext list shows both sources (the exit test's "sees ... each").
    let output = sandbox.run(Some(&model), &["ext", "list"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("openai-compatible"), "{text}");
    assert!(text.contains("skills"), "{text}");
    assert!(text.contains(&reference), "{text}");
    assert!(text.contains(&url), "{text}");

    // --- ext info: digest, consent, denial count (FR-EXT-9). No
    // denial yet: nothing has tried anything.
    let output = sandbox.run(Some(&model), &["ext", "info", "openai-compatible"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("digest:   sha256:"), "{text}");
    assert!(text.contains("denials:  0"), "{text}");

    // The installed WASM provider cannot read the host environment
    // (sandboxing is the point), so its endpoint and key live in its own
    // credential namespace - exactly what `/login` writes for a real
    // install. Without this the WASM provider would default to
    // api.openai.com and the turn would fail with a connect error.
    sandbox.write_credentials(
        "openai-compatible",
        serde_json::json!({ "api_key": "test-key", "base_url": model.url() }),
    );

    // --- Turn one, WITHOUT ad hoc consent: the installed provider
    // reaches for127.0.0.1, the engine refuses and journals it.
    let output = sandbox.run(Some(&model), &["-p", "hi"]);
    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("matches no granted") || text.contains("permission denied"),
        "{text}"
    );

    let output = sandbox.run(Some(&model), &["ext", "info", "openai-compatible"]);
    let text = stdout(&output);
    assert!(text.contains("denials:  1"), "the journal counted: {text}");

    // --- Grant the loopback consent (FR-PERM-16's modal, standing in
    // offline) and run the turn: the INSTALLED provider, loaded from
    // its digest record, talks to the mocked model (FR-DIST-8).
    sandbox.write_grants(true);
    let output = sandbox.run(Some(&model), &["-p", "hi", "--json"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let text_line = lines
        .iter()
        .find(|l| l["type"] == "text")
        .expect("a text envelope");
    assert_eq!(text_line["content"], "installed and chatting");
    assert_eq!(model.request_count(), 1, "exactly one model call");

    // --- ext update against the same source: already current.
    let output = sandbox.run(Some(&model), &["ext", "update", "openai-compatible"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("up to date"), "{text}");

    // --- ext remove forgets the tree and the record.
    let output = sandbox.run(Some(&model), &["ext", "remove", "skills"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(!sandbox.extensions_root().join("skills").exists());
    let output = sandbox.run(Some(&model), &["ext", "list"]);
    let text = stdout(&output);
    assert!(!text.contains("\nskills "), "gone from the list: {text}");

    // Consent refused writes nothing at all: EOF at the prompt declines.
    let archive2 = lca_registry::pack_archive(SKILLS_MANIFEST, SKILLS_COMPONENT).expect("pack");
    let runtime2 = rt();
    let archive_addr2 = runtime2.block_on(mock_archive(archive2));
    let url2 = format!("http://{archive_addr2}/skills-abi-0.1.zip");
    let output = sandbox.run_with_stdin(Some(&model), &["ext", "install", &url2], "");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("aborted"), "{}", stdout(&output));
    assert!(
        !sandbox.extensions_root().join("skills").exists(),
        "nothing written"
    );
}

// ---------------------------------------------------------------------------
// Real-terminal tests (docs/testing-plan.md section 14): the TUI in a tmux
// pane, asserting what is on screen. Unix only; a machine without tmux
// skips rather than fails.
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// One tmux session on its own socket name; dropping it kills only that
/// session, never the server or anyone else's panes.
#[cfg(unix)]
struct Tmux {
    name: String,
}

#[cfg(unix)]
impl Tmux {
    fn new(tag: &str) -> Tmux {
        let name = format!("lca-smoke-{}-{tag}", std::process::id());
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &name])
            .status();
        Tmux { name }
    }

    fn tmux(args: &[&str]) -> Output {
        Command::new("tmux").args(args).output().expect("run tmux")
    }

    fn capture(&self) -> String {
        let out = Self::tmux(&["capture-pane", "-t", &self.name, "-p"]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn spawn(
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
        let extra: String = extra_env
            .iter()
            .map(|(key, value)| format!(" {key}={value}"))
            .collect();
        let arguments: String = args.iter().map(|arg| format!(" {arg}")).collect();
        let command = format!(
            "cd {project} && HOME={home} USERPROFILE={home} XDG_DATA_HOME={data} \
             APPDATA={data} LOCALAPPDATA={data} XDG_CONFIG_HOME={config} \
             LCA_UPDATE_CHECK=false{endpoint}{key}{extra} {bin}{arguments}",
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

    fn send(&self, keys: &[&str]) {
        let mut args = vec!["send-keys", "-t", &self.name];
        args.extend_from_slice(keys);
        let out = Self::tmux(&args);
        assert!(
            out.status.success(),
            "tmux send-keys: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn wait_for(&self, needle: &str, timeout: std::time::Duration) -> String {
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

    fn resize(&self, cols: u32, rows: u32) -> String {
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
            .args(["kill-session", "-t", &self.name])
            .status();
    }
}

#[cfg(any(unix, windows))]
fn find_session_log(state: &std::path::Path) -> Option<std::path::PathBuf> {
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
fn find_session_end(state: &std::path::Path) -> Option<String> {
    let path = find_session_log(state)?;
    let text = std::fs::read_to_string(&path).ok()?;
    text.contains("\"t\":\"session-end\"").then_some(text)
}

#[cfg(unix)]
fn wait_for_session_end(state: &std::path::Path, timeout: std::time::Duration) -> String {
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
#[cfg(unix)]
#[test]
fn the_tui_renders_a_turn_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("shell", r#"{"command":"echo smoke-ok"}"#)),
        Reply::Sse(sse_text_with_usage("turn complete", 20, 0)),
    ]));
    let sandbox = sandbox("tui-smoke");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("turn");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);

    // 1. Startup renders the frame with the configured model.
    session.wait_for(
        "openai-compatible/gpt-4o-mini",
        std::time::Duration::from_secs(20),
    );

    // 2. A prompt asks the permission question for the shell call.
    session.send(&["do a thing", "Enter"]);
    session.wait_for("Allow this action?", std::time::Duration::from_secs(25));

    // 3. Answering it lets the turn complete and render the reply.
    session.send(&["o"]);
    session.wait_for("turn complete", std::time::Duration::from_secs(25));

    // 4. A resize re-renders without losing the frame.
    let resized = session.resize(100, 30);
    assert!(
        resized.contains("openai-compatible") || resized.contains("turn complete"),
        "the frame survives a resize:\n{resized}"
    );

    // 5. A clean quit writes the session-end marker.
    session.send(&["/exit", "Enter"]);
    let log = wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
    assert!(log.contains("\"t\":\"session-end\""), "session-end written");
}

// Verifies: the real-terminal checklist's secret-prompt case - what the user
// types into `/login` never reaches the visible frame.
#[cfg(unix)]
#[test]
fn the_logins_secret_prompt_masks_input_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("tui-login-mask");
    let session = Tmux::new("mask");
    // No key: `/login` asks for the secret.
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));

    session.send(&["/login", "Enter"]);
    session.wait_for("input hidden", std::time::Duration::from_secs(15));
    let secret = "sk-super-secret-value";
    session.send(&[secret]);
    // Give the frame a beat to render; the secret must not be visible.
    std::thread::sleep(std::time::Duration::from_millis(600));
    let pane = session.capture();
    assert!(
        !pane.contains(secret),
        "the secret never appears in the frame:\n{pane}"
    );
    assert!(pane.contains("input hidden"), "still the masked prompt");
}

// Verifies: the SRDD's restart exit test, FR-SESS-4/FR-SESS-5, and
// FR-CACHE-5/FR-CACHE-6 across a process boundary: a session created in one
// process is resumed in another, the resumed run crosses the compaction
// threshold, and the compaction record's replaced range ends before the
// resumed turn (the in-process analogue is
// `after_compaction_a_new_turn_stays_outside_the_stable_prefix`).
#[cfg(unix)]
#[test]
fn a_resumed_session_compacts_at_the_turn_boundary() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text_with_usage("first reply", 50, 0)),
        Reply::Sse(sse_text_with_usage("compaction summary", 5, 0)),
        Reply::Sse(sse_text_with_usage("second reply", 30, 0)),
    ]));
    let sandbox = sandbox("resume-compact");
    // Documented knobs only: `compaction.threshold` is a configuration key
    // (`LCA_COMPACTION_THRESHOLD`), and the provider's window is
    // `OPENAI_CONTEXT_WINDOW` (docs/providers/openai-compatible.md). A
    // 1000-token window at 1% compacts once a turn reports 10 prompt tokens.
    let budget = [
        ("OPENAI_CONTEXT_WINDOW", "1000"),
        ("LCA_COMPACTION_THRESHOLD", "0.01"),
    ];

    // Run 1: create the session in one process, then exit.
    let output = sandbox.run_env(Some(&mock), &["-p", "first turn"], &budget);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("first reply"),
        "{}",
        stdout(&output)
    );

    // The session outlives the process.
    let log = find_session_log(&sandbox.state_dir()).expect("a session log");
    let id = log
        .parent()
        .expect("session directory")
        .file_name()
        .expect("session id")
        .to_string_lossy()
        .to_string();

    // Run 2: resume it in a real terminal; the transcript loads from the log
    // before compaction runs.
    let session = Tmux::new("resume");
    session.spawn(&sandbox, Some(&mock), true, &budget, &["resume", &id]);
    session.wait_for("first reply", std::time::Duration::from_secs(25));
    session.send(&["second turn", "Enter"]);
    session.wait_for("second reply", std::time::Duration::from_secs(30));
    session.send(&["/exit", "Enter"]);

    // The compaction record lands at the boundary: its replaced range starts
    // at the first turn's user record and ends before the resumed turn, so
    // the resumed turn stays outside the cached prefix.
    let records: Vec<serde_json::Value> = std::fs::read_to_string(&log)
        .expect("read the session log")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("record json"))
        .collect();
    let compaction = records
        .iter()
        .position(|record| record["t"] == "compaction")
        .expect("a compaction record (FR-SESS-4)");
    let resumed_user = records
        .iter()
        .rposition(|record| record["t"] == "user")
        .expect("the resumed turn's user record");
    let first_user = records
        .iter()
        .find(|record| record["t"] == "user")
        .expect("the first turn's user record");
    assert_eq!(
        records[compaction]["replaced_from"], first_user["id"],
        "the replaced range starts at the first turn"
    );
    assert_ne!(
        records[compaction]["replaced_to"], records[resumed_user]["id"],
        "the resumed turn is not inside the replaced range"
    );
    let range_end = records
        .iter()
        .position(|record| record["id"] == records[compaction]["replaced_to"])
        .expect("the replaced range's end is a real record");
    assert!(
        range_end < resumed_user,
        "the replaced range ends before the resumed turn: {records:#?}"
    );
    assert_eq!(records[compaction]["summary"], "compaction summary");
}

// ---------------------------------------------------------------------------
// Real-terminal tests on Windows (docs/testing-plan.md section 14): the TUI
// under a pseudo-console (ConPTY). The checklist is the same as the tmux
// suite; the driver is not.
// ---------------------------------------------------------------------------

/// Read the pseudo-console until `needle` appears in the accumulated screen,
/// or the deadline passes. ConPTY's read is a non-blocking peek, so this
/// polls.
#[cfg(windows)]
fn read_until(
    pty: &mut lca_tools::PtyChild,
    screen: &mut String,
    needle: &str,
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if screen.contains(needle) {
            return true;
        }
        if std::time::Instant::now() > deadline {
            return false;
        }
        match pty.read(65536) {
            Ok(Some(chunk)) if !chunk.is_empty() => {
                screen.push_str(&String::from_utf8_lossy(&chunk));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            _ => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// The sandbox environment the TUI process needs, as `PtyChild::envs`.
#[cfg(windows)]
fn console_env(sandbox: &Sandbox, mock: &Mock, with_key: bool) -> Vec<(&'static str, String)> {
    let home = sandbox.home.to_string_lossy().into_owned();
    let data = sandbox.data.to_string_lossy().into_owned();
    let config = sandbox.home.join(".config").to_string_lossy().into_owned();
    let mut envs = vec![
        ("HOME", home.clone()),
        ("USERPROFILE", home),
        ("XDG_DATA_HOME", data.clone()),
        ("APPDATA", data.clone()),
        ("LOCALAPPDATA", data.clone()),
        ("XDG_CONFIG_HOME", config),
        ("LCA_UPDATE_CHECK", "false".to_string()),
        ("OPENAI_BASE_URL", mock.url()),
    ];
    if with_key {
        envs.push(("OPENAI_API_KEY", "test-key".to_string()));
    }
    envs
}

#[cfg(windows)]
fn spawn_console(sandbox: &Sandbox, envs: &[(&'static str, String)]) -> lca_tools::PtyChild {
    let borrowed: Vec<(&str, &str)> = envs
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    lca_tools::PtyChild::spawn(
        env!("CARGO_BIN_EXE_lca"),
        &[],
        &sandbox.project(),
        40,
        140,
        &borrowed,
    )
    .expect("spawn the TUI on a ConPTY")
}

// Verifies: the real-terminal checklist (docs/testing-plan.md section 14) on
// the Windows harness: startup renders, a scripted turn streams and renders,
// and a clean quit writes `session-end`.
#[cfg(windows)]
#[ignore = "ConPTY produced no bytes on the hosted runner (docs/platform-notes.md, Windows quarantine ledger)"]
#[test]
fn the_tui_renders_a_turn_in_a_windows_console() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text_with_usage(
        "console smoke ok",
        20,
        0,
    ))]));
    let sandbox = sandbox("tui-conpty");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let envs = console_env(&sandbox, &mock, true);
    let mut pty = spawn_console(&sandbox, &envs);
    let mut screen = String::new();

    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "openai-compatible",
            std::time::Duration::from_secs(60)
        ),
        "startup renders the frame: {screen:?}"
    );

    pty.write(b"hello console\r").expect("write the prompt");
    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "console smoke ok",
            std::time::Duration::from_secs(60)
        ),
        "the reply renders: {screen:?}"
    );

    pty.write(b"/exit\r").expect("write /exit");
    let log = find_session_log(&sandbox.state_dir()).expect("a session log");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains("\"t\":\"session-end\"") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a clean quit writes session-end"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// Verifies: the real-terminal checklist's secret-prompt case on Windows -
// what the user types into `/login` never reaches the visible screen.
#[cfg(windows)]
#[ignore = "ConPTY produced no bytes on the hosted runner (docs/platform-notes.md, Windows quarantine ledger)"]
#[test]
fn the_logins_secret_prompt_masks_input_in_a_windows_console() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("tui-conpty-mask");
    sandbox.approve_loopback_net(serde_json::json!({}));

    // No key: `/login` asks for the secret.
    let envs = console_env(&sandbox, &mock, false);
    let mut pty = spawn_console(&sandbox, &envs);
    let mut screen = String::new();

    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "no model",
            std::time::Duration::from_secs(60)
        ),
        "the no-model state renders: {screen:?}"
    );
    pty.write(b"/login\r").expect("write /login");
    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "input hidden",
            std::time::Duration::from_secs(30)
        ),
        "the masked prompt renders: {screen:?}"
    );

    let secret = "sk-super-secret-value";
    pty.write(secret.as_bytes()).expect("write the secret");
    std::thread::sleep(std::time::Duration::from_millis(800));
    let mut settle = String::new();
    let _ = read_until(
        &mut pty,
        &mut settle,
        "\u{0}",
        std::time::Duration::from_millis(800),
    );
    screen.push_str(&settle);
    assert!(
        !screen.contains(secret),
        "the secret never appears on the ConPTY screen: {screen:?}"
    );
}
