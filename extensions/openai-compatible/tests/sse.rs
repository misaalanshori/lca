//! Offline tests for the OpenAI streaming parser: no network, canned bytes
//! only (testing plan section 4).

use lca_protocol::StreamEvent;
use openai_compatible::{classify_status, parse_sse};

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
