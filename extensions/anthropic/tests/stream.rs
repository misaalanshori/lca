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
    assert!(call.contains("x-app"), "the x-app header rides: {call}");
    assert!(
        call.contains("claude-cli"),
        "the claude-cli user agent rides: {call}"
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
    assert!(
        !calls[0].contains("x-app"),
        "api key turn carries no subscription x-app"
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

/// A scripted transport: 429 once, with a `retry-after` header, so the
/// producer path is pinned without touching the mock gateway's fixed
/// headers.
struct ThrottledCap {
    reads: std::sync::Mutex<u32>,
}

impl lca_protocol::ProviderCap for ThrottledCap {
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
        Ok(429)
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
                br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#
                    .to_vec(),
            ))
        } else {
            Ok(None)
        }
    }
    fn net_close_response(&self, _handle: u32) -> Result<(), lca_protocol::CapabilityError> {
        Ok(())
    }
    fn net_response_headers(
        &self,
        _handle: u32,
    ) -> Result<Vec<(String, String)>, lca_protocol::CapabilityError> {
        Ok(vec![("retry-after".to_string(), "7".to_string())])
    }
    fn credentials_get(&self, _key: &str) -> Option<String> {
        None
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

// Verifies: gh #52 - a throttled response carries the server's delay
// on the error text (`retry-after: 7`), the token the host's retry
// honors across the string-only WIT boundary.
#[test]
fn a_throttled_response_carries_its_retry_after() {
    let cap = ThrottledCap {
        reads: std::sync::Mutex::new(0),
    };
    let err = match anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}") {
        Err(err) => err,
        Ok(_) => panic!("the 429 surfaces"),
    };
    assert!(err.retryable, "a 429 stays retryable");
    assert!(
        err.message.contains("retry-after: 7"),
        "the server delay rides the message: {}",
        err.message
    );
}

// ---------------------------------------------------------------------------
// gh #278: the header must be read before the handle is closed
// ---------------------------------------------------------------------------

/// A capability that models the host's real handle lifecycle: closing a
/// response **removes** it, so any header read afterwards is `NotFound`.
///
/// The existing `ThrottledCap` returns headers unconditionally, which is why
/// `a_throttled_response_carries_its_retry_after` passed while the extension
/// closed first and read second (gh #278). A double that cannot fail the way
/// the real host fails proves nothing about ordering.
struct LifecycleCap {
    reads: std::sync::Mutex<u32>,
    closed: std::sync::atomic::AtomicBool,
    /// What the response carries as `retry-after`, if anything.
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
                br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#
                    .to_vec(),
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
        // The host's behaviour: a closed handle is gone from the table.
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
    fn credentials_get(&self, _key: &str) -> Option<String> {
        None
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

// Verifies: gh #278 acceptance 1 - the `retry-after` value survives to the
// error text when the double enforces the real handle lifecycle. Against the
// pre-fix ordering this row fails: the header read happens after close, hits
// `NotFound`, and the delay is silently lost.
#[test]
fn the_retry_after_header_is_read_before_the_handle_closes() {
    let cap = LifecycleCap::throttled(Some("7"));
    let err = match anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}") {
        Err(err) => err,
        Ok(_) => panic!("the 429 surfaces"),
    };
    assert!(err.retryable, "a 429 stays retryable");
    assert!(
        err.message.contains("retry-after: 7"),
        "the server delay survives: {}",
        err.message
    );
}

// Verifies: gh #278 acceptance 2 - the handle is still closed. Reading
// headers first must not leak the response; the close is mandatory, not
// optional.
#[test]
fn the_handle_is_closed_after_the_headers_are_read() {
    let cap = LifecycleCap::throttled(Some("3"));
    let _ = anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}");
    assert!(
        cap.closed.load(std::sync::atomic::Ordering::SeqCst),
        "the response handle is closed on the error path"
    );
}

// Verifies: gh #278 design rule 3 - a response with no `retry-after` falls
// back to the host's own backoff, and the message does not invent a delay.
#[test]
fn a_throttle_without_the_header_carries_no_delay() {
    let cap = LifecycleCap::throttled(None);
    let err = match anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}") {
        Err(err) => err,
        Ok(_) => panic!("the 429 surfaces"),
    };
    assert!(
        !err.message.contains("retry-after"),
        "no header, no token: {}",
        err.message
    );
    assert!(cap.closed.load(std::sync::atomic::Ordering::SeqCst));
}

// Verifies: gh #278 - the value rides verbatim, including the HTTP-date
// form the host's parser understands (pi accepts both). The extension must
// not try to interpret it; it forwards the server's words.
#[test]
fn an_http_date_retry_after_rides_verbatim() {
    let cap = LifecycleCap::throttled(Some("Wed, 21 Oct 2026 07:28:00 GMT"));
    let err = match anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}") {
        Err(err) => err,
        Ok(_) => panic!("the 429 surfaces"),
    };
    assert!(
        err.message
            .contains("retry-after: Wed, 21 Oct 2026 07:28:00 GMT"),
        "the date form is forwarded, not reinterpreted: {}",
        err.message
    );
}
