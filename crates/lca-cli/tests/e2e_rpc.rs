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
// Quarantined under testing-plan section 13 (2026-10-07, gh #195):
// under full-suite parallel load the child exits 0 with responses
// but zero turn-ends. Solo/group/reruns green; one Windows CI hit.
// Remove the ignore to un-quarantine (exit: 5x stressed green).
#[test]
#[ignore = "turn-ends dropped under load - tracked in gh #195"]
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

// Verifies: gh #426 - a message accepted mid-turn is announced on the
// event stream, and the accept precedes the drain. This is the end-to-end
// half: the unit rows pin the sink's line, this one pins that RPC actually
// drives it, in order, on the same stream a client reads.
//
// No sleep and no cancel, so it is not exposed to the load flake that
// quarantined the #56 happy path above: a tool call keeps the turn open
// exactly long enough for the accept to land, and the turn then completes
// normally.
#[test]
fn an_accepted_steer_is_announced_before_it_drains() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        // Turn 1: a tool call gives the steer time to arrive mid-turn.
        Reply::Sse(sse_tool_call("shell", r#"{"command":"sleep 1"}"#)),
        // Turn 1 continues after the drain, and the injected message shows.
        Reply::Sse(sse_text("after the tool")),
        // Turn 2 (the follow-up) answers.
        Reply::Sse(sse_text("followed up")),
    ]));
    let box_ = sandbox("rpc-queue-queued");
    let script = [
        cmd("p1", "prompt", r#""message":"sleep please""#),
        cmd("s1", "steer", r#""message":"a note""#),
        cmd("f1", "follow_up", r#""message":"later""#),
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
        "clean exit: {}",
        stderr(&output)
    );
    let records = json_lines(&output);

    // Both accepts are present, with the mode each was submitted under.
    let queued: Vec<&serde_json::Value> = records
        .iter()
        .filter(|line| line["type"] == "queue-queued")
        .collect();
    assert_eq!(
        queued.len(),
        2,
        "one accept per accepted message: {queued:?}"
    );
    assert_eq!(queued[0]["mode"], "steer", "the steer accept");
    assert_eq!(queued[1]["mode"], "follow-up", "the follow-up accept");

    // Ordering is the contract: every accept precedes the drain it belongs
    // to. Position in the stream, not just presence, is what a script reads.
    let position = |kind: &str| -> Vec<usize> {
        records
            .iter()
            .enumerate()
            .filter(|(_, line)| line["type"] == kind)
            .map(|(index, _)| index)
            .collect()
    };
    let accepts = position("queue-queued");
    let flushes = position("queue-flushed");
    assert!(
        !flushes.is_empty(),
        "the steer drain is on the stream too: {:?}",
        records
            .iter()
            .map(|l| l["type"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
    );
    assert!(
        accepts[0] < flushes[0],
        "accept at {} must precede the drain at {}: {accepts:?} {flushes:?}",
        accepts[0],
        flushes[0]
    );
    // And the drained text is what was accepted - accept and drain agree.
    let drained = records
        .iter()
        .find(|line| line["type"] == "queue-flushed")
        .expect("a drain");
    assert_eq!(drained["text"], "a note");
    assert_eq!(drained["mode"], "steer", "the drain names the same mode");
}

// Verifies: gh #426 - a queue that never reached the turn does not claim a
// drain. A steer accepted and then cancelled before the next model-call
// boundary must show the ACCEPT and no `queue-flushed`: "accepted, never
// injected" is a distinct fact from "never arrived", and a client that saw a
// flush here would think the model had read the message when it had not.
//
// The turn is cancelled from the mock side (a long tool call plus a cancel),
// so the drain genuinely cannot happen.
#[test]
fn a_cancelled_queue_announces_the_accept_and_no_drain() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        // One long tool call; the cancel below lands during it, so the turn
        // ends before the next boundary and `drain_steer` never runs.
        Reply::Sse(sse_tool_call("shell", r#"{"command":"sleep 20"}"#)),
        Reply::Sse(sse_text("never reached")),
    ]));
    let box_ = sandbox("rpc-queue-cancelled");
    let script = [
        cmd("p1", "prompt", r#""message":"long sleep""#),
        cmd("s1", "steer", r#""message":"dropped note""#),
        cmd("c1", "cancel", ""),
        cmd("d1", "shutdown", ""),
    ]
    .concat();
    let output = box_.run_with_stdin(
        Some(&mock),
        &["--yolo", "--model", "zen-free", "--mode", "rpc"],
        &script,
    );
    let records = json_lines(&output);
    // Prove the script actually ran: the steer's response says "queued".
    // Without this the two assertions below could pass on a stream where
    // nothing was ever accepted (a vacuous green).
    let queued_response = records
        .iter()
        .find(|line| line["type"] == "response" && line["id"] == "s1")
        .unwrap_or_else(|| panic!("a response for s1: {records:?}"));
    assert_eq!(
        queued_response["data"]["disposition"], "queued",
        "the steer really was accepted"
    );
    let accepts: Vec<&serde_json::Value> = records
        .iter()
        .filter(|line| line["type"] == "queue-queued")
        .collect();
    assert_eq!(accepts.len(), 1, "the steer was accepted: {accepts:?}");
    assert_eq!(accepts[0]["mode"], "steer");
    // And the run really was cancelled before a boundary.
    assert!(
        records
            .iter()
            .any(|line| line["type"] == "turn-end" && line["stop_reason"] == "cancelled"),
        "the turn ended cancelled: {:?}",
        records
            .iter()
            .filter(|l| l["type"] == "turn-end")
            .collect::<Vec<_>>()
    );
    // The honest half: no flush for a message no boundary ever drained.
    let drained: Vec<&serde_json::Value> = records
        .iter()
        .filter(|line| line["type"] == "queue-flushed")
        .collect();
    assert!(
        drained.is_empty(),
        "a cancelled turn must not claim it injected the message: {drained:?}"
    );
}

// Verifies: gh #56 (malformed input and unknown commands fail loud,
// never silent, and never kill the loop).
// Same quarantine as above (gh #195): missing turn-end under load.
#[test]
#[ignore = "turn-end dropped under load - tracked in gh #195"]
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
