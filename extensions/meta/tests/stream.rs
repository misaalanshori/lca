//! The Meta stream against the mock gateway (gh #186): typed events
//! out of the recorded fixture, the minted bearer on the wire.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;

mod common;

use common::{mock_server, sandbox};

fn live_cap(mock: &common::Mock, name: &str) -> Arc<lca_tools::Capabilities> {
    let cap = sandbox("meta", name, mock, meta::manifest_grants());
    cap.credentials_set("access", "meta-live-key")
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
        tools: Vec::new(),
        model: model.to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

// Verifies: gh #186 - a turn streams typed events over the Meta base
// with the minted key as the bearer.
#[test]
fn a_turn_streams_typed_events_over_the_meta_shape() {
    let mock = mock_server();
    let cap = live_cap(&mock, "stream");
    let request = user_request("llama-3.3-70b-instruct");
    let mut events = Vec::new();
    meta::run_provider_stream(cap.as_ref(), &request, &mut |event| {
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
    assert!(
        events
            .iter()
            .any(|event| matches!(event, lca_protocol::StreamEvent::Usage { .. })),
        "usage streams"
    );

    let calls = mock.requests_of("/chat/completions");
    assert_eq!(calls.len(), 1, "one call: {calls:?}");
    let call = &calls[0];
    assert!(
        call.contains("Bearer meta-live-key"),
        "the minted key rides"
    );
    assert!(call.contains("llama-3.3-70b-instruct"), "the model rides");
    assert!(cap.denials().is_empty(), "everything was granted");
}
