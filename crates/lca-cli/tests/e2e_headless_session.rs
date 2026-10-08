//! GitHub #111, live: headless honors `--model`, `-c`, `-r` against the
//! loopback mock (no real network, no real credentials).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::sse_tool_call;
use common::{Reply, rt, sandbox, sse_text, start_mock};
use std::collections::HashMap;

/// Every `log.jsonl` under the sandbox state dir, with its parsed lines.
fn session_logs(box_: &common::Sandbox) -> HashMap<String, Vec<serde_json::Value>> {
    let mut out = HashMap::new();
    let sessions = box_.state_dir().join("sessions");
    let mut stack = vec![sessions];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some("log.jsonl") {
                let text = std::fs::read_to_string(&path).expect("read log");
                let lines = text
                    .lines()
                    .map(|line| serde_json::from_str(line).expect("log line parses"))
                    .collect();
                out.insert(path.to_string_lossy().into_owned(), lines);
            }
        }
    }
    out
}

fn user_texts(lines: &[serde_json::Value]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| l["t"] == "user")
        .map(|l| l["content"].as_str().unwrap_or("").to_string())
        .collect()
}

// Verifies: #111 (`lca -c -p x` appends to the last session's `log.jsonl`).
#[test]
fn gh111_continue_appends_to_the_last_session_log() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text("first answer")),
        Reply::Sse(sse_text("second answer")),
        Reply::Sse(sse_text("spare")),
    ]));
    let box_ = sandbox("headless-continue");
    let first = box_.run(
        Some(&mock),
        &["--model", "zen-free", "-p", "first", "--json"],
    );
    assert_eq!(
        first.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(session_logs(&box_).len(), 1, "one session after run one");

    let second = box_.run(Some(&mock), &["-c", "-p", "second", "--json"]);
    assert_eq!(
        second.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let logs = session_logs(&box_);
    assert_eq!(
        logs.len(),
        1,
        "continue reuses the session, it does not fork one"
    );
    let lines = logs.values().next().expect("the session log");
    assert_eq!(
        user_texts(lines),
        vec!["first".to_string(), "second".to_string()],
        "both turns append to the same log.jsonl"
    );
}

// Verifies: #111 (`lca --model m -p x` records `m`).
#[test]
fn gh111_model_override_is_recorded() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("answer"))]));
    let box_ = sandbox("headless-model");
    let output = box_.run(Some(&mock), &["--model", "zen-free", "-p", "hi", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let logs = session_logs(&box_);
    assert_eq!(logs.len(), 1);
    let (path, lines) = logs.into_iter().next().expect("the session log");
    let assistants: Vec<_> = lines.iter().filter(|l| l["t"] == "assistant").collect();
    assert!(!assistants.is_empty(), "a turn ran");
    for record in &assistants {
        assert_eq!(
            record["model"], "zen-free",
            "the requested model lands on the record"
        );
    }
    let meta_path = std::path::Path::new(&path)
        .parent()
        .expect("session dir")
        .join("meta.json");
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&meta_path).expect("read meta"))
            .expect("meta parses");
    assert_eq!(
        meta["model"], "zen-free",
        "meta.json names the model last used"
    );
}

// Verifies: #111 (continuing with no session is a loud session error,
// not a silent fresh start a script would mistake for a continuation).
#[test]
fn gh111_continue_with_no_session_exits_six() {
    let box_ = sandbox("headless-continue-empty");
    let output = box_.run(None, &["-c", "-p", "x"]);
    assert_eq!(
        output.status.code(),
        Some(6),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// Verifies: #111 (`-c` with `-r` is a usage error).
#[test]
fn gh111_continue_with_resume_is_a_usage_error() {
    let box_ = sandbox("headless-contradiction");
    let output = box_.run(None, &["-c", "-r", "abc", "-p", "x"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// Verifies: #111 (`-r <id> -p x` resumes that session headlessly).
#[test]
fn gh111_resume_reruns_in_the_named_session() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text("first answer")),
        Reply::Sse(sse_text("second answer")),
    ]));
    let box_ = sandbox("headless-resume");
    let first = box_.run(
        Some(&mock),
        &["--model", "zen-free", "-p", "first", "--json"],
    );
    assert_eq!(
        first.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let logs = session_logs(&box_);
    assert_eq!(logs.len(), 1);
    let id = std::path::Path::new(logs.keys().next().expect("log path"))
        .parent()
        .expect("session dir")
        .file_name()
        .expect("session id")
        .to_string_lossy()
        .into_owned();

    let second = box_.run(Some(&mock), &["-r", &id, "-p", "second", "--json"]);
    assert_eq!(
        second.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let logs = session_logs(&box_);
    assert_eq!(logs.len(), 1, "resume reuses the named session");
    let lines = logs.values().next().expect("the session log");
    assert_eq!(user_texts(lines).len(), 2, "the second turn appended");
}

// Verifies: gh #40 (structured shell results ride the `--json` envelope
// and the session record): a failing shell call surfaces `exit_code`
// and `truncated` on the wire and on the log line.
#[cfg(unix)]
#[test]
fn gh40_shell_results_carry_exit_code_on_the_wire_and_in_the_log() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("shell", r#"{"command":"exit 3"}"#)),
        Reply::Sse(sse_text("done")),
    ]));
    let box_ = sandbox("headless-structured");
    let output = box_.run(Some(&mock), &["--yolo", "-p", "run it", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelopes: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let tool_result = envelopes
        .iter()
        .find(|l| l["type"] == "tool-result")
        .expect("a tool-result envelope");
    assert_eq!(
        tool_result["exit_code"], 3,
        "the envelope names it: {tool_result}"
    );
    assert_eq!(tool_result["truncated"], false);
    assert!(
        tool_result.get("full_output_path").is_none() || tool_result["full_output_path"].is_null(),
        "no spill means no path: {tool_result}"
    );

    let logs = session_logs(&box_);
    let lines = logs.values().next().expect("the session log");
    let record = lines
        .iter()
        .find(|l| l["t"] == "tool-result")
        .expect("a tool-result record");
    assert_eq!(record["exit_code"], 3, "the record names it: {record}");
}

// Verifies: gh #67 - `lca -t read -p x` offers the model exactly
// `read`; without the flag the built-ins ride along.
#[test]
fn tools_allowlist_reaches_the_model_request() {
    fn tool_names(mock: &common::Mock) -> Vec<Vec<String>> {
        mock.bodies()
            .iter()
            .filter_map(|body| serde_json::from_str::<serde_json::Value>(body).ok())
            .filter(|body| body.get("messages").is_some())
            .map(|body| {
                body["tools"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
                    .collect()
            })
            .collect()
    }

    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text("scoped")),
        Reply::Sse(sse_text("full")),
    ]));
    let box_ = sandbox("headless-tools");
    let scoped = box_.run(
        Some(&mock),
        &["--model", "zen-free", "-t", "read", "-p", "hi"],
    );
    assert_eq!(
        scoped.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&scoped.stderr)
    );
    let full = box_.run(Some(&mock), &["--model", "zen-free", "-p", "hi"]);
    assert_eq!(
        full.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&full.stderr)
    );
    let requests = tool_names(&mock);
    assert_eq!(requests.len(), 2, "one turn each: {requests:?}");
    assert_eq!(requests[0], vec!["read".to_string()], "allowlisted");
    assert!(
        requests[1].contains(&"write".to_string()),
        "unflagged runs keep the built-ins: {:?}",
        requests[1]
    );
}

