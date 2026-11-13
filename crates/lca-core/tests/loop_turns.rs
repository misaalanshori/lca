//! The turn flow: deltas arriving, usage recorded, tool calls running in order,
//! a denied call reaching the model, and the gh #45 hook doubles (`settle`
//! continuation and stream observation) at turn level.
//!
//! Gh #548 phase 4 split `tests/loop.rs` by subject, so each of these rows now
//! reports as `loop_turns::<row>` instead of `loop::<row>` - the trade the cycle
//! report states with both the raw and normalised comparison.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_core::TurnEvent;
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_permissions::Decision;
use lca_protocol::{DispatchError, Record, ToolResultStatus};
use lca_session::ViewMode;
use lca_testkit::{FakeProvider, fake_usage};
use std::sync::Arc;
use std::sync::Mutex;

// Verifies: FR-CORE-4 (partial content renders as it arrives)
#[tokio::test]
async fn streams_text_deltas_as_they_arrive() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("Hello, ")
                .text("world")
                .usage(fake_usage(10, 5, 0, 10))
        })
        .build();
    let mut h = harness("stream", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        sink.count(|e| matches!(e, TurnEvent::TextDelta(_))),
        2,
        "two deltas, in order"
    );
    assert_eq!(sink.texts(), "Hello, world");
    assert!(
        sink.events.iter().any(|e| matches!(
            e,
            TurnEvent::TurnEnded {
                status: lca_core::TurnStatus::Ok,
                ..
            }
        )),
        "turn end is signalled"
    );
}

// Verifies: FR-CORE-8 (token count and cost, including cache fields, are
// recorded on each turn)
#[tokio::test]
async fn records_usage_with_cache_fields_per_turn() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("done").usage(lca_protocol::Usage {
                input: 40,
                output: 12,
                cache_read: 1200,
                cache_write: 0,
                ..Default::default()
            })
        })
        .build();
    let mut h = harness("usage", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.usage.input, 40);
    assert_eq!(outcome.usage.cache_read, 1200);

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    let assistant = read
        .records
        .iter()
        .find_map(|r| match r {
            Record::Assistant { usage, content, .. } => Some((usage.clone(), content.clone())),
            _ => None,
        })
        .expect("assistant record");
    let usage = assistant.0.expect("usage on the record");
    assert_eq!(usage.cache_read, 1200);
    assert_eq!(usage.output, 12);
    let text = assistant
        .1
        .iter()
        .find_map(|b| match b {
            lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .expect("text block");
    assert_eq!(text, "done");
}

// The full tool loop: model calls a tool, the result goes back, the model
// finishes (docs/flows.md, "a turn with a tool call").
#[tokio::test]
async fn runs_tool_calls_sequentially_until_the_model_stops() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("read", r#"{"path":"notes.md"}"#)
                .usage(fake_usage(100, 20, 0, 100))
        })
        .turn(|t| {
            t.text("The file contains three notes.")
                .usage(fake_usage(200, 30, 100, 100))
        })
        .build();
    let mut h = harness("tool-loop", provider, default_config());
    std::fs::write(h.project.join("notes.md"), "three notes").expect("write");
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "what is in notes.md?", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert!(
        sink.events.iter().any(|e| matches!(e, TurnEvent::ToolFinished(r) if r.status == ToolResultStatus::Ok && r.content.contains("three notes"))),
        "tool ran and its result surfaced"
    );
    assert!(sink.texts().contains("three notes"));

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    let kinds: Vec<&str> = read.records.iter().map(|r| r.type_tag()).collect();
    assert_eq!(
        kinds,
        vec![
            "session-start",
            "user",
            "assistant",
            "tool-call",
            "tool-result",
            "assistant"
        ],
        "the log tells the whole turn"
    );

    // The tool result reaches the next provider call, keyed by call id.
    let request = h.provider.last_request().expect("request recorded");
    assert!(
        request
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("call-0")),
        "tool results round-trip into the next request by call id"
    );
}

