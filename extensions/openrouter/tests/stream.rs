//! The OpenRouter stream against the mock gateway (gh #185): typed
//! events out of the recorded fixture, wire shapes asserted on the
//! recorded request.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;

mod common;

use common::{mock_server, sandbox};

fn live_cap(mock: &common::Mock, name: &str) -> Arc<lca_tools::Capabilities> {
    let cap = sandbox("openrouter", name, mock, openrouter::manifest_grants());
    cap.credentials_set("access", "sk-or-live").expect("seed");
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

// Verifies: gh #185 - a turn streams typed events (text, a tool call
// with its arguments, usage) over the chat path with the provisioned
// key as the bearer.
#[test]
fn a_turn_streams_typed_events_over_chat_completions() {
    let mock = mock_server();
    let cap = live_cap(&mock, "stream");
    let request = user_request("openai/gpt-4o");
    let mut events = Vec::new();
    openrouter::run_provider_stream(cap.as_ref(), &request, &mut |event| {
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
    assert_eq!(usage.input, 10, "prompt tokens bill: {usage:?}");
    assert_eq!(usage.output, 3, "completion tokens bill: {usage:?}");

    let calls = mock.requests_of("/chat/completions");
    assert_eq!(calls.len(), 1, "one call: {calls:?}");
    let call = &calls[0];
    assert!(call.contains("Bearer sk-or-live"), "the key rides");
    assert!(call.contains("openai/gpt-4o"), "the model rides: {call}");
    assert!(call.contains("\"tools\""), "tools ride: {call}");
    assert!(call.contains("include_usage"), "usage requested: {call}");
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: gh #185 - without a login and without the environment
// key, the turn fails with the re-login error (never an empty
// bearer); with the variable set the environment signs instead.
#[test]
fn without_a_login_the_re_login_error_names_the_way_back() {
    if std::env::var("OPENROUTER_API_KEY").is_ok_and(|key| !key.trim().is_empty()) {
        eprintln!("skipping: OPENROUTER_API_KEY is set, so the environment signs");
        return;
    }
    let mock = mock_server();
    let cap = sandbox(
        "openrouter",
        "nologin",
        &mock,
        openrouter::manifest_grants(),
    );
    let request = user_request("openai/gpt-4o");
    let mut events = Vec::new();
    let outcome = openrouter::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        events.push(event);
        true
    });
    let Err(failure) = outcome else {
        panic!("an keyless turn must fail");
    };
    assert!(
        failure.message.contains("/login openrouter"),
        "the re-login error names the way back: {}",
        failure.message
    );
    assert!(
        mock.requests_of("/chat/completions").is_empty(),
        "no bearer-less call crosses"
    );
}

// ---------------------------------------------------------------------------
// gh #278: the `retry-after` header must be read before the handle closes
// ---------------------------------------------------------------------------

/// A capability that models the host's handle lifecycle: closing removes the
/// response, so a header read afterwards is `NotFound`. A double that always
/// answers headers cannot catch an ordering bug, which is why the anthropic
/// sibling's original test stayed green over the defect.
struct LifecycleCap {
    reads: std::sync::Mutex<u32>,
    closed: std::sync::atomic::AtomicBool,
    retry_after: Option<&'static str>,
    status: u16,
}

impl LifecycleCap {
    fn throttled(retry_after: Option<&'static str>) -> LifecycleCap {
        LifecycleCap {
            reads: std::sync::Mutex::new(0),
            closed: std::sync::atomic::AtomicBool::new(false),
            retry_after,
            status: 429,
        }
    }
}

impl lca_protocol::ProviderCap for LifecycleCap {
    fn net_request(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(&str, &str)],
        _body: Option<&[u8]>,
    ) -> Result<u32, lca_protocol::CapabilityError> {
        Ok(1)
    }
    fn net_response_status(&self, _handle: u32) -> Result<u16, lca_protocol::CapabilityError> {
        Ok(self.status)
    }
    fn net_read_body(
        &self,
        _handle: u32,
        _max: usize,
    ) -> Result<Option<Vec<u8>>, lca_protocol::CapabilityError> {
        let mut reads = self.reads.lock().expect("reads");
        *reads += 1;
        if *reads == 1 {
            Ok(Some(
                br#"{"error":{"message":"rate limited","code":429}}"#.to_vec(),
            ))
        } else {
            Ok(None)
        }
    }
    fn net_close_response(&self, _handle: u32) -> Result<(), lca_protocol::CapabilityError> {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn net_response_headers(
        &self,
        _handle: u32,
    ) -> Result<Vec<(String, String)>, lca_protocol::CapabilityError> {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(lca_protocol::CapabilityError::NotFound(
                "unknown handle 1".to_string(),
            ));
        }
        Ok(self
            .retry_after
            .map(|value| vec![("retry-after".to_string(), value.to_string())])
            .unwrap_or_default())
    }
    fn credentials_get(&self, key: &str) -> Option<String> {
        (key == "access").then(|| "sk-or-mock".to_string())
    }
    fn credentials_set(
        &self,
        _key: &str,
        _value: &str,
    ) -> Result<(), lca_protocol::CapabilityError> {
        Ok(())
    }
    fn credentials_delete(&self, _key: &str) -> Result<(), lca_protocol::CapabilityError> {
        Ok(())
    }
}

// Verifies: gh #278 - openrouter lost the server's `retry-after` entirely
// (it never read the header at all) and would have hit `NotFound` had it
// read after the close. Both halves are pinned: the value reaches the error
// text, and the handle is still closed.
#[test]
fn a_throttled_response_carries_its_retry_after() {
    let cap = LifecycleCap::throttled(Some("5"));
    let err =
        match openrouter::ChatDriver::open(&cap, "https://mock/v1/chat/completions", &[], b"{}") {
            Err(err) => err,
            Ok(_) => panic!("the 429 surfaces"),
        };
    assert!(err.retryable, "a 429 stays retryable");
    assert!(
        err.message.contains("retry-after: 5"),
        "the server delay rides the message: {}",
        err.message
    );
    assert!(
        cap.closed.load(std::sync::atomic::Ordering::SeqCst),
        "and the handle is still closed"
    );
}

// Verifies: gh #278 design rule 3 - no header means no token, so the host
// falls back to its exponential schedule rather than retrying immediately.
#[test]
fn a_throttle_without_the_header_carries_no_delay() {
    let cap = LifecycleCap::throttled(None);
    let err =
        match openrouter::ChatDriver::open(&cap, "https://mock/v1/chat/completions", &[], b"{}") {
            Err(err) => err,
            Ok(_) => panic!("the 429 surfaces"),
        };
    assert!(!err.message.contains("retry-after"), "{}", err.message);
}
