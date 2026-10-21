//! `HeadlessSink`'s in-module rows, split from `headless.rs` under the
//! workspace's 1,200-line ceiling (gate 11). Behaviour unchanged.

use super::*;
use lca_core::TurnSink;

pub(super) struct VecWriter(pub(super) std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for VecWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn sink_plain() -> (HeadlessSink, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = HeadlessSink::with_writer(false, false, Box::new(VecWriter(out.clone())));
    (sink, out)
}

/// The bytes written so far, as text.
fn written(out: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> String {
    String::from_utf8(
        out.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
    )
    .expect("utf8")
}

fn sink() -> (HeadlessSink, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = HeadlessSink::with_writer(true, false, Box::new(VecWriter(out.clone())));
    (sink, out)
}

fn lines(out: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> Vec<serde_json::Value> {
    let bytes = out
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    String::from_utf8(bytes)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("json line"))
        .collect()
}

fn tool_call() -> lca_protocol::ToolCall {
    lca_protocol::ToolCall {
        call_id: "call-1".to_string(),
        name: "shell".to_string(),
        arguments: "{}".to_string(),
        parent_call_id: None,
    }
}

// Verifies: gh #44 (the golden turn): a scripted turn emits the
// taxonomy in order - turn-start, message-start, text deltas,
// tool-call, tool-update, tool-result, usage, turn-end - so a
// transcript rebuilds offline.
#[test]
fn the_json_stream_reconstructs_the_transcript() {
    let (mut sink, out) = sink();
    sink.emit_session_start("session-1");
    sink.on_event(TurnEvent::TurnStarted);
    sink.on_event(TurnEvent::MessageStarted { role: "user" });
    sink.on_event(TurnEvent::MessageEnded { role: "user" });
    sink.on_event(TurnEvent::MessageStarted { role: "assistant" });
    sink.on_event(TurnEvent::TextDelta("Hello ".into()));
    sink.on_event(TurnEvent::TextDelta("world".into()));
    sink.on_event(TurnEvent::ToolStarted(tool_call()));
    sink.on_event(TurnEvent::ToolOutputChunk {
        call_id: "call-1".into(),
        chunk: "partial".into(),
    });
    sink.on_event(TurnEvent::ToolFinished(lca_protocol::ToolResult::ok(
        "call-1", "done",
    )));
    sink.on_event(TurnEvent::Usage(lca_protocol::Usage::default()));
    sink.on_event(TurnEvent::AssistantText("Hello world".into()));
    sink.on_event(TurnEvent::MessageEnded { role: "assistant" });
    sink.on_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });

    let types: Vec<String> = lines(&out)
        .iter()
        .map(|line| line["type"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        types,
        vec![
            "session-start",
            "turn-start",
            "message-start",
            "message-end",
            "message-start",
            "text-delta",
            "text-delta",
            "tool-call",
            "tool-update",
            "tool-result",
            "usage",
            "text",
            "message-end",
            "turn-end",
        ]
    );
    let records = lines(&out);
    assert_eq!(records[0]["id"], "session-1");
    assert_eq!(records[2]["role"], "user");
    assert_eq!(records[5]["delta"], "Hello ");
    assert_eq!(records[7]["name"], "shell");
    assert_eq!(records[8]["chunk"], "partial");
}

// Verifies: gh #44 (queue and compaction surface): a flushed steer
// and a compaction round-trip name their modes and reasons.
#[test]
fn queue_and_compaction_events_name_their_modes() {
    let (mut sink, out) = sink();
    sink.on_event(TurnEvent::UserInjected {
        text: "faster".into(),
        mode: "steer".into(),
    });
    sink.on_event(TurnEvent::CompactionStarted {
        reason: "threshold".into(),
    });
    sink.on_event(TurnEvent::CompactionEnded {
        reason: "threshold".into(),
        success: true,
    });
    sink.on_event(TurnEvent::RetryScheduled {
        attempt: 1,
        max: 3,
        delay_ms: 2000,
        error: "overloaded".into(),
    });
    sink.on_event(TurnEvent::RetryFinished { success: true });
    let records = lines(&out);
    let types: Vec<&str> = records
        .iter()
        .map(|line| line["type"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        types,
        vec![
            "queue-flushed",
            "compaction-start",
            "compaction-end",
            "error",
            "retry-scheduled",
            "retry-end",
        ]
    );
    assert_eq!(records[0]["mode"], "steer");
    assert_eq!(records[2]["success"], true);
    assert_eq!(records[4]["max_attempts"], 3);
}

// ---- gh #426: the accept, not just the drain ----

// Verifies: gh #426 - the accept emits its documented line, and it is
// the line the event table promises: `{"type":"queue-queued","mode":…}`
// with no text (the accept carries the mode only; the drain carries the
// text). This is the row the drift guard's exception was hiding.
#[test]
fn an_accepted_message_emits_queue_queued() {
    let (mut sink, out) = sink();
    sink.emit_queue_queued(lca_protocol::SubmitMode::Steer);
    // Field names are the contract, key *order* is not (serde_json's map
    // is not insertion-ordered, and no consumer may depend on it), so the
    // shape is asserted as a parsed object - exactly how the
    // `queue-flushed` row beside this one reads its line.
    let records = lines(&out);
    assert_eq!(records.len(), 1, "one line: {records:?}");
    assert_eq!(records[0]["type"], "queue-queued");
    assert_eq!(records[0]["mode"], "steer");
    assert!(
        records[0].get("text").is_none(),
        "the accept carries no text; the drain does: {:?}",
        records[0]
    );
}

// Verifies: gh #426 - `mode` reports the submit mode actually used. A
// follow-up must not silently read as the steer default, which is the
// failure mode the brief pinned.
#[test]
fn the_accepted_mode_is_the_one_submitted() {
    let (mut sink, out) = sink();
    sink.emit_queue_queued(lca_protocol::SubmitMode::FollowUp);
    let records = lines(&out);
    assert_eq!(records[0]["type"], "queue-queued");
    assert_eq!(
        records[0]["mode"], "follow-up",
        "a follow-up reports follow-up, not the steer default"
    );
}

// Verifies: gh #426 - ordering is the contract: accept then drain, never
// the reverse. One sink, so arrival order is emission order.
#[test]
fn the_accept_precedes_the_drain() {
    let (mut sink, out) = sink();
    sink.emit_queue_queued(lca_protocol::SubmitMode::Steer);
    sink.on_event(TurnEvent::UserInjected {
        text: "faster".into(),
        mode: "steer".into(),
    });
    let records = lines(&out);
    let types: Vec<&str> = records
        .iter()
        .map(|line| line["type"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(types, vec!["queue-queued", "queue-flushed"]);
    assert_eq!(records[1]["text"], "faster", "the drain carries the text");
}

// Verifies: gh #426 - in plain (non-JSON) mode nothing is emitted: the
// human-readable output is the assistant's text, and an accept line
// would pollute it. The gate is the sink's own `json` flag, the same one
// `queue-flushed` already respects.
#[test]
fn the_accept_is_silent_outside_json_mode() {
    let (mut sink, out) = sink_plain();
    sink.emit_queue_queued(lca_protocol::SubmitMode::Steer);
    assert!(
        written(&out).is_empty(),
        "plain mode writes no JSON: {:?}",
        written(&out)
    );
}