// Verifies: FR-TOOL-7 and session-log-format's attachment rule: an
// over-limit tool result is spilled to the session's attachment directory
// and the record references its hash, so nothing the model saw is lost.
#[tokio::test]
async fn over_limit_tool_output_is_recorded_as_an_attachment() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("read", r#"{"path":"big.txt"}"#)
                .usage(fake_usage(100, 20, 0, 100))
        })
        .turn(|t| t.text("read it").usage(fake_usage(200, 30, 100, 100)))
        .build();
    let mut h = harness("attachment-spill", provider, default_config());
    let big: String = (1..=20_000).map(|i| format!("line {i}\n")).collect();
    std::fs::write(h.project.join("big.txt"), &big).expect("write");
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    let outcome = turn(&mut h, "read big.txt", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    let (hash, truncated) = read
        .records
        .iter()
        .find_map(|record| match record {
            Record::ToolResult {
                attachment: Some(hash),
                truncated,
                ..
            } => Some((hash.clone(), *truncated)),
            _ => None,
        })
        .expect("the spilled result references an attachment");
    assert!(truncated, "the display was cut");
    assert_eq!(hash.len(), 64, "SHA-256 hex");
    let spilled = std::fs::read_to_string(h.session.dir().join("attachments").join(&hash))
        .expect("the attachment file exists");
    assert!(spilled.contains("line 20000"));
    assert!(spilled.contains("line 1\n"), "the head is preserved");
}

// Verifies: FR-CONC-2 (tool calls within a turn run sequentially, in the
// order the model emitted them)
#[tokio::test]
async fn executes_tool_calls_in_order() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("write", r#"{"path":"a.txt","content":"A"}"#)
                .tool_call("write", r#"{"path":"b.txt","content":"B"}"#)
                .usage(fake_usage(50, 40, 0, 50))
        })
        .turn(|t| t.text("both written").usage(fake_usage(90, 10, 50, 0)))
        .build();
    let mut h = harness("order", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "write both", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let started: Vec<String> = sink
        .events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::ToolStarted(call) => Some(call.name.clone() + ":" + &call.arguments),
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), 2);
    assert!(
        started[0].contains("a.txt"),
        "first call first: {started:?}"
    );
    assert!(started[1].contains("b.txt"));
}

// A denied shell command is recorded as denied and the model continues
// (FR-TOOL-3 through the core; a hook denial that never prompts is
// FR-CORE-10, covered when hooks land in Phase 2).
#[tokio::test]
async fn denied_commands_reach_the_model_as_denied_results() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"rm -rf /"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("understood").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("deny", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![Decision::Denied],
        asked: vec![],
    };

    let outcome = turn(&mut h, "clean up", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(prompt.asked.len(), 1, "the prompt showed the exact command");
    assert!(
        prompt.asked[0].contains("rm -rf /"),
        "FR-UI-4: exact command"
    );
    assert!(
        sink.events.iter().any(
            |e| matches!(e, TurnEvent::ToolFinished(r) if r.status == ToolResultStatus::Denied)
        )
    );

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    assert!(
        read.records.iter().any(|r| matches!(
            r,
            Record::ToolResult {
                status: ToolResultStatus::Denied,
                ..
            }
        )),
        "denial recorded in the log"
    );
    assert!(
        read.records
            .iter()
            .any(|r| matches!(r, Record::Permission { .. })),
        "permission decision recorded"
    );
}

// Verifies: cycle 9 - a tool call the user denies still *reaches* the
// interface. The start event used to fire only after the permission check,
// so a denied call produced a `tool-result` with no `tool-call` before it
// (docs/headless.md pairs them) and the transcript showed nothing at all:
// no card, no `denied` label, no tint. The start now fires when the model
// asks, which is also when the session log records the request.
#[tokio::test]
async fn a_denied_tool_call_is_announced_before_it_settles() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"rm -rf /"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("understood").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("denied-start", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "clean up", &mut sink, &mut prompt).await;
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "error: {:?}",
        outcome.error
    );
    assert_eq!(prompt.asked.len(), 1, "the denial came from the prompt");
    let started = sink
        .events
        .iter()
        .position(|e| matches!(e, TurnEvent::ToolStarted(_)))
        .expect("the request was announced");
    let finished = sink
        .events
        .iter()
        .position(
            |e| matches!(e, TurnEvent::ToolFinished(r) if r.status == ToolResultStatus::Denied),
        )
        .expect("and it settled as denied");
    assert!(
        started < finished,
        "start precedes result: {:?}",
        sink.events
    );

    // The session log agrees with the events: the request was recorded
    // before the refusal, in both directions.
    let log = h.store.read(&h.session).expect("read");
    let call = log
        .records
        .iter()
        .position(|r| matches!(r, Record::ToolCall { .. }))
        .expect("tool-call record");
    let result = log
        .records
        .iter()
        .position(|r| matches!(r, Record::ToolResult { .. }))
        .expect("tool-result record");
    assert!(call < result, "record order: {:?}", log.records);
}

