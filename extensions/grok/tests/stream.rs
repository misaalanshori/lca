//! The Grok stream against the mock gateway (gh #181): typed
//! events out, wire shapes asserted on the recorded request.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;

mod common;

use common::{mock_server, sandbox};

fn live_cap(mock: &common::Mock, name: &str) -> Arc<lca_tools::Capabilities> {
    let cap = sandbox("grok", name, mock, grok::manifest_grants(), "test-client");
    cap.credentials_set("access", "live-access").expect("seed");
    cap.credentials_set("account_id", "acct-1").expect("seed");
    cap.credentials_set("expires", &format!("{}", u64::MAX - 60))
        .expect("seed");
    cap
}

fn user_request(
    model: &str,
    tools: Vec<lca_protocol::ToolSpec>,
) -> lca_protocol::CompletionRequest {
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
        tools,
        model: model.to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

// Verifies: gh #181 - a turn streams typed events (text, a tool call
// with its arguments, usage) and the wire carries the Grok shape:
// the responses path, `store: false`, the JWT account header, and
// pi's `originator` spelled for this agent.
#[test]
fn a_turn_streams_typed_events_over_the_grok_shape() {
    let mock = mock_server();
    let cap = live_cap(&mock, "stream");
    let request = user_request(
        "grok-4.20",
        vec![lca_protocol::ToolSpec {
            name: "read".to_string(),
            description: "read".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        }],
    );
    let mut events = Vec::new();
    grok::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        events.push(event);
        true
    })
    .expect("stream completes");

    let text: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            lca_protocol::StreamEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, vec!["Hi"]);
    let order: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            lca_protocol::StreamEvent::ToolCallStart { name, .. } => Some(format!("start:{name}")),
            lca_protocol::StreamEvent::ToolCallArgDelta { .. } => Some("delta".to_string()),
            lca_protocol::StreamEvent::ToolCallEnd { .. } => Some("end".to_string()),
            lca_protocol::StreamEvent::Usage { usage } => {
                Some(format!("usage:{}:{}", usage.input, usage.output))
            }
            _ => None,
        })
        .collect();
    assert_eq!(order, vec!["start:read", "delta", "end", "usage:11:4"]);

    let calls = mock.requests_of("/v1/responses");
    assert_eq!(calls.len(), 1, "one responses call: {calls:?}");
    assert!(
        calls[0].contains(r#""store":false"#),
        "store false: {}",
        calls[0]
    );
    assert!(
        calls[0].contains(r#""model":"grok-4.20""#),
        "the model rides: {}",
        calls[0]
    );
    assert!(
        calls[0].contains("x-grok-user-id"),
        "the account header: {}",
        calls[0]
    );
    assert!(
        calls[0].contains("x-grok-client-identifier") && calls[0].contains("lca"),
        "this agent's identifier: {}",
        calls[0]
    );
    assert!(cap.denials().is_empty());
}

// Verifies: gh #181 - a 401 on the stream purges the stored tokens,
// so the next call reports "no login" instead of looping.
#[test]
fn a_401_purges_the_namespace() {
    let mock = mock_server();
    let cap = live_cap(&mock, "purge");
    mock.fail_once(
        "/v1/responses",
        401,
        Some(r#"{"error":{"message":"invalid credentials"}}"#),
    );
    let request = user_request("grok-4.20", Vec::new());
    let failed = grok::run_provider_stream(cap.as_ref(), &request, &mut |_| true);
    assert!(failed.is_err(), "the rejected call fails");

    for key in ["access", "refresh", "expires"] {
        assert_eq!(
            lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key),
            None,
            "{key} purged"
        );
    }
    let usage = grok::run_usage(cap.as_ref());
    assert!(
        format!("{usage:?}").contains("no grok login yet"),
        "the next call re-authenticates: {usage:?}"
    );
}
