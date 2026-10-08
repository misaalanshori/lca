//! Offline tests for the OpenAI streaming parser: no network, canned bytes
//! only (testing plan section 4).

use lca_protocol::StreamEvent;
use lca_wire_openai::{SseDecoder, classify_status, parse_sse};

fn events_of(body: &str) -> Vec<StreamEvent> {
    let mut sink = Vec::new();
    parse_sse(body.as_bytes(), &mut |event| sink.push(event));
    sink
}

#[test]
fn extracts_text_deltas() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\", world\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let text: String = events_of(body)
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello, world");
}

#[test]
fn opens_closes_tool_calls_and_carries_argument_fragments() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-7\",\"function\":{\"name\":\"read\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"a\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = events_of(body);
    let start = events.iter().find_map(|e| match e {
        StreamEvent::ToolCallStart { call_id, name } => Some((call_id.as_str(), name.as_str())),
        _ => None,
    });
    assert_eq!(start, Some(("call-7", "read")), "start precedes any delta");
    let order: Vec<&str> = events
        .iter()
        .map(|e| match e {
            StreamEvent::ToolCallStart { .. } => "start",
            StreamEvent::ToolCallArgDelta { .. } => "delta",
            StreamEvent::ToolCallEnd { .. } => "end",
            _ => "other",
        })
        .collect();
    assert_eq!(order, vec!["start", "delta", "end"], "FR-PROV-7 ordering");
}

#[test]
fn maps_usage_with_cache_fields_from_cached_tokens() {
    let body = concat!(
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1240,\"completion_tokens\":12,\"prompt_tokens_details\":{\"cached_tokens\":1200}}}\n\n",
        "data: [DONE]\n\n",
    );
    let usage = events_of(body)
        .iter()
        .find_map(|e| match e {
            StreamEvent::Usage { usage } => Some(usage.clone()),
            _ => None,
        })
        .expect("usage event");
    assert_eq!(usage.cache_read, 1200);
    assert_eq!(
        usage.input,
        1240 - 1200,
        "billed input excludes cache reads"
    );
    assert_eq!(usage.output, 12);
    assert_eq!(usage.prompt_tokens(), 1240, "prompt total stays stable");
}

#[test]
fn ignores_heartbeats_malformed_lines_and_vendor_noise() {
    let body = concat!(
        ": keep-alive\n\n",
        "data: {not json}\n\n",
        "data: {\"choices\":[{\"delta\":{\"legacy\":\"x\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"kept\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let text: String = events_of(body)
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "kept", "bad lines drop, good lines survive");
}

#[test]
fn server_errors_are_retryable_but_client_errors_are_not() {
    assert!(classify_status(500));
    assert!(classify_status(502));
    assert!(classify_status(429));
    assert!(!classify_status(401));
    assert!(!classify_status(400));
    assert!(!classify_status(404));
}

/// A `length` finish with no budget of ours on the request: chat turns
/// send no `max_tokens`, so there is no number of ours to name and the
/// stream stays as quiet as it was before gh #169.
fn length_body() -> &'static str {
    concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        "data: [DONE]\n\n",
    )
}

// Verifies gh #169 acceptance 3: a budgeted request (the completion
// capability's summarization) that ends on `finish_reason: length` was
// cut mid-generation - the failure says so, names the budget that cut
// it, and is not retryable (the same request would cap again).
#[test]
fn a_capped_generation_names_the_budget_that_cut_it() {
    let mut events = Vec::new();
    let mut decoder = SseDecoder::default().with_max_tokens(Some(4096));
    decoder.feed(length_body().as_bytes(), &mut |event| events.push(event));
    decoder.finish(&mut |event| events.push(event));
    let (message, retryable) = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::Error { message, retryable } => Some((message.clone(), *retryable)),
            _ => None,
        })
        .expect("gh #169: the cap is surfaced, not swallowed");
    assert!(
        message.contains("token cap") && message.contains("4096"),
        "the cap with its number: {message}"
    );
    assert!(!retryable, "a cap does not clear itself on retry");
}

