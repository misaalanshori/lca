//! The Messages engine's contract (gh #189): request-body shape,
//! the SSE event grammar, thinking signatures, and usage buckets —
//! no sockets, no network. Fixtures follow pi's
//! `anthropic-messages.ts` event table.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_protocol::{CompletionRequest, ContentBlock, MessageRole, StreamEvent, ToolSpec};
use lca_wire_anthropic::messages::{AnthropicStream, build_messages_body};

fn user_request() -> CompletionRequest {
    CompletionRequest {
        messages: vec![lca_protocol::ChatMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        }],
        tools: Vec::new(),
        model: "m".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

#[test]
fn the_body_pins_the_messages_shape() {
    let body = build_messages_body(&user_request(), "sys", "m", 1024, false);
    assert_eq!(body["model"], "m");
    assert_eq!(body["max_tokens"], 1024);
    assert_eq!(body["stream"], true);
    assert_eq!(body["system"], "sys");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["type"], "text");
    assert!(body.get("tools").is_none(), "no tools means no tools key");
}

#[test]
fn breakpoints_land_on_system_last_block_and_last_tool() {
    let mut request = user_request();
    request.tools = vec![
        ToolSpec {
            name: "a".to_string(),
            description: "a".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        },
        ToolSpec {
            name: "b".to_string(),
            description: "b".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        },
    ];
    let body = build_messages_body(&request, "sys", "m", 1024, true);
    assert_eq!(
        body["system"][0]["cache_control"]["type"], "ephemeral",
        "system carries a breakpoint"
    );
    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"]["type"], "ephemeral",
        "the last message block carries one"
    );
    assert!(
        body["tools"][0].get("cache_control").is_none(),
        "only the last tool carries one"
    );
    assert_eq!(
        body["tools"][1]["cache_control"]["type"], "ephemeral",
        "pi's last-tool rule"
    );
    assert_eq!(body["tools"][1]["input_schema"]["type"], "object");
}

// Verifies: gh #189 - the decoder turns the Messages event shapes
// into typed events: text deltas stream, usage lands off both start
// and delta, and the stop closes the stream without inventing errors.
#[test]
fn a_text_stream_decodes_to_deltas_and_usage() {
    let frames = [
        serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 8, "output_tokens": 0}},
        }),
        serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""},
        }),
        serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": "Hello"},
        }),
        serde_json::json!({"type": "content_block_stop", "index": 0}),
        serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 5},
        }),
        serde_json::json!({"type": "message_stop"}),
    ];
    let mut stream = AnthropicStream::new();
    let events: Vec<StreamEvent> = frames.iter().flat_map(|frame| stream.feed(frame)).collect();
    assert!(
        events.iter().any(|event| matches!(
            event,
            StreamEvent::TextDelta { delta } if delta == "Hello"
        )),
        "text streams: {events:?}"
    );
    let usages: Vec<&lca_protocol::Usage> = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::Usage { usage } => Some(usage),
            _ => None,
        })
        .collect();
    assert_eq!(usages.len(), 2, "start and delta both report: {events:?}");
    assert_eq!(usages[0].input, 8);
    assert_eq!(usages[1].output, 5);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, StreamEvent::Error { .. })),
        "a clean stop invents nothing: {events:?}"
    );
}

// Verifies: gh #189 - a tool_use block opens the call, the
// input_json fragments ride arg deltas, and the block stop closes it
// (FR-PROV-7 ordering on a second wire family).
#[test]
fn a_tool_use_block_assembles_its_arguments() {
    let frames = [
        serde_json::json!({
            "type": "content_block_start", "index": 1,
            "content_block": {"type": "tool_use", "id": "t1", "name": "read"},
        }),
        serde_json::json!({
            "type": "content_block_delta", "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"path\":"},
        }),
        serde_json::json!({
            "type": "content_block_delta", "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "\"a\"}"},
        }),
        serde_json::json!({"type": "content_block_stop", "index": 1}),
    ];
    let mut stream = AnthropicStream::new();
    let events: Vec<StreamEvent> = frames.iter().flat_map(|frame| stream.feed(frame)).collect();
    let order: Vec<&str> = events
        .iter()
        .map(|event| match event {
            StreamEvent::ToolCallStart { .. } => "start",
            StreamEvent::ToolCallArgDelta { .. } => "delta",
            StreamEvent::ToolCallEnd { .. } => "end",
            _ => "other",
        })
        .collect();
    assert_eq!(order, vec!["start", "delta", "delta", "end"]);
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::ToolCallArgDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect::<String>(),
        "{\"path\":\"a\"}"
    );
}