// Verifies: gh #69 - `--no-session -p x` runs without persisting any
// session files.
#[test]
fn no_session_leaves_no_session_files() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("volatile"))]));
    let box_ = sandbox("headless-no-session");
    let run = box_.run(
        Some(&mock),
        &["--model", "zen-free", "--no-session", "-p", "hi"],
    );
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let sessions = box_.state_dir().join("sessions");
    let leftovers: Vec<_> = std::fs::read_dir(&sessions)
        .map(|entries| entries.flatten().collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "no session files: {leftovers:?}");
}

// Verifies: gh #69 - `--session-id` creates the id when absent and
// reopens it on the next run; `--name` titles it.
#[test]
fn session_id_creates_reopens_and_names() {
    fn meta_titles(box_: &common::Sandbox) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut stack = vec![box_.state_dir().join("sessions")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().and_then(|n| n.to_str()) == Some("meta.json") {
                    let text = std::fs::read_to_string(&path).expect("read meta");
                    let meta: serde_json::Value = serde_json::from_str(&text).expect("meta parses");
                    let id = path
                        .parent()
                        .and_then(|parent| parent.file_name())
                        .and_then(|name| name.to_str())
                        .unwrap_or("")
                        .to_string();
                    out.push((id, meta["title"].as_str().unwrap_or("").to_string()));
                }
            }
        }
        out.sort();
        out
    }

    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text("one")),
        Reply::Sse(sse_text("two")),
    ]));
    let box_ = sandbox("headless-session-id");
    let first = box_.run(
        Some(&mock),
        &[
            "--model",
            "zen-free",
            "--session-id",
            "demo-1",
            "--name",
            "Demo",
            "-p",
            "hi",
        ],
    );
    assert_eq!(
        first.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        meta_titles(&box_),
        vec![("demo-1".to_string(), "Demo".to_string())],
        "created under the id, titled"
    );
    let second = box_.run(
        Some(&mock),
        &[
            "--model",
            "zen-free",
            "--session-id",
            "demo-1",
            "-p",
            "again",
        ],
    );
    assert_eq!(
        second.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        session_logs(&box_).len(),
        1,
        "the second run reopens the same session"
    );
}

// Verifies: gh #69 - `--fork` clones the parent at its tip and runs
// the fork.
#[test]
fn fork_runs_the_tip_clone() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text("parent")),
        Reply::Sse(sse_text("child")),
    ]));
    let box_ = sandbox("headless-fork");
    let parent = box_.run(Some(&mock), &["--model", "zen-free", "-p", "start"]);
    assert_eq!(
        parent.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&parent.stderr)
    );
    let logs = session_logs(&box_);
    assert_eq!(logs.len(), 1, "one parent session");
    let parent_log = logs.keys().next().expect("a log");
    let parent_id = std::path::Path::new(parent_log)
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .expect("the id directory")
        .to_string();
    let fork = box_.run(
        Some(&mock),
        &["--model", "zen-free", "--fork", &parent_id, "-p", "forked"],
    );
    assert_eq!(
        fork.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&fork.stderr)
    );
    assert_eq!(session_logs(&box_).len(), 2, "parent plus fork");
}

// Verifies: gh #71 - the canonical pipe idiom end to end: piped stdin
// prepends the first prompt the faux provider sees.
#[test]
fn piped_stdin_prepends_the_first_prompt() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("reviewed"))]));
    let box_ = sandbox("headless-pipe");
    let run = box_.run_with_stdin(
        Some(&mock),
        &[
            "--model",
            "zen-free",
            "--allow-host",
            "127.0.0.1",
            "-p",
            "review",
        ],
        "DIFF-BODY",
    );
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let logs = session_logs(&box_);
    assert_eq!(logs.len(), 1);
    assert_eq!(
        user_texts(logs.values().next().expect("the session log")),
        vec!["DIFF-BODYreview".to_string()],
        "stdin prepends in pi order"
    );
}
