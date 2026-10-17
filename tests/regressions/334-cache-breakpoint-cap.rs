//! GitHub #334: `build_messages_body` planted a `cache_control` breakpoint
//! on the last block of **every user message in history**, so a session
//! with three user turns emitted five breakpoints (system + tools + three
//! messages). Anthropic caps a request at four and rejects the rest with
//! `invalid_request_error: At most 4 cache_control breakpoints may be
//! specified`. The same arm only matched `MessageRole::User`, so a request
//! ending in a tool result got **no** final breakpoint at all.
//!
//! Verifies: the outbound request JSON, which IS the observable contract —
//! exactly three breakpoints on pi's fixed anchors (system, the last tool
//! declaration, the final block of the final message), at any history
//! length, including a tool-result final.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::{ChatMessage, ContentBlock, MessageRole, ToolCall, ToolSpec};
use lca_wire_anthropic::build_messages_body;

fn request(messages: Vec<ChatMessage>, tools: Vec<ToolSpec>) -> lca_protocol::CompletionRequest {
    lca_protocol::CompletionRequest {
        messages,
        tools,
        model: "claude-test".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

fn user(text: &str) -> ChatMessage {
    ChatMessage {
        role: MessageRole::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    }
}

fn assistant(text: &str) -> ChatMessage {
    ChatMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    }
}

fn tool() -> ToolSpec {
    ToolSpec {
        name: "read".to_string(),
        description: "read a file".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    }
}

/// Count every `cache_control` key anywhere under `value`.
fn count_breakpoints(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(map) => {
            map.keys()
                .filter(|key| key.as_str() == "cache_control")
                .count()
                + map.values().map(count_breakpoints).sum::<usize>()
        }
        serde_json::Value::Array(items) => items.iter().map(count_breakpoints).sum(),
        _ => 0,
    }
}

/// `true` when the block at the end of the last message carries a marker.
fn last_block_marked(body: &serde_json::Value) -> bool {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_array())
        .and_then(|blocks| blocks.last())
        .is_some_and(|block| block.get("cache_control").is_some())
}

// Verifies: #334 AC1 - a long history stays at three, never growing with
// the number of turns. Before the fix a 5-turn history emitted 7
// (system + tools + 5).
#[test]
fn a_long_history_never_exceeds_three_breakpoints() {
    let mut messages = Vec::new();
    for turn in 0..10 {
        messages.push(user(&format!("u{turn}")));
        messages.push(assistant(&format!("a{turn}")));
    }
    messages.push(user("final"));
    let body = build_messages_body(&request(messages, vec![tool()]), "sys", "m", 1024, true);

    assert_eq!(
        count_breakpoints(&body),
        3,
        "system + last tool + final block, and Anthropic caps at 4: {body}"
    );
    assert!(
        body["system"][0].get("cache_control").is_some(),
        "system anchor"
    );
    assert!(
        body["tools"][0].get("cache_control").is_some(),
        "last tool anchor"
    );
    assert!(last_block_marked(&body), "final-block anchor");
}

// Verifies: #334 AC2 - a tool-final request gets the tail breakpoint on
// its `tool_result` block. Before the fix the Tool arm never reached the
// breakpoint code, so the tail was silently uncached.
#[test]
fn a_tool_final_request_carries_the_tail_breakpoint() {
    let mut messages = vec![user("read it")];
    messages.push(ChatMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::Text {
            text: "reading".to_string(),
        }],
        tool_calls: vec![ToolCall {
            call_id: "c1".to_string(),
            name: "read".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        }],
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    });
    messages.push(ChatMessage {
        role: MessageRole::Tool,
        content: vec![ContentBlock::Text {
            text: "contents".to_string(),
        }],
        tool_calls: Vec::new(),
        tool_call_id: Some("c1".to_string()),
        usage: None,
        extras: Default::default(),
    });
    let body = build_messages_body(&request(messages, Vec::new()), "sys", "m", 1024, true);

    let last = body["messages"]
        .as_array()
        .expect("messages")
        .last()
        .expect("a final message");
    assert_eq!(last["role"], "user", "a tool result rides the user role");
    assert_eq!(
        last["content"][0]["type"], "tool_result",
        "the tail anchor lands on the tool_result block"
    );
    assert!(
        last["content"][0].get("cache_control").is_some(),
        "the tool_result carries the marker: {last}"
    );
    assert_eq!(count_breakpoints(&body), 2, "system + the tail");
}

// Verifies: #334 AC3 - minimal shapes stay under the cap, and opting out
// plants nothing.
#[test]
fn minimal_shapes_and_opt_out_stay_clean() {
    let body = build_messages_body(
        &request(vec![user("hi")], Vec::new()),
        "sys",
        "m",
        1024,
        true,
    );
    assert_eq!(count_breakpoints(&body), 2, "system + one message");

    let body = build_messages_body(&request(Vec::new(), Vec::new()), "sys", "m", 1024, true);
    assert_eq!(count_breakpoints(&body), 1, "system only");

    let body = build_messages_body(
        &request(vec![user("hi")], vec![tool()]),
        "sys",
        "m",
        1024,
        false,
    );
    assert_eq!(count_breakpoints(&body), 0, "opting out plants nothing");
}