// Verifies: gh #45 - a settle handler appends entries and continues
// one more provider request; the turn then settles (no loop).
#[tokio::test]
async fn a_settle_handler_continues_exactly_one_more_request() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("first").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("second").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let continued = Arc::new(Mutex::new(false));
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(SettleOnce {
        continued: continued.clone(),
    }));
    let mut config = default_config();
    config.extensions = Arc::new(registry);
    let mut h = harness("settle-once", provider, config);
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "go", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(h.provider.call_count(), 2, "exactly one continuation");
    assert_eq!(sink.texts(), "firstsecond");
    // The append rode a context message the second request saw.
    let requests = h.provider.requests();
    assert_eq!(requests.len(), 2);
    let second_texts: Vec<String> = requests[1]
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        second_texts.iter().any(|text| text.contains("settle-note")),
        "the append reached the continued request: {second_texts:?}"
    );
    // And the transcript kept it.
    let log = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    assert!(
        log.records.iter().any(|record| matches!(
            record,
            Record::CustomMessage { content, .. } if content.contains("settle-note")
        )),
        "settle append recorded"
    );
}

// Verifies: gh #45 (PG-051) - stream observers see the normalized
// sequence in order with its payloads.
#[tokio::test]
async fn stream_observers_see_the_normalized_sequence_in_order() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("hi")
                .tool_call("read", r#"{"path":"a"}"#)
                .usage(fake_usage(10, 5, 0, 0))
        })
        .turn(|t| t.text("done").usage(fake_usage(10, 5, 0, 0)))
        .build();
    std::fs::write(scratch("stream-order").join("x"), "").expect("scratch");
    let kinds = Arc::new(Mutex::new(Vec::new()));
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(StreamLog {
        kinds: kinds.clone(),
    }));
    let mut config = default_config();
    config.extensions = Arc::new(registry);
    let mut h = harness("stream-order", provider, config);
    std::fs::write(h.project.join("a"), "a").expect("write");
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![Decision::Always],
        asked: vec![],
    };

    let outcome = turn(&mut h, "read it", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let kinds = kinds.lock().expect("kinds").clone();
    assert!(
        kinds.iter().any(|entry| entry == "text-delta:hi"),
        "text delta with payload: {kinds:?}"
    );
    let tool_at = kinds
        .iter()
        .position(|entry| entry.starts_with("tool-call-start:read"))
        .expect("tool-call-start names the tool");
    let text_at = kinds
        .iter()
        .position(|entry| entry == "text-delta:hi")
        .expect("text first");
    assert!(text_at < tool_at, "arrival order kept: {kinds:?}");
    assert!(
        kinds.iter().any(|entry| entry.starts_with("usage:")),
        "usage closes the sequence: {kinds:?}"
    );
}

/// A hooks double: continues once with an appended note (gh #45's
/// actionable settle), then settles.
struct SettleOnce {
    continued: Arc<Mutex<bool>>,
}

impl ExtensionDispatch for SettleOnce {
    fn name(&self) -> &str {
        "settle-once"
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::HooksSettle]
    }
    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.name().to_string(),
            world: "tool",
        })))
    }
    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn invoke_command(
        &self,
        _name: &str,
        _argument: &str,
    ) -> Result<lca_protocol::CommandEffect, DispatchError> {
        Ok(lca_protocol::CommandEffect::None)
    }
    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
    }
    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a lca_protocol::PostToolObservation,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_turn_end<'a>(
        &'a self,
        _rounds: u32,
        _tool_calls: u32,
        _status: &'a str,
    ) -> DispatchFuture<'a, Result<lca_protocol::SettleDecision, DispatchError>> {
        let mut continued = self.continued.lock().expect("continued");
        let decision = if *continued {
            lca_protocol::SettleDecision::default()
        } else {
            *continued = true;
            lca_protocol::SettleDecision {
                append: Some("settle-note".to_string()),
                continue_once: true,
            }
        };
        Box::pin(std::future::ready(Ok(decision)))
    }
}

/// A stream double: records normalized event kinds in arrival order.
struct StreamLog {
    kinds: Arc<Mutex<Vec<String>>>,
}

impl ExtensionDispatch for StreamLog {
    fn name(&self) -> &str {
        "stream-log"
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::HooksStream]
    }
    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.name().to_string(),
            world: "tool",
        })))
    }
    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn invoke_command(
        &self,
        _name: &str,
        _argument: &str,
    ) -> Result<lca_protocol::CommandEffect, DispatchError> {
        Ok(lca_protocol::CommandEffect::None)
    }
    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
    }
    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a lca_protocol::PostToolObservation,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_stream_event<'a>(
        &'a self,
        _provider: &'a str,
        _model: &'a str,
        kind: &'a str,
        data: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        self.kinds
            .lock()
            .expect("kinds")
            .push(format!("{kind}:{data}"));
        Box::pin(std::future::ready(Ok(())))
    }
}