// Verifies the scope half of gh #169: without a budget on the request
// (an ordinary chat turn), a `length` finish stays exactly what it was -
// no error of ours is invented where no number of ours exists.
#[test]
fn a_length_finish_stays_quiet_when_no_budget_was_set() {
    let events = events_of(length_body());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, StreamEvent::Error { .. })),
        "unbudgeted turns keep the old behavior: {events:?}"
    );
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "partial");
}

use lca_protocol::{ChatMessage, ContentBlock, MessageRole, StreamEvent as E2};
use lca_wire_openai::to_wire;

// Verifies: ADR-0029 - a message with an image maps to the OpenAI
// content-part array with a base64 data URI; a text-only message keeps
// the plain string content (no behavior change for the common case).
// Moved with the mapper from `openai-compatible` (gh #189).
#[test]
fn an_image_maps_to_the_vision_content_array() {
    let message = ChatMessage {
        role: MessageRole::User,
        content: vec![
            ContentBlock::Text {
                text: "look".to_string(),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                bytes: vec![1, 2, 3],
            },
        ],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    };
    let wire = to_wire(&[message]);
    let parts = wire[0]["content"].as_array().expect("array content");
    assert_eq!(parts[0]["type"], "text");
    assert_eq!(parts[1]["type"], "image_url");
    assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,AQID");

    let text_only = to_wire(&[ChatMessage::text(MessageRole::User, "hi")]);
    assert_eq!(
        text_only[0]["content"], "hi",
        "no image keeps the string form"
    );
}

#[test]
fn a_body_with_no_sse_frames_reports_an_error() {
    let mut events = Vec::new();
    lca_wire_openai::parse_sse(b"this is not SSE at all\n", &mut |event| events.push(event));
    assert!(
        events.iter().any(|event| matches!(event, E2::Error { .. })),
        "a non-SSE body must surface, not read as an empty success: {events:?}"
    );
}

#[test]
fn a_valid_stream_with_no_content_is_not_an_error() {
    let mut events = Vec::new();
    lca_wire_openai::parse_sse(
        b"data: {\"choices\":[{\"delta\":{}}]}\n\ndata: [DONE]\n\n",
        &mut |event| events.push(event),
    );
    assert!(
        !events.iter().any(|event| matches!(event, E2::Error { .. })),
        "an empty but well-formed stream is a valid empty answer: {events:?}"
    );
}

// Verifies: gh #202 acceptance 1 - HTTP 529 (and its 503/504 siblings)
// classify retryable, pinning the status half of the capacity spec.
#[test]
fn capacity_statuses_are_retryable() {
    assert!(classify_status(529), "HTTP 529 Overloaded retries");
    assert!(classify_status(503), "HTTP 503 retries");
    assert!(classify_status(504), "HTTP 504 retries");
}

// Verifies: gh #202 acceptance 2 - a mid-stream OpenAI error payload
// naming overload surfaces as a retryable error, not a silent drop.
#[test]
fn a_mid_stream_overload_error_is_retryable() {
    let body = "data: {\"error\":{\"message\":\"The engine is currently overloaded, please try again later.\",\"type\":\"server_error\"}}\n\n";
    let (message, retryable) = events_of(body)
        .iter()
        .find_map(|event| match event {
            StreamEvent::Error { message, retryable } => Some((message.clone(), *retryable)),
            _ => None,
        })
        .expect("gh #202: the mid-stream error is surfaced, not swallowed");
    assert!(
        message.contains("overloaded"),
        "the vendor message survives: {message}"
    );
    assert!(retryable, "overload clears itself on retry");
}

// Verifies: gh #202 - a mid-stream error naming nothing capacity-like
// stays non-retryable (the override is capacity-only).
#[test]
fn a_mid_stream_auth_error_stays_non_retryable() {
    let body = "data: {\"error\":{\"message\":\"Invalid API key provided\",\"type\":\"invalid_request_error\"}}\n\n";
    let retryable = events_of(body)
        .iter()
        .find_map(|event| match event {
            StreamEvent::Error { retryable, .. } => Some(*retryable),
            _ => None,
        })
        .expect("gh #202: the mid-stream error is surfaced");
    assert!(!retryable, "an auth refusal does not clear itself on retry");
}
