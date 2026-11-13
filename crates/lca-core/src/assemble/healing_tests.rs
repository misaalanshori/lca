use super::*;
use lca_protocol::ToolCall;

fn assistant_with_call(call_id: &str) -> ChatMessage {
    let mut message = ChatMessage::text(MessageRole::Assistant, "working");
    message.tool_calls.push(ToolCall {
        call_id: call_id.to_string(),
        name: "shell".to_string(),
        arguments: "{}".to_string(),
        parent_call_id: None,
    });
    message.content.push(ContentBlock::ToolCall {
        call_id: call_id.to_string(),
        name: "shell".to_string(),
        arguments: "{}".to_string(),
    });
    message
}

// A dangling call (the iteration-limit / cancel shape) gains a tool
// result, so the next request is valid.
#[test]
fn a_dangling_tool_call_is_healed() {
    let mut messages = vec![
        ChatMessage::text(MessageRole::User, "hi"),
        assistant_with_call("call_1"),
        ChatMessage::text(MessageRole::User, "next"),
    ];
    heal_dangling_tool_calls(&mut messages);
    let tool = messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("a synthetic tool result");
    assert_eq!(tool.tool_call_id.as_deref(), Some("call_1"));
    // It sits before the next user message.
    let tool_at = messages
        .iter()
        .position(|m| m.role == MessageRole::Tool)
        .unwrap();
    let user_at = messages
        .iter()
        .rposition(|m| m.role == MessageRole::User)
        .unwrap();
    assert!(tool_at < user_at);
}

// A normal turn is untouched (no extra messages, order preserved).
#[test]
fn answered_calls_are_not_touched() {
    let mut messages = vec![
        assistant_with_call("call_1"),
        ChatMessage::tool_result("call_1", "ok"),
        ChatMessage::text(MessageRole::Assistant, "done"),
    ];
    let before = messages.len();
    heal_dangling_tool_calls(&mut messages);
    assert_eq!(messages.len(), before);
    assert_eq!(messages[1].role, MessageRole::Tool);
}

// One of two calls answered: only the missing one is filled.
#[test]
fn only_the_missing_call_is_filled() {
    let mut messages = vec![
        assistant_with_call("call_1"),
        ChatMessage::tool_result("call_1", "ok"),
    ];
    messages[0].tool_calls.push(lca_protocol::ToolCall {
        call_id: "call_2".to_string(),
        name: "shell".to_string(),
        arguments: "{}".to_string(),
        parent_call_id: None,
    });
    heal_dangling_tool_calls(&mut messages);
    let ids: Vec<Option<&str>> = messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .map(|m| m.tool_call_id.as_deref())
        .collect();
    assert_eq!(ids, vec![Some("call_1"), Some("call_2")]);
}
