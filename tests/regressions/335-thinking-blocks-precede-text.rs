//! GitHub #335: `content_blocks` merged assistant text and pushed it at
//! **index 0**, then appended the `thinking` / `redacted_thinking` blocks.
//! Anthropic requires thinking blocks to precede every other assistant
//! content block, so replaying a thinking turn failed with
//! `invalid_request_error: thinking blocks must precede all other content
//! blocks in assistant messages` — turn 2 of any thinking conversation.
//!
//! Verifies: the outbound request JSON — thinking first with its signature
//! verbatim, merged text next, `tool_use` last, for both the signed
//! `thinking` and the `redacted_thinking` shapes, with the single-kind
//! shapes unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::{ChatMessage, ContentBlock, MessageRole, ToolCall};
use lca_wire_anthropic::build_messages_body;

fn request(messages: Vec<ChatMessage>) -> lca_protocol::CompletionRequest {
    lca_protocol::CompletionRequest {
        messages,
        tools: Vec::new(),
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

fn assistant(content: Vec<ContentBlock>, tool_calls: Vec<ToolCall>) -> ChatMessage {
    ChatMessage {
        role: MessageRole::Assistant,
        content,
        tool_calls,
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    }
}

/// The block `type`s of the assistant message's serialized content.
fn kinds(body: &serde_json::Value) -> Vec<String> {
    body["messages"][1]["content"]
        .as_array()
        .expect("assistant blocks")
        .iter()
        .map(|block| block["type"].as_str().unwrap_or("?").to_string())
        .collect()
}

// Verifies: #335 AC1 - `thinking` is block 0 with its signature, text
// follows. Before: `["text", "thinking"]`, which Anthropic rejects.
#[test]
fn thinking_is_the_first_block_on_replay() {
    let body = build_messages_body(
        &request(vec![
            user("hi"),
            assistant(
                vec![
                    ContentBlock::Reasoning {
                        reasoning: "because".to_string(),
                        signature: Some("sig-bytes".to_string()),
                    },
                    ContentBlock::Text {
                        text: "done".to_string(),
                    },
                ],
                Vec::new(),
            ),
        ]),
        "",
        "m",
        1024,
        false,
    );
    assert_eq!(kinds(&body), vec!["thinking", "text"], "{body}");
    assert_eq!(body["messages"][1]["content"][0]["thinking"], "because");
    assert_eq!(
        body["messages"][1]["content"][0]["signature"], "sig-bytes",
        "the signature travels verbatim"
    );
}

// Verifies: #335 AC2 - with a tool call the order is thinking, text,
// tool_use. Before: text came first, so every tool round in a thinking
// session was rejected.
#[test]
fn thinking_precedes_text_and_tool_use() {
    let body = build_messages_body(
        &request(vec![
            user("hi"),
            assistant(
                vec![
                    ContentBlock::Reasoning {
                        reasoning: "plan".to_string(),
                        signature: Some("sig".to_string()),
                    },
                    ContentBlock::Text {
                        text: "calling".to_string(),
                    },
                ],
                vec![ToolCall {
                    call_id: "c1".to_string(),
                    name: "read".to_string(),
                    arguments: "{}".to_string(),
                    parent_call_id: None,
                }],
            ),
        ]),
        "",
        "m",
        1024,
        false,
    );
    assert_eq!(kinds(&body), vec!["thinking", "text", "tool_use"], "{body}");
}

// Verifies: #335 - `redacted_thinking` obeys the same ordering rule and
// keeps its signature as `data`.
#[test]
fn redacted_thinking_is_also_first() {
    let body = build_messages_body(
        &request(vec![
            user("hi"),
            assistant(
                vec![
                    ContentBlock::Reasoning {
                        reasoning: String::new(),
                        signature: Some("sig-bytes".to_string()),
                    },
                    ContentBlock::Text {
                        text: "done".to_string(),
                    },
                ],
                Vec::new(),
            ),
        ]),
        "",
        "m",
        1024,
        false,
    );
    assert_eq!(kinds(&body), vec!["redacted_thinking", "text"], "{body}");
    assert_eq!(body["messages"][1]["content"][0]["data"], "sig-bytes");
}

// Verifies: #335 AC3 - the single-kind shapes are untouched.
#[test]
fn single_kind_shapes_are_unchanged() {
    let body = build_messages_body(
        &request(vec![
            user("hi"),
            assistant(
                vec![ContentBlock::Text {
                    text: "just text".to_string(),
                }],
                Vec::new(),
            ),
        ]),
        "",
        "m",
        1024,
        false,
    );
    assert_eq!(kinds(&body), vec!["text"], "{body}");

    let body = build_messages_body(
        &request(vec![
            user("hi"),
            assistant(
                vec![ContentBlock::Reasoning {
                    reasoning: "only thinking".to_string(),
                    signature: Some("sig".to_string()),
                }],
                Vec::new(),
            ),
        ]),
        "",
        "m",
        1024,
        false,
    );
    assert_eq!(kinds(&body), vec!["thinking"], "{body}");
}
