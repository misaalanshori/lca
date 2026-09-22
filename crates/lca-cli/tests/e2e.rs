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
    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn run(&self, mock: Option<&Mock>, args: &[&str]) -> Output {
        self.run_env(mock, args, &[])
    }

    fn run_env(&self, mock: Option<&Mock>, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lca"));
        command
            .args(args)
            .current_dir(self.project())
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_DATA_HOME", &self.data)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
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
    assert!(text.contains("abi"), "{text}");
    assert!(text.contains("0.1"), "{text}");
    assert!(text.contains("target"), "{text}");
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
}
