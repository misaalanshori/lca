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

// Verifies: gh #113 - a tool result carrying images serializes them
// natively: the `tool_result` content becomes the text+image block
// array Anthropic accepts. Text-only results keep the string form.
#[test]
fn a_tool_image_serializes_inside_the_tool_result() {
    let mut request = user_request();
    request.messages.push(lca_protocol::ChatMessage {
        role: MessageRole::Tool,
        content: vec![
            ContentBlock::Text {
                text: "seen".to_string(),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                bytes: vec![1, 2, 3],
            },
        ],
        tool_calls: Vec::new(),
        tool_call_id: Some("call-9".to_string()),
        usage: None,
        extras: Default::default(),
    });
    let body = build_messages_body(&request, "sys", "m", 1024, false);
    let tool = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| {
            message["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
        })
        .expect("tool result rides a user message");
    let content = tool["content"][0]["content"]
        .as_array()
        .expect("block array");
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(
        content[1]["source"]["data"], "AQID",
        "base64 bytes ride the image block"
    );
}

// ---------------------------------------------------------------------
// gh #334: prompt-cache breakpoints are pi's three FIXED anchors —
// system, last tool declaration, and the final block of the LAST
// message. Anthropic caps a request at four; the old code planted one on
// every user message in history, so a 3rd user turn already broke the cap.
// ---------------------------------------------------------------------

/// Every `cache_control` in the body, as a path string, so a test can
/// assert both the count and *where* they landed.
fn breakpoint_paths(body: &serde_json::Value) -> Vec<String> {
    let mut found = Vec::new();
    if body["system"][0].get("cache_control").is_some() {
        found.push("system".to_string());
    }
    if let Some(tools) = body["tools"].as_array() {
        for (index, tool) in tools.iter().enumerate() {
            if tool.get("cache_control").is_some() {
                found.push(format!("tools[{index}]"));
            }
        }
    }
    if let Some(messages) = body["messages"].as_array() {
        for (mi, message) in messages.iter().enumerate() {
            if let Some(blocks) = message["content"].as_array() {
                for (bi, block) in blocks.iter().enumerate() {
                    if block.get("cache_control").is_some() {
                        found.push(format!("messages[{mi}].content[{bi}]"));
                    }
                }
            }
        }
    }
    found
}

