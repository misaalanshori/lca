//! GitHub #56, live: `--mode rpc` drives one session across commands.
//! Replays pi's `rpc-commands.md` happy path (prompt → steer →
//! follow_up → cancel → shutdown) and asserts semantic outcomes plus
//! multi-turn session persistence.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::{Reply, json_lines, rt, sandbox, sse_text, sse_tool_call, start_mock, stderr};

fn cmd(id: &str, command: &str, extra: &str) -> String {
    if extra.is_empty() {
        format!("{{\"id\":\"{id}\",\"type\":\"{command}\"}}\n")
    } else {
        format!("{{\"id\":\"{id}\",\"type\":\"{command}\",{extra}}}\n")
    }
}

// Verifies: gh #56 (the RPC happy path): prompt runs, steer and
// follow_up queue mid-run, cancel ends the run, the follow-up still
// runs, shutdown persists and exits clean - all correlated by id, all
// turns in one session log. One long sleep makes mid-run delivery
// deterministic; the pipe preserves command order.
#[test]
fn rpc_replays_prompt_steer_follow_up_cancel_shutdown() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("shell", r#"{"command":"sleep 5"}"#)),
        Reply::Sse(sse_text("after the tool")),
        Reply::Sse(sse_text("followed up")),
        Reply::Sse(sse_text("spare")),
    ]));
    let box_ = sandbox("rpc-happy-path");
    // The whole script up front: the pipe buffers it, and the loop
    // reads each line while the sleep runs, so steer/follow-up/cancel
    // land mid-turn exactly like an interactive client.
    let script = [
        cmd("p1", "prompt", r#""message":"sleep please""#),
        cmd("s1", "steer", r#""message":"a note""#),
        cmd("f1", "follow_up", r#""message":"later""#),
        cmd("c1", "cancel", ""),
        cmd("d1", "shutdown", ""),
    ]
    .concat();
    let output = box_.run_with_stdin(
        Some(&mock),
        &["--yolo", "--model", "zen-free", "--mode", "rpc"],
        &script,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "shutdown exits clean: {}",
        stderr(&output)
    );
    let records = json_lines(&output);
    let responses: Vec<&serde_json::Value> = records
        .iter()
        .filter(|line| line["type"] == "response")
        .collect();
    let response = |id: &str| {
        responses
            .iter()
            .find(|line| line["id"] == id)
            .unwrap_or_else(|| panic!("a response for {id}: {responses:?}"))
    };
    assert_eq!(response("p1")["data"]["disposition"], "started");
    assert_eq!(response("s1")["data"]["disposition"], "queued");
    assert_eq!(response("f1")["data"]["disposition"], "queued");
    assert_eq!(response("c1")["success"], true);
    assert_eq!(response("d1")["success"], true);

    // The sleep turn ends cancelled, and the follow-up runs after it.
    let turn_ends: Vec<&serde_json::Value> = records
        .iter()
        .filter(|line| line["type"] == "turn-end")
        .collect();
    assert!(
        turn_ends.len() >= 2,
        "cancelled sleep plus follow-up each end: {turn_ends:?}"
    );
    assert!(
        turn_ends
            .iter()
            .any(|line| line["stop_reason"] == "cancelled"),
        "cancel ends the run: {turn_ends:?}"
    );
    assert!(
        records
            .iter()
            .any(|line| line["type"] == "text" && line["content"] == "after the tool"),
        "the follow-up turn ran a reply"
    );

    // One session holds every turn, prompts and queued messages alike.
    let sessions = std::fs::read_dir(box_.state_dir().join("sessions")).expect("sessions");
    let mut logs = Vec::new();
    let mut stack: Vec<std::path::PathBuf> =
        sessions.map(|entry| entry.expect("entry").path()).collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|name| name.to_str()) == Some("log.jsonl") {
                logs.push(path);
            }
        }
    }
    assert_eq!(logs.len(), 1, "one session across commands");
    let text = std::fs::read_to_string(&logs[0]).expect("read log");
    for expected in ["sleep please", "later"] {
        assert!(
            text.contains(expected),
            "the log holds {expected:?}: {text}"
        );
    }
}

// Verifies: gh #56 (malformed input and unknown commands fail loud,
// never silent, and never kill the loop).
#[test]
fn rpc_rejects_parse_errors_and_unknown_commands() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("hi"))]));
    let box_ = sandbox("rpc-errors");
    let script = [
        "not json\n".to_string(),
        cmd("u1", "frobnicate", ""),
        cmd("p1", "prompt", r#""message":"hi""#),
        cmd("d1", "shutdown", ""),
    ]
    .concat();
    let output = box_.run_with_stdin(
        Some(&mock),
        &["--yolo", "--model", "zen-free", "--mode", "rpc"],
        &script,
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let records = json_lines(&output);
    let parse = records
        .iter()
        .find(|line| line["command"] == "parse")
        .expect("a parse response");
    assert_eq!(parse["success"], false);
    assert!(!parse.get("id").is_some(), "parse errors carry no id");
    let unknown = records
        .iter()
        .find(|line| line["id"] == "u1")
        .expect("an unknown-command response");
    assert_eq!(unknown["success"], false);
    assert!(
        records.iter().any(|line| line["type"] == "turn-end"),
        "the loop survives both and still runs"
    );
}