// Verifies: gh #189 - thinking streams as reasoning, and the signature
// (start plus streamed fragments) emits as one vendor event when the
// block closes, so #41 has the bytes for multi-turn continuity.
#[test]
fn thinking_keeps_its_signature_for_continuity() {
    let frames = [
        serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "thinking", "thinking": "hmm", "signature": "sig-"},
        }),
        serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "more"},
        }),
        serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "signature_delta", "signature": "1"},
        }),
        serde_json::json!({"type": "content_block_stop", "index": 0}),
    ];
    let mut stream = AnthropicStream::new();
    let events: Vec<StreamEvent> = frames.iter().flat_map(|frame| stream.feed(frame)).collect();
    let reasoning: String = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ReasoningDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning, "hmmmore");
    let signature = events.iter().find_map(|event| match event {
        StreamEvent::VendorEvent { kind, payload } if kind == "thinking-signature" => {
            payload.get("signature").and_then(|s| s.as_str())
        }
        _ => None,
    });
    assert_eq!(signature, Some("sig-1"), "fragments assemble: {events:?}");
}

// Verifies: gh #189 - the cache buckets map off message_start,
// including the 1h write bucket pi reads.
#[test]
fn usage_carries_both_cache_buckets() {
    let mut stream = AnthropicStream::new();
    let events = stream.feed(&serde_json::json!({
        "type": "message_start",
        "message": {"usage": {
            "input_tokens": 100,
            "output_tokens": 10,
            "cache_read_input_tokens": 60,
            "cache_creation_input_tokens": 20,
            "cache_creation": {"ephemeral_1h_input_tokens": 7},
        }},
    }));
    let usage = events.iter().find_map(|event| match event {
        StreamEvent::Usage { usage } => Some(usage),
        _ => None,
    });
    let usage = usage.expect("usage event");
    assert_eq!(usage.input, 100);
    assert_eq!(usage.cache_read, 60);
    assert_eq!(usage.cache_write, 20);
    assert_eq!(usage.cache_write_1h, 7);
}

// Verifies: gh #189 - a tool_result answers the tool_use id on the
// user role, and an assistant turn replays stored calls as tool_use
// blocks (the multi-turn round-trip).
#[test]
fn tool_results_round_trip_on_the_user_role() {
    use lca_protocol::ChatMessage;
    let request = CompletionRequest {
        messages: vec![
            ChatMessage {
                role: MessageRole::Assistant,
                content: Vec::new(),
                tool_calls: vec![lca_protocol::ToolCall {
                    call_id: "t1".to_string(),
                    name: "read".to_string(),
                    arguments: "{\"path\":\"a\"}".to_string(),
                    parent_call_id: None,
                }],
                tool_call_id: None,
                usage: None,
                extras: Default::default(),
            },
            ChatMessage {
                role: MessageRole::Tool,
                content: vec![ContentBlock::Text {
                    text: "contents".to_string(),
                }],
                tool_calls: Vec::new(),
                tool_call_id: Some("t1".to_string()),
                usage: None,
                extras: Default::default(),
            },
        ],
        tools: Vec::new(),
        model: "m".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let body = build_messages_body(&request, "", "m", 1024, false);
    assert_eq!(body["messages"][0]["content"][0]["type"], "tool_use");
    assert_eq!(body["messages"][0]["content"][0]["id"], "t1");
    assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
    assert_eq!(
        body["messages"][1]["content"][0]["tool_use_id"], "t1",
        "the result answers the call"
    );
}

// Verifies: gh #202 acceptance 2 - an Anthropic `error` event naming an
// overloaded model surfaces as a retryable error.
#[test]
fn an_overloaded_error_event_is_retryable() {
    let mut stream = AnthropicStream::new();
    let events = stream.feed(&serde_json::json!({
        "type": "error",
        "error": {"type": "overloaded_error", "message": "Overloaded"},
    }));
    let (message, retryable) = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::Error { message, retryable } => Some((message.clone(), *retryable)),
            _ => None,
        })
        .expect("gh #202: the error event is surfaced");
    assert!(
        message.contains("Overloaded"),
        "the vendor message survives: {message}"
    );
    assert!(retryable, "overload clears itself on retry");
}

