//! The llama stream against the mock server (gh #62): typed events
//! out of the recorded fixture, over the server's `/v1` prefix.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;

mod common;

use common::{mock_server, sandbox};

fn live_cap(mock: &common::Mock, name: &str) -> Arc<lca_tools::Capabilities> {
    sandbox("llama", name, mock, llama::manifest_grants())
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
        tools: Vec::new(),
        model: model.to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

// Verifies: gh #62 - a turn streams typed events to the server's
// `/v1/chat/completions` with the loaded model.
#[test]
fn a_turn_streams_typed_events_over_the_v1_prefix() {
    let mock = mock_server();
    let cap = live_cap(&mock, "stream");
    let request = user_request("mock/loaded");
    let mut events = Vec::new();
    llama::run_provider_stream(cap.as_ref(), &request, &mut |event| {
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

    let calls = mock.requests_of("/v1/chat/completions");
    assert_eq!(calls.len(), 1, "one call: {calls:?}");
    assert!(
        calls[0].contains("mock/loaded"),
        "the model rides: {}",
        calls[0]
    );
    assert!(
        !calls[0].contains("authorization"),
        "no bearer without a key: {}",
        calls[0]
    );
    assert!(cap.denials().is_empty(), "everything was granted");
}
