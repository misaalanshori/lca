//! GitHub #56, live: `--mode rpc` drives one session across commands.
//! Replays pi's `rpc-commands.md` happy path (prompt → steer →
//! follow_up → cancel → shutdown) and asserts semantic outcomes plus
//! multi-turn session persistence.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::{Reply, json_lines, rt, sandbox, sse_text, sse_tool_call, start_mock, stderr};

/// Drive `--mode rpc` with **interleaved** writes: each element of `lines` is
/// written after sleeping its own delay, so a row can be explicit about *when*
/// each command lands relative to the turn. Closing stdin ends the session (EOF
/// is the documented exit).
///
/// gh #489: `run_with_stdin` writes the whole script up front, which is *a*
/// valid client but not the only one — and until the fix it was the only shape
/// the suite exercised. The bug was invisible to it precisely because one
/// buffered write usually delivers every line before the turn starts; a client
/// that writes while the turn runs loses the turn. This driver is what makes
/// the difference observable, so the rows below can assert the documented
/// promises instead of retrying until a session happens to survive.
fn drive_interleaved(
    box_: &common::Sandbox,
    mock: &common::Mock,
    lines: &[(std::time::Duration, String)],
) -> std::process::Output {
    use std::io::Write as _;
    use std::process::Stdio as Std;
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_lca"));
    command
        .args(["--yolo", "--model", "zen-free", "--mode", "rpc"])
        .current_dir(box_.project())
        .env("HOME", &box_.home)
        .env("USERPROFILE", &box_.home)
        .env("OPENAI_BASE_URL", mock.url())
        .env("OPENAI_API_KEY", "test-key")
        .env("OPENAI_MODEL", "test-model")
        .env("LCA_UPDATE_CHECK", "false")
        .stdin(Std::piped())
        .stdout(Std::piped())
        .stderr(Std::piped());
    let mut child = command.spawn().expect("spawn lca");
    {
        let mut stdin = child.stdin.take().expect("piped stdin");
        for (pause, line) in lines {
            if !pause.is_zero() {
                std::thread::sleep(*pause);
            }
            stdin.write_all(line.as_bytes()).expect("write line");
            stdin.flush().expect("flush");
        }
        // Dropping stdin is the documented shutdown-as-EOF path.
    }
    child.wait_with_output().expect("run lca")
}

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
// **Timing is absorbed, behavior never is** - the house pattern from
// `conformance_diff.rs`'s pty-io row, and the same one the sibling
// cancelled-queue row uses. The steer is written to a pipe up front, so under
// load it can arrive after the turn's only tool call has already returned and
// the boundary has passed; the drain then never happens and the precondition
// (a `queue-flushed` on the stream) is simply not met. That is a miss in the
// *scenario*: retried once, and a second miss fails loud. No assertion is
// relaxed and no sleep widens the window.
#[test]
fn an_accepted_steer_is_announced_before_it_drains() {
    // One attempt; `None` when the precondition (a steer drain) was not met.
    let attempt = || -> Option<Vec<serde_json::Value>> {
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
        let records = json_lines(&output);
        // The precondition this scenario needs: the drain actually happened.
        let drained = records.iter().any(|line| line["type"] == "queue-flushed");
        drained.then_some(records)
    };

    // Sequential, not a tuple: `(attempt(), attempt())` would evaluate both
    // eagerly and run the whole journey twice even when the first is fine.
    let records = match attempt().or_else(attempt) {
        Some(records) => records,
        None => panic!(
            "the steer never drained in two attempts; the accept/drain \
             ordering is not landing inside the turn"
        ),
    };

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
// **Timing is absorbed, behavior never is** - the house pattern from
// `conformance_diff.rs`'s pty-io row. The script is fire-and-forget on a
// pipe, so under load the cancel can reach the run before the turn has begun
// streaming inside its tool call, and the scenario then never reaches the
// state this row is about. That is a miss in the *scenario*, not in the
// product: it is retried once, and a second miss fails loud. The assertions
// are never relaxed and no sleep widens the window - a sleep is only a
// slower race.
#[test]
fn a_cancelled_queue_announces_the_accept_and_no_drain() {
    // One attempt at the whole journey; `None` when the precondition (a
    // cancelled turn-end) was not met this time.
    let attempt = || -> Option<Vec<serde_json::Value>> {
        let runtime = rt();
        let mock = runtime.block_on(start_mock(vec![
            // One long tool call; the cancel below lands during it, so the
            // turn ends before the next boundary and `drain_steer` never runs.
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
        // The precondition this scenario needs: the turn really was cancelled.
        let cancelled = records
            .iter()
            .any(|line| line["type"] == "turn-end" && line["stop_reason"] == "cancelled");
        cancelled.then_some(records)
    };

    // Sequential, not a tuple: `(attempt(), attempt())` would evaluate both
    // eagerly and run the whole journey twice even when the first is fine.
    let records = match attempt().or_else(attempt) {
        Some(records) => records,
        None => panic!(
            "the scenario never reached a cancelled turn in two attempts; \
             the steer/cancel ordering is not landing on a streaming turn"
        ),
    };

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

// ---------------------------------------------------------------------------
// gh #489: the four promises `docs/headless.md` makes about the RPC loop.
//
// The defect was `run.take()` **inside** a `select!` branch future. `select!`
// rebuilds every branch future on each loop iteration, so when the biased stdin
// branch became ready while a turn was pending, the select dropped the future
// holding the taken run - and `turn_state`, which `start_run!` had moved into
// that same future, died with it. The RPC session then answered `"started"` to
// prompts it could never run.
//
// Every row below writes its lines **interleaved** with sleeps rather than
// buffering them up front. That is the shape that exposes the bug: one buffered
// write usually delivers every line before the first turn starts, which is why
// the suite never saw this and why the old rows needed a retry to absorb it.
// ---------------------------------------------------------------------------

/// The provider delay used to guarantee a turn is genuinely mid-flight when the
/// next stdin line lands. Long enough that no scheduler hiccup closes the
/// window, short enough that a row costs under two seconds.
const MID_TURN: std::time::Duration = std::time::Duration::from_millis(400);

/// Write this line immediately - the client is not pausing, the line is just
/// first.
const ZERO: std::time::Duration = std::time::Duration::ZERO;

// Verifies: gh #489 (promise 1) - `prompt` answers `"disposition": "started"`
// and a run ACTUALLY began: the provider is asked, and the turn ends.
//
// Pre-fix: `prompt` + one more line 400 ms later produced `turn-start`,
// `message-start`, and then nothing - zero model requests, zero `turn-end`, exit
// 0. Deterministic at every non-zero delay.
#[test]
fn a_prompt_written_before_a_second_line_still_runs_its_turn() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::SseAfter(
        800,
        sse_text("first done"),
    )]));
    let box_ = sandbox("rpc-489-prompt-runs");
    let output = drive_interleaved(
        &box_,
        &mock,
        &[
            (ZERO, cmd("p1", "prompt", r#""message":"hello""#)),
            // Any second line mid-turn used to drop the run; `steer` is the
            // realistic client, and the next row shows it is not the trigger.
            (MID_TURN, cmd("s1", "steer", r#""message":"a note""#)),
        ],
    );
    let records = json_lines(&output);
    let turns = records.iter().filter(|l| l["type"] == "turn-end").count();
    assert_eq!(
        turns, 1,
        "the prompt's turn must end even though another line arrived mid-flight: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|l| l["type"] == "text" && l["content"] == "first done"),
        "and the provider was actually asked: {records:?}"
    );
    assert!(
        mock.request_count() >= 1,
        "at least one model request reached the provider"
    );
    let _ = std::fs::remove_dir_all(&box_.root);
}

// Verifies: gh #489 - **the trigger is any stdin readiness, not the steer.** A
// bare `prompt` followed by `shutdown` mid-turn lost the turn 3 times in 30
// before the fix; this row is the deterministic form of that observation.
#[test]
fn a_shutdown_arriving_mid_turn_does_not_discard_the_turn() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::SseAfter(
        800,
        sse_text("done anyway"),
    )]));
    let box_ = sandbox("rpc-489-shutdown-midturn");
    let output = drive_interleaved(
        &box_,
        &mock,
        &[
            (ZERO, cmd("p1", "prompt", r#""message":"hello""#)),
            (MID_TURN, cmd("d1", "shutdown", "")),
        ],
    );
    let records = json_lines(&output);
    assert_eq!(
        records.iter().filter(|l| l["type"] == "turn-end").count(),
        1,
        "shutdown mid-turn must let the turn finish, not destroy it: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|l| l["type"] == "text" && l["content"] == "done anyway"),
        "the reply landed: {records:?}"
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "and the documented exit code holds: {}",
        stderr(&output)
    );
    let _ = std::fs::remove_dir_all(&box_.root);
}

// Verifies: gh #489 (promise 4) - **multi-turn state survives.** After a steer
// arrived mid-run, the session was dead: a second `prompt` answered
// `{"disposition":"started"}` while the provider received nothing, because
// `turn_state` had been moved into the dropped future and `start_run!` silently
// found `None`.
//
// Pre-fix this row saw exactly one model request for two prompts.
#[test]
fn a_second_prompt_after_a_mid_turn_steer_still_runs() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::SseAfter(600, sse_text("first done")),
        Reply::Sse(sse_text("second done")),
    ]));
    let box_ = sandbox("rpc-489-multi-turn");
    let output = drive_interleaved(
        &box_,
        &mock,
        &[
            (ZERO, cmd("p1", "prompt", r#""message":"one""#)),
            // Arrives mid-turn (well inside the 600 ms reply): this is what used
            // to kill the session.
            (
                std::time::Duration::from_millis(150),
                cmd("s1", "steer", r#""message":"nudge""#),
            ),
            // Far enough past turn one that the 600 ms reply has landed even
            // with a slow scheduler. The margin is deliberately generous: a
            // tight wait here would make the row fail on *its own* timing under
            // load rather than on the product, which is the mistake the
            // original flake taught. `a_prompt_written_before_...` above is the
            // row that pins mid-turn arrival; this one pins *multi-turn survival*
            // and needs turn one finished, not in flight.
            (
                std::time::Duration::from_millis(3000),
                cmd("p2", "prompt", r#""message":"two""#),
            ),
        ],
    );
    let records = json_lines(&output);
    let responses: Vec<&serde_json::Value> = records
        .iter()
        .filter(|line| line["type"] == "response")
        .collect();
    let disposition = |id: &str| {
        responses
            .iter()
            .find(|line| line["id"] == id)
            .map(|line| {
                line["data"]["disposition"]
                    .as_str()
                    .unwrap_or("")
                    .to_string()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        disposition("p2"),
        "started",
        "p2 is accepted: {responses:?}"
    );
    assert!(
        mock.request_count() >= 2,
        "two prompts must reach the provider, not one (got {}): {records:?}",
        mock.request_count()
    );
    assert_eq!(
        records.iter().filter(|l| l["type"] == "turn-end").count(),
        2,
        "both turns end: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|l| l["type"] == "text" && l["content"] == "second done"),
        "the second turn's reply landed: {records:?}"
    );
    let _ = std::fs::remove_dir_all(&box_.root);
}

// Verifies: gh #489 (promise 2) - **`follow_up` submits after the run ends.**
// This promise was previously untestable through the interleaved path because
// the run it hangs off had already been destroyed; the only rows that reached it
// buffered the whole script up front.
#[test]
fn a_follow_up_written_mid_turn_submits_after_the_run_ends() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::SseAfter(600, sse_text("first done")),
        Reply::Sse(sse_text("followed up")),
    ]));
    let box_ = sandbox("rpc-489-follow-up");
    let output = drive_interleaved(
        &box_,
        &mock,
        &[
            (ZERO, cmd("p1", "prompt", r#""message":"one""#)),
            (MID_TURN, cmd("f1", "follow_up", r#""message":"later""#)),
        ],
    );
    let records = json_lines(&output);
    assert!(
        records.iter().any(|l| {
            l["type"] == "response" && l["id"] == "f1" && l["data"]["disposition"] == "queued"
        }),
        "the follow-up was accepted: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|l| l["type"] == "text" && l["content"] == "followed up"),
        "and it actually ran after the first turn ended: {records:?}"
    );
    assert_eq!(
        records.iter().filter(|l| l["type"] == "turn-end").count(),
        2,
        "first turn plus the follow-up's: {records:?}"
    );
    let _ = std::fs::remove_dir_all(&box_.root);
}

// Verifies: gh #489 (promise 3) - **`shutdown` drains queued follow-ups first.**
// Before the fix a mid-turn shutdown dropped the run, so there was no run end to
// hang the queue off and the accepted follow-up was silently discarded while the
// process exited 0.
#[test]
fn a_shutdown_drains_a_queued_follow_up_before_exiting() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::SseAfter(600, sse_text("first done")),
        Reply::Sse(sse_text("drained on the way out")),
    ]));
    let box_ = sandbox("rpc-489-shutdown-drains");
    let output = drive_interleaved(
        &box_,
        &mock,
        &[
            (ZERO, cmd("p1", "prompt", r#""message":"one""#)),
            (MID_TURN, cmd("f1", "follow_up", r#""message":"queued""#)),
            (MID_TURN, cmd("d1", "shutdown", "")),
        ],
    );
    let records = json_lines(&output);
    assert!(
        records
            .iter()
            .any(|l| l["type"] == "text" && l["content"] == "drained on the way out"),
        "the queued follow-up ran before exit, as documented: {records:?}"
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "and the exit code is still 0: {}",
        stderr(&output)
    );
    let _ = std::fs::remove_dir_all(&box_.root);
}

// Verifies: gh #489 - **a bare `prompt` whose stdin closes at EOF runs.** The
// 2-in-30 case: the turn was lost even with no second *command*, because EOF is
// itself a stdin event and the biased branch read it mid-turn.
//
// Repeated, because this is the narrowest window in the family: EOF is queued as
// soon as the one line is written, so whether it is observed before or after the
// turn's first poll is a genuine race. The fix removes the consequence, so all
// rounds must pass; on pre-fix code this fails within a few rounds, which is why
// the count is here rather than a single attempt.
#[test]
fn a_prompt_with_stdin_closed_immediately_still_runs() {
    for round in 0..8 {
        let runtime = rt();
        let mock = runtime.block_on(start_mock(vec![Reply::SseAfter(
            300,
            sse_text("still ran"),
        )]));
        let box_ = sandbox(&format!("rpc-489-eof{round}"));
        let output = drive_interleaved(
            &box_,
            &mock,
            // One line, and stdin closes the instant it is written.
            &[(ZERO, cmd("p1", "prompt", r#""message":"hello""#))],
        );
        let records = json_lines(&output);
        assert_eq!(
            records.iter().filter(|l| l["type"] == "turn-end").count(),
            1,
            "round {round}: EOF mid-turn must not destroy the turn: {records:?}"
        );
        assert!(
            records
                .iter()
                .any(|l| l["type"] == "text" && l["content"] == "still ran"),
            "round {round}: the reply landed: {records:?}"
        );
        let _ = std::fs::remove_dir_all(&box_.root);
    }
}

// ---------------------------------------------------------------------------
// gh #268: RPC honours `queue.follow_up_mode`.
//
// RPC was the third surface to owe this ordering rule and the only one that did
// not pay it: TUI (`tui/commands.rs`) and `lca-ui`'s queue both branch on
// `QueueMode`, while `rpc.rs` drained its `follow_ups` one at a time
// unconditionally. So an embedder driving LCA over RPC saw different semantics
// from the same session in a terminal, and nothing in the suite noticed because
// every existing row queues exactly one follow-up.
//
// **The rows assert what the provider received**, not the queue's internals: the
// mock records each request it is asked to serve, so "serial" and "joined" are
// distinguishable from the outside. A broken drain that dropped a message would
// also fail these, which is the point.
// ---------------------------------------------------------------------------

/// Write `queue.follow_up_mode` into the sandbox's user config.
fn set_follow_up_mode(box_: &common::Sandbox, mode: &str) {
    let dir = box_.state_dir();
    std::fs::create_dir_all(&dir).expect("mkdir state");
    let path = dir.join("config.toml");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut text = existing;
    if !text.contains("queue.follow_up_mode") {
        text.push_str(&format!("\nqueue.follow_up_mode = \"{mode}\"\n"));
    }
    std::fs::write(&path, text).expect("write config");
}

/// The **newest** user message of each request the provider served, in order.
///
/// The transcript is cumulative, so reading every user message would report
/// `one|two|three` for the third request whether the queue joined them or ran
/// them serially - a joined drain and a serial one would look identical. Only the
/// last user message says what the queue handed over for *that* turn.
fn served_prompts(mock: &common::Mock) -> Vec<String> {
    mock.bodies()
        .iter()
        .filter_map(|body| {
            let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
            let messages = parsed["messages"].as_array()?;
            messages
                .iter()
                .filter(|m| m["role"] == "user")
                .filter_map(|m| m["content"].as_str())
                .next_back()
                .map(str::to_string)
        })
        .collect()
}

// Verifies: gh #268 - `one-at-a-time` (the default) delivers two queued
// follow-ups as **two separate turns**, in the order they were queued.
//
// This is the row that passes today, kept as the control: it proves the new
// `all` row below is measuring the mode and not some unrelated change in the
// drain.
#[test]
fn rpc_one_at_a_time_delivers_queued_follow_ups_serially() {
    let runtime = rt();
    // Same shape as the `all` row - slow first turn, both follow-ups inside it -
    // so the pair differs in exactly one variable: the configured mode.
    let mock = runtime.block_on(start_mock(vec![
        Reply::SseAfter(1200, sse_text("first done")),
        Reply::Sse(sse_text("second")),
        Reply::Sse(sse_text("third")),
    ]));
    let box_ = sandbox("rpc-268-one-at-a-time");
    set_follow_up_mode(&box_, "one-at-a-time");
    let output = drive_interleaved(
        &box_,
        &mock,
        &[
            (ZERO, cmd("p1", "prompt", r#""message":"one""#)),
            (
                std::time::Duration::from_millis(150),
                cmd("f1", "follow_up", r#""message":"two""#),
            ),
            (
                std::time::Duration::from_millis(150),
                cmd("f2", "follow_up", r#""message":"three""#),
            ),
        ],
    );
    let prompts = served_prompts(&mock);
    let turns = json_lines(&output)
        .iter()
        .filter(|l| l["type"] == "turn-end")
        .count();
    assert_eq!(
        turns, 3,
        "one-at-a-time runs one turn per queued message: {prompts:?}"
    );
    // Serial: each request carries its own message, never a joined pair.
    assert!(
        prompts.iter().any(|p| p.contains("two")),
        "the second message reached the provider on its own: {prompts:?}"
    );
    assert!(
        prompts.iter().any(|p| p.contains("three")),
        "and the third too: {prompts:?}"
    );
    assert!(
        !prompts
            .iter()
            .any(|p| p.contains("two") && p.contains("three")),
        "never joined, because one-at-a-time flushes one per turn: {prompts:?}"
    );
    let _ = std::fs::remove_dir_all(&box_.root);
}

// Verifies: gh #268 - `all` **joins** two queued follow-ups into one turn, the
// behaviour TUI and `lca-ui`'s queue already implement (`texts.join("\n\n")`).
//
// This is the row that fails on pre-fix code: RPC ignored the setting and always
// drained one message per turn, so an embedder asking for `all` got serial turns.
#[test]
fn rpc_all_joins_queued_follow_ups_into_one_turn() {
    let runtime = rt();
    // The first turn is deliberately slow (1200 ms) and both follow-ups are
    // written well inside it. That matters: a follow-up written *after* the run
    // ends starts a run of its own, so only messages still queued at a run end can
    // be joined. With a 600 ms turn and 400 ms gaps, the second write landed after
    // `turn-end` and the row measured serial delivery for a reason that had
    // nothing to do with the mode.
    let mock = runtime.block_on(start_mock(vec![
        Reply::SseAfter(1200, sse_text("first done")),
        Reply::Sse(sse_text("joined reply")),
    ]));
    let box_ = sandbox("rpc-268-all");
    set_follow_up_mode(&box_, "all");
    let output = drive_interleaved(
        &box_,
        &mock,
        &[
            (ZERO, cmd("p1", "prompt", r#""message":"one""#)),
            (
                std::time::Duration::from_millis(150),
                cmd("f1", "follow_up", r#""message":"two""#),
            ),
            (
                std::time::Duration::from_millis(150),
                cmd("f2", "follow_up", r#""message":"three""#),
            ),
        ],
    );
    let prompts = served_prompts(&mock);
    let turns = json_lines(&output)
        .iter()
        .filter(|l| l["type"] == "turn-end")
        .count();
    assert_eq!(
        turns, 2,
        "`all` drains both queued follow-ups in one turn, so two turns total: \
         {prompts:?}"
    );
    assert!(
        prompts
            .iter()
            .any(|p| p.contains("two") && p.contains("three")),
        "and the provider saw them joined, TUI's `texts.join(\"\\n\\n\")` shape: \
         {prompts:?}"
    );
    let _ = std::fs::remove_dir_all(&box_.root);
}