// Verifies: gh #41 (replay resends thinking verbatim with its
// signature); unsigned reasoning never returns to the wire.
#[test]
fn replay_resends_thinking_with_its_signature() {
    let mut request = user_request();
    request.messages.push(lca_protocol::ChatMessage {
        role: MessageRole::Assistant,
        content: vec![
            ContentBlock::Reasoning {
                reasoning: "because".to_string(),
                signature: Some("sig-bytes".to_string()),
            },
            ContentBlock::Text {
                text: "done".to_string(),
            },
        ],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    });
    let body = build_messages_body(&request, "", "m", 1024, false);
    let blocks = body["messages"][1]["content"].as_array().expect("blocks");
    let thinking = blocks
        .iter()
        .find(|block| block["type"] == "thinking")
        .expect("the thinking block is replayed");
    assert_eq!(thinking["thinking"], "because");
    assert_eq!(thinking["signature"], "sig-bytes");
}

// Verifies: gh #41 (a signature over redacted text travels as
// `redacted_thinking`, pi's shape).
#[test]
fn redacted_thinking_keeps_its_signature() {
    let mut request = user_request();
    request.messages.push(lca_protocol::ChatMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::Reasoning {
            reasoning: String::new(),
            signature: Some("sig-bytes".to_string()),
        }],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    });
    let body = build_messages_body(&request, "", "m", 1024, false);
    let blocks = body["messages"][1]["content"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 1, "only the redacted block: {blocks:?}");
    assert_eq!(blocks[0]["type"], "redacted_thinking");
    assert_eq!(blocks[0]["data"], "sig-bytes");
}

// Verifies: gh #41 (the host's budget extra becomes the wire's
// `thinking.enabled`; the budget never eats the answer's room).
#[test]
fn the_budget_extra_becomes_thinking_enabled() {
    let mut request = user_request();
    request
        .extras
        .insert("thinking-budget-tokens".to_string(), "8192".to_string());
    let body = build_messages_body(&request, "", "m", 16384, false);
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 8192);

    // A small ceiling clamps the budget to its room (pi's reserve).
    let body = build_messages_body(&request, "", "m", 2048, false);
    assert_eq!(body["thinking"]["budget_tokens"], 1024);

    // No extra, no thinking key.
    let body = build_messages_body(&user_request(), "", "m", 16384, false);
    assert!(body.get("thinking").is_none());
}

// Verifies: gh #201 (the frozen list carries the placeholder and
// never changes when tools arrive mid-conversation).
#[test]
fn the_frozen_tool_list_survives_mid_conversation_changes() {
    let initial = vec![lca_protocol::ToolSpec {
        name: "read".to_string(),
        description: "read".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    }];
    let frozen = lca_wire_anthropic::frozen_tools(&initial);
    assert_eq!(
        frozen.last().expect("placeholder")["name"],
        "__pi_deferred_placeholder__"
    );
    assert_eq!(frozen.last().expect("placeholder")["defer_loading"], true);
    let snapshot = serde_json::to_string(&frozen).expect("snapshot");

    // Turn two: a tool arrives as a system block; the frozen list is
    // byte-for-byte the snapshot.
    let late = lca_protocol::ToolSpec {
        name: "mcp_jira_lookup".to_string(),
        description: "lookup".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    };
    let addition = lca_wire_anthropic::tool_addition_block(&late);
    assert_eq!(addition["type"], "tool_addition");
    assert_eq!(addition["tool"]["definition"]["name"], "mcp_jira_lookup");
    let removal = lca_wire_anthropic::tool_removal_block("mcp_jira_lookup");
    assert_eq!(removal["type"], "tool_removal");
    assert_eq!(removal["tool"]["name"], "mcp_jira_lookup");
    assert_eq!(
        serde_json::to_string(&frozen).expect("resnapshot"),
        snapshot,
        "the top-level list never moves"
    );
    assert_eq!(
        lca_wire_anthropic::INLINE_TOOLS_BETA,
        "inline-tools-2026-09-15"
    );
}

// Verifies: gh #201 riding gh #41 (a mid-conversation turn resends
// thinking signatures beside frozen tools: cache prefix and replay
// continuity together).
#[test]
fn signatures_replay_beside_frozen_tools() {
    let mut request = user_request();
    request.tools = vec![lca_protocol::ToolSpec {
        name: "read".to_string(),
        description: "read".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    }];
    // The extension (#183) sends the frozen list once; the kit pins
    // that freezing the list keeps every declaration verbatim.
    let frozen = lca_wire_anthropic::frozen_tools(&request.tools);
    assert_eq!(frozen[0]["name"], "read");
    request.messages.push(lca_protocol::ChatMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::Reasoning {
            reasoning: "because".to_string(),
            signature: Some("sig-bytes".to_string()),
        }],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    });
    let body = build_messages_body(&request, "", "m", 16384, false);
    let blocks = body["messages"][1]["content"].as_array().expect("blocks");
    assert!(
        blocks.iter().any(|block| block["signature"] == "sig-bytes"),
        "the signature replays: {blocks:?}"
    );
}