/// A user text turn.
fn user_turn(text: &str) -> lca_protocol::ChatMessage {
    lca_protocol::ChatMessage {
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

/// An assistant text turn.
fn assistant_turn(text: &str) -> lca_protocol::ChatMessage {
    lca_protocol::ChatMessage {
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

// Verifies: gh #334 AC1 - a ten-turn history still serializes exactly
// three breakpoints, and they are the three fixed anchors (system, the last
// tool declaration, the final block of the final message) - not one per
// user turn.
#[test]
fn a_ten_turn_history_keeps_exactly_three_breakpoints() {
    let mut request = user_request();
    request.tools = vec![ToolSpec {
        name: "a".to_string(),
        description: "a".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    }];
    request.messages.clear();
    for turn in 0..5 {
        request.messages.push(user_turn(&format!("u{turn}")));
        request.messages.push(assistant_turn(&format!("a{turn}")));
    }
    request.messages.push(user_turn("final"));

    let body = build_messages_body(&request, "sys", "m", 1024, true);
    let paths = breakpoint_paths(&body);
    assert_eq!(
        paths.len(),
        3,
        "three anchors, never one per turn: {paths:?} (Anthropic caps at 4)"
    );
    assert_eq!(paths, vec!["system", "tools[0]", "messages[10].content[0]"]);
}

// Verifies: gh #334 AC2 - a request ending in a tool_result still gets the
// final breakpoint. The Tool arm serializes as `role: user` + a
// `tool_result` block, and the old code only planted a breakpoint inside
// the User arm, so a tool-final request had NO tail breakpoint at all.
#[test]
fn a_tool_final_request_gets_the_final_breakpoint() {
    let mut request = user_request();
    request.messages.clear();
    request.messages.push(user_turn("read something"));
    request.messages.push(lca_protocol::ChatMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::Text {
            text: "reading".to_string(),
        }],
        tool_calls: vec![lca_protocol::ToolCall {
            call_id: "c1".to_string(),
            name: "read".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        }],
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    });
    request.messages.push(lca_protocol::ChatMessage {
        role: MessageRole::Tool,
        content: vec![ContentBlock::Text {
            text: "file contents".to_string(),
        }],
        tool_calls: Vec::new(),
        tool_call_id: Some("c1".to_string()),
        usage: None,
        extras: Default::default(),
    });

    let body = build_messages_body(&request, "sys", "m", 1024, true);
    let paths = breakpoint_paths(&body);
    // The tool result is the LAST serialized message, so its block carries
    // the third anchor.
    assert!(
        paths.iter().any(|path| path == "messages[2].content[0]"),
        "the tool_result block carries the final breakpoint: {paths:?}"
    );
    assert_eq!(paths.len(), 2, "system + the final block: {paths:?}");
}

// Verifies: gh #334 AC3 - the minimal shapes never approach the cap: a
// single user message gives two (system + message), and an empty message
// list gives one, and no tools means no tool anchor.
#[test]
fn minimal_shapes_stay_far_below_the_cap() {
    let body = build_messages_body(&user_request(), "sys", "m", 1024, true);
    assert_eq!(
        breakpoint_paths(&body),
        vec!["system", "messages[0].content[0]"]
    );

    let mut empty = user_request();
    empty.messages.clear();
    let body = build_messages_body(&empty, "sys", "m", 1024, true);
    assert_eq!(
        breakpoint_paths(&body),
        vec!["system"],
        "no messages, no tail"
    );

    // Opting out carries none at all.
    let body = build_messages_body(&user_request(), "sys", "m", 1024, false);
    assert!(
        breakpoint_paths(&body).is_empty(),
        "opting out plants nothing"
    );
}

// ---------------------------------------------------------------------
// gh #335: Anthropic requires `thinking`/`redacted_thinking` blocks to
// precede every other assistant content block. The old `content_blocks`
// pushed the merged text block first.
// ---------------------------------------------------------------------

/// Assistant content with reasoning first, then text, then a tool call:
/// the shape a replaying thinking turn produces.
fn thinking_turn() -> lca_protocol::ChatMessage {
    lca_protocol::ChatMessage {
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
    }
}

// Verifies: gh #335 AC1 - thinking lands at block index 0 with its
// signature, and the text follows. Before the fix the text was index 0 and
// Anthropic rejected the replay.
#[test]
fn thinking_precedes_text_in_an_assistant_replay() {
    let mut request = user_request();
    request.messages.push(thinking_turn());
    let body = build_messages_body(&request, "", "m", 1024, false);
    let blocks = body["messages"][1]["content"].as_array().expect("blocks");
    assert_eq!(
        blocks[0]["type"], "thinking",
        "thinking is first: {blocks:?}"
    );
    assert_eq!(blocks[0]["thinking"], "because");
    assert_eq!(blocks[0]["signature"], "sig-bytes");
    assert_eq!(blocks[1]["type"], "text", "text follows: {blocks:?}");
    assert_eq!(blocks[1]["text"], "done");
}

// Verifies: gh #335 AC2 - mixed thinking/text/tool_use keeps tool_use
// last: thinking, text, tool_use.
#[test]
fn thinking_precedes_text_and_tool_use() {
    let mut request = user_request();
    let mut turn = thinking_turn();
    turn.tool_calls.push(lca_protocol::ToolCall {
        call_id: "c1".to_string(),
        name: "read".to_string(),
        arguments: "{}".to_string(),
        parent_call_id: None,
    });
    request.messages.push(turn);
    let body = build_messages_body(&request, "", "m", 1024, false);
    let blocks = body["messages"][1]["content"].as_array().expect("blocks");
    let kinds: Vec<&str> = blocks
        .iter()
        .map(|b| b["type"].as_str().unwrap_or("?"))
        .collect();
    assert_eq!(kinds, vec!["thinking", "text", "tool_use"], "{blocks:?}");
}

// Verifies: gh #335 - `redacted_thinking` obeys the same ordering rule
// (it is a thinking block), and keeps its signature as `data`.
#[test]
fn redacted_thinking_also_precedes_text() {
    let mut request = user_request();
    request.messages.push(lca_protocol::ChatMessage {
        role: MessageRole::Assistant,
        content: vec![
            ContentBlock::Reasoning {
                reasoning: String::new(),
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
    assert_eq!(blocks[0]["type"], "redacted_thinking", "{blocks:?}");
    assert_eq!(blocks[0]["data"], "sig-bytes");
    assert_eq!(blocks[1]["type"], "text");
}

// Verifies: gh #335 AC3 - the shapes that have only one kind are
// unchanged: text-only stays a single text block, thinking-only stays a
// single thinking block.
#[test]
fn single_kind_shapes_are_unchanged() {
    let mut request = user_request();
    request.messages.push(assistant_turn("just text"));
    let body = build_messages_body(&request, "", "m", 1024, false);
    let blocks = body["messages"][1]["content"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0]["type"], "text");

    let mut request = user_request();
    request.messages.push(lca_protocol::ChatMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::Reasoning {
            reasoning: "only thinking".to_string(),
            signature: Some("sig".to_string()),
        }],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    });
    let body = build_messages_body(&request, "", "m", 1024, false);
    let blocks = body["messages"][1]["content"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0]["type"], "thinking");
}
