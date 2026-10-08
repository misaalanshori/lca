//! The Responses engine's contract (gh #63, moved to the wire kit
//! gh #189): the request-body shape and the SSE event mapper — no
//! sockets, no network.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_wire_openai::responses::{ResponsesStream, build_responses_body};

// Verifies: gh #63 - the Responses body pins pi's shape: `store`
// false, instructions, input items, tools, and the effort mapping.
#[test]
fn the_responses_body_pins_pi_shape() {
    use lca_protocol::{CompletionRequest, ContentBlock, MessageRole, ToolSpec};
    let request = CompletionRequest {
        messages: vec![
            lca_protocol::ChatMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                tool_calls: Vec::new(),
                tool_call_id: None,
                usage: None,
                extras: Default::default(),
            },
            lca_protocol::ChatMessage {
                role: MessageRole::Tool,
                content: vec![ContentBlock::Text {
                    text: "out".to_string(),
                }],
                tool_calls: Vec::new(),
                tool_call_id: Some("c1".to_string()),
                usage: None,
                extras: Default::default(),
            },
        ],
        tools: vec![ToolSpec {
            name: "read".to_string(),
            description: "read".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        }],
        model: "m".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let body = build_responses_body(&request, "sys", "m", Some("minimal"));
    assert_eq!(body["store"], false);
    assert_eq!(body["model"], "m");
    assert_eq!(body["instructions"], "sys");
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(body["input"][1]["type"], "function_call_output");
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["reasoning"]["effort"], "low", "minimal maps to low");
}

// Verifies: gh #63 - the SSE mapper turns the Responses event shapes
// into typed events, closing calls on `.done` with whole arguments
// when no deltas came.
#[test]
fn the_sse_mapper_turns_responses_events_typed() {
    use lca_protocol::StreamEvent;
    let mut stream = ResponsesStream::new();
    let mut event = |payload: serde_json::Value| stream.feed(&payload);
    let added = serde_json::json!({
        "type": "response.output_item.added",
        "output_index": 0,
        "item": {"type": "function_call", "call_id": "c1", "name": "read"},
    });
    assert_eq!(
        event(added),
        vec![StreamEvent::ToolCallStart {
            call_id: "c1".to_string(),
            name: "read".to_string(),
        }]
    );
    let delta = serde_json::json!({
        "type": "response.function_call_arguments.delta",
        "output_index": 0,
        "delta": "{\"path\":",
    });
    assert!(matches!(
        event(delta)[..],
        [StreamEvent::ToolCallArgDelta { .. }]
    ));
    let done = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "output_index": 0,
        "arguments": "{\"path\":\"a.txt\"}",
    });
    let events = event(done);
    assert!(
        events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolCallArgDelta { delta, .. } if delta == "\"a.txt\"}"
        )),
        "the suffix past the fragments streams: {events:?}"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolCallEnd { call_id } if call_id == "c1"
    )));
    let completed = serde_json::json!({
        "type": "response.completed",
        "response": {"usage": {"input_tokens": 10, "output_tokens": 3}},
    });
    assert!(matches!(event(completed)[..], [StreamEvent::Usage { .. }]));
}

// Verifies: gh #202 acceptance 2 - a Responses `response.failed` naming
// capacity surfaces as a retryable error.
#[test]
fn a_failed_response_naming_capacity_is_retryable() {
    use lca_protocol::StreamEvent;
    let mut stream = ResponsesStream::new();
    let events = stream.feed(&serde_json::json!({
        "type": "response.failed",
        "error": {"message": "Selected model is at capacity"},
    }));
    let (message, retryable) = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::Error { message, retryable } => Some((message.clone(), *retryable)),
            _ => None,
        })
        .expect("gh #202: the failed response is surfaced");
    assert!(
        message.contains("at capacity"),
        "the vendor message survives: {message}"
    );
    assert!(retryable, "capacity clears itself on retry");
}
