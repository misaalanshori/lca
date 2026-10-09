//! The Anthropic stream against the mock gateway (gh #183): typed
//! events out of the recorded fixture, wire shapes asserted on the
//! recorded request.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;

mod common;

use common::{mock_server, sandbox};

fn live_cap(mock: &common::Mock, name: &str) -> Arc<lca_tools::Capabilities> {
    let cap = sandbox("anthropic", name, mock, anthropic::manifest_grants());
    cap.credentials_set("access", "live-access").expect("seed");
    cap.credentials_set("refresh", "live-refresh")
        .expect("seed");
    cap.credentials_set("expires", &format!("{}", u64::MAX - 60))
        .expect("seed");
    cap
}

fn user_request(model: &str) -> lca_protocol::CompletionRequest {
    lca_protocol::CompletionRequest {
        messages: vec![lca_protocol::ChatMessage {
            role: lca_protocol::MessageRole::User,
            content: vec![lca_protocol::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        }],
        tools: vec![lca_protocol::ToolSpec {
            name: "read".to_string(),
            description: "read".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        }],
        model: model.to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

// Verifies: gh #183 - a turn streams typed events (reasoning, the
// thinking signature, text, a tool call with its arguments, usage
// with both cache buckets) and the wire carries the Messages shape:
// the path, the version and beta headers, the subscription identity,
// cache breakpoints, and a token budget.
#[test]
fn a_turn_streams_typed_events_over_the_messages_shape() {
    let mock = mock_server();
    let cap = live_cap(&mock, "stream");
    let request = user_request("claude-3-5-sonnet");
    let mut events = Vec::new();
    anthropic::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        events.push(event);
        true
    })
    .expect("stream completes");

    let text: String = events
        .iter()
        .filter_map(|event| match event {
            lca_protocol::StreamEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        text.contains("Hi there"),
        "the fixture's text streams: {text}"
    );
    let reasoning: String = events
        .iter()
        .filter_map(|event| match event {
            lca_protocol::StreamEvent::ReasoningDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        reasoning.contains("let me think"),
        "the thinking streams: {reasoning}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            lca_protocol::StreamEvent::VendorEvent { kind, payload }
            if kind == "thinking-signature"
                && payload.get("signature").and_then(|v| v.as_str()) == Some("sig-bytes")
        )),
        "the signature crosses for continuity"
    );
    let (call_id, name, args): (String, String, String) = events.iter().fold(
        (String::new(), String::new(), String::new()),
        |mut acc, event| {
            match event {
                lca_protocol::StreamEvent::ToolCallStart { call_id, name } => {
                    acc.0 = call_id.clone();
                    acc.1 = name.clone();
                }
                lca_protocol::StreamEvent::ToolCallArgDelta { delta, .. } => acc.2.push_str(delta),
                _ => {}
            }
            acc
        },
    );
    assert_eq!(name, "read");
    assert!(!call_id.is_empty());
    assert!(args.contains("a.txt"), "arguments reassemble: {args}");
    let usage = events.iter().find_map(|event| match event {
        lca_protocol::StreamEvent::Usage { usage } => Some(usage),
        _ => None,
    });
    let usage = usage.expect("usage streams");
    assert!(usage.input > 0 && usage.output > 0);
    assert!(
        usage.cache_read > 0 && usage.cache_write > 0,
        "both cache buckets report: {usage:?}"
    );

    let calls = mock.requests_of("/v1/messages");
    assert_eq!(calls.len(), 1, "one call: {calls:?}");
    let call = &calls[0];
    assert!(call.contains("x-api-key"), "the key header rides");
    assert!(
        call.contains("live-access"),
        "the subscription token authenticates"
    );
    assert!(call.contains("2023-06-01"), "the API version rides: {call}");
    assert!(
        call.contains("prompt-caching-2024-07-31"),
        "the cache beta rides: {call}"
    );
    assert!(
        call.contains("oauth-2025-04-20"),
        "the subscription identity rides: {call}"
    );
    assert!(call.contains("cache_control"), "breakpoints plant: {call}");
    assert!(call.contains("max_tokens"), "a budget rides: {call}");
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: gh #183 - an API key authenticates without the OAuth
// identity betas: plain billing, no subscription headers.
#[test]
fn an_api_key_turn_carries_no_subscription_identity() {
    let mock = mock_server();
    let cap = sandbox("anthropic", "keyturn", &mock, anthropic::manifest_grants());
    cap.credentials_set("api_key", "sk-ant-test").expect("seed");
    let request = user_request("claude-3-5-haiku");
    let mut events = Vec::new();
    anthropic::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        events.push(event);
        true
    })
    .expect("stream completes");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, lca_protocol::StreamEvent::TextDelta { .. })),
        "the turn streams"
    );
    let calls = mock.requests_of("/v1/messages");
    assert_eq!(calls.len(), 1);
    assert!(calls[0].contains("sk-ant-test"), "the key rides");
    assert!(
        !calls[0].contains("oauth-2025-04-20"),
        "no subscription identity on a key call"
    );
}

// Verifies: gh #183 - a rejected subscription purges the trio, so the
// next call re-authenticates instead of looping on a dead token.
#[test]
fn a_rejected_subscription_purges_its_tokens() {
    let mock = mock_server();
    mock.fail_once(
        "/v1/messages",
        401,
        Some(
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid token"}}"#,
        ),
    );
    let cap = live_cap(&mock, "purge");
    let request = user_request("claude-3-5-sonnet");
    let mut events = Vec::new();
    let outcome = anthropic::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        events.push(event);
        true
    });
    assert!(outcome.is_err(), "the 401 surfaces");
    let err = outcome.expect_err("surfaces");
    assert!(
        err.message.contains("invalid token"),
        "the nested message reads: {}",
        err.message
    );
    assert!(
        lca_protocol::ProviderCap::credentials_get(cap.as_ref(), "access").is_none(),
        "the trio purges"
    );
}
