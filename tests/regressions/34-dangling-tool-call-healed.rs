//! Discovered in the cycle-5 chess dogfood (2026-09-29): a turn aborted at
//! the iteration limit (FR-CORE-9), or cancelled mid tool-batch, leaves a
//! persisted `tool-call` with no matching `tool-result`. The next request
//! then carries an assistant tool call with no response, which OpenAI-shaped
//! endpoints reject with HTTP 400 — and since the session log is
//! append-only, every later turn fails the same way with an opaque
//! "unknown error". The session is permanently poisoned.
//!
//! `assemble` heals this: a dangling call gains a synthetic `tool` message
//! before the next non-tool message, so the request is valid again. A normal
//! turn (every call answered) is untouched.

use lca_core::assemble;
use lca_protocol::{FORMAT_VERSION, MessageRole, Record, ToolSource};

// Verifies: NFR-24 (a released-defect guard), FR-CORE-9 (the iteration-limit
// stop must not poison the session).
#[test]
fn a_dangling_tool_call_from_an_aborted_turn_is_healed() {
    let records = vec![
        Record::User {
            v: FORMAT_VERSION,
            ts: 1,
            id: "u1".into(),
            content: "make it".into(),
            attachments: Vec::new(),
            queue: None,
        },
        Record::Assistant {
            v: FORMAT_VERSION,
            ts: 2,
            id: "a1".into(),
            content: Vec::new(),
            reasoning: None,
            model: None,
            provider: None,
            usage: None,
        },
        // The model asked for a tool; the turn ended before it ran.
        Record::ToolCall {
            v: FORMAT_VERSION,
            ts: 3,
            id: "tc1".into(),
            call_id: "call_1".into(),
            name: "shell".into(),
            arguments: "{}".into(),
            source: ToolSource::Builtin,
        },
        Record::User {
            v: FORMAT_VERSION,
            ts: 4,
            id: "u2".into(),
            content: "keep going".into(),
            attachments: Vec::new(),
            queue: None,
        },
    ];
    let assembled = assemble(&records, "system");
    // Every assistant tool call now has a following tool message.
    let assistant_at = assembled
        .messages
        .iter()
        .position(|m| m.role == MessageRole::Assistant && !m.tool_calls.is_empty())
        .expect("the assistant with the call");
    let tool = assembled
        .messages
        .iter()
        .skip(assistant_at + 1)
        .find(|m| m.role == MessageRole::Tool)
        .expect("a healed tool message");
    assert_eq!(tool.tool_call_id.as_deref(), Some("call_1"));
    // It sits before the next user message, so the wire order is valid.
    let tool_at = assembled
        .messages
        .iter()
        .position(|m| m.role == MessageRole::Tool)
        .unwrap();
    let last_user = assembled
        .messages
        .iter()
        .rposition(|m| m.role == MessageRole::User)
        .unwrap();
    assert!(tool_at < last_user);
}

// A fully answered turn gains nothing.
#[test]
fn a_complete_turn_is_unchanged() {
    let records = vec![
        Record::Assistant {
            v: FORMAT_VERSION,
            ts: 1,
            id: "a1".into(),
            content: Vec::new(),
            reasoning: None,
            model: None,
            provider: None,
            usage: None,
        },
        Record::ToolCall {
            v: FORMAT_VERSION,
            ts: 2,
            id: "tc1".into(),
            call_id: "call_1".into(),
            name: "shell".into(),
            arguments: "{}".into(),
            source: ToolSource::Builtin,
        },
        Record::ToolResult {
            v: FORMAT_VERSION,
            ts: 3,
            id: "tr1".into(),
            call_id: "call_1".into(),
            status: lca_protocol::ToolResultStatus::Ok,
            content: Some("ok".into()),
            attachment: None,
            truncated: false,
            exit_code: None,
            nested: Vec::new(),
            full_output_path: None,
        },
    ];
    let assembled = assemble(&records, "system");
    let tools = assembled
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .count();
    assert_eq!(tools, 1, "no synthetic result for an answered call");
}
