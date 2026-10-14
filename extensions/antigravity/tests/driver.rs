//! The pull-based Antigravity stream driver: events leave as the host yields
//! body chunks, not after the whole response (`docs/deferred_workplan.md`
//! C1). Deterministic: a fake capability view scripts the chunks.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use antigravity::StreamDriver;
use lca_protocol::{CapabilityError, CompletionRequest, ProviderCap, StreamEvent};

/// A capability view with a valid stored token and scripted body chunks;
/// counts how many chunks the driver asked for.
struct ChunkedCap {
    chunks: Mutex<VecDeque<Option<Vec<u8>>>>,
    reads: AtomicUsize,
}

impl ProviderCap for ChunkedCap {
    fn net_request(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(&str, &str)],
        _body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        Ok(1)
    }

    fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
        Ok(200)
    }

    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.chunks.lock().unwrap().pop_front().flatten())
    }

    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_get(&self, key: &str) -> Option<String> {
        match key {
            // A non-expired stored token, so `open` needs no refresh.
            "access" => Some("mock-access".to_string()),
            "expires" => Some("99999999999".to_string()),
            _ => None,
        }
    }

    fn credentials_set(&self, _key: &str, _value: &str) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_delete(&self, _key: &str) -> Result<(), CapabilityError> {
        Ok(())
    }
}

fn frame(text: &str) -> Vec<u8> {
    format!(
        "data: {{\"candidates\":[{{\"content\":{{\"parts\":[{{\"text\":\"{text}\"}}]}}}}]}}\n\n"
    )
    .into_bytes()
}

#[test]
fn the_driver_yields_an_event_before_the_body_ends() {
    let cap = ChunkedCap {
        chunks: Mutex::new(VecDeque::from(vec![
            Some(frame("Hi")),
            Some(frame(" there")),
            Some(b"data: [DONE]\n\n".to_vec()),
            None,
        ])),
        reads: AtomicUsize::new(0),
    };
    let mut driver = StreamDriver::open(&cap, &CompletionRequest::default()).expect("open");

    let first = driver.next_event().expect("an event").expect("ok");
    assert_eq!(first, StreamEvent::TextDelta { delta: "Hi".into() });
    assert_eq!(
        cap.reads.load(Ordering::SeqCst),
        1,
        "the first event did not drain the body"
    );

    let second = driver.next_event().expect("an event").expect("ok");
    assert_eq!(
        second,
        StreamEvent::TextDelta {
            delta: " there".into()
        }
    );
    assert!(driver.next_event().is_none(), "the stream ends");
}

// Verifies: gh #241A - a bare JSON error body (no SSE framing, as
// gateways and proxies return rejections) surfaces as an Error event
// instead of an empty silent end (pi-antigravity's stray-line rule:
// non-`data:` lines accumulate, and an `{"error": ...}` envelope
// throws rather than vanishing).
#[test]
fn bare_error_bodies_surface_instead_of_ending_silently() {
    for body in [
        "{\"error\":{\"code\":403,\"message\":\"denied by policy\"}}\n\n",
        "{\"error\":{\"code\":403,\"message\":\"denied by policy\"}}",
    ] {
        let cap = ChunkedCap {
            chunks: Mutex::new(VecDeque::from(vec![Some(body.as_bytes().to_vec()), None])),
            reads: AtomicUsize::new(0),
        };
        let mut driver = StreamDriver::open(&cap, &CompletionRequest::default()).expect("open");
        let event = driver.next_event().expect("an event, not silence");
        match event {
            Ok(lca_protocol::StreamEvent::Error { message, .. }) => {
                assert!(message.contains("denied by policy"), "{message}")
            }
            other => panic!("an Error event, got {other:?}"),
        }
    }
}

// Verifies: Google Cloud Code / Antigravity returns stream chunks
// wrapped in a top-level `{"response": {"candidates": [...], "usageMetadata": ...}}`
// envelope; both candidates and usage must unwrap and emit properly.
#[test]
fn wrapped_cloud_code_response_envelopes_decode_text_deltas_and_usage() {
    let chunk1 = b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}]}}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":2}}}\n\n".to_vec();
    let chunk2 = b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" world\"}]}}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":4}}}\n\n".to_vec();
    let cap = ChunkedCap {
        chunks: Mutex::new(VecDeque::from(vec![
            Some(chunk1),
            Some(chunk2),
            Some(b"data: [DONE]\n\n".to_vec()),
            None,
        ])),
        reads: AtomicUsize::new(0),
    };
    let mut driver = StreamDriver::open(&cap, &CompletionRequest::default()).expect("open");
    let ev1 = driver.next_event().expect("ev1").expect("ok");
    assert_eq!(
        ev1,
        StreamEvent::TextDelta {
            delta: "Hello".into()
        }
    );
    let ev2 = driver.next_event().expect("ev2").expect("ok");
    assert!(matches!(ev2, StreamEvent::Usage { .. }));
    let ev3 = driver.next_event().expect("ev3").expect("ok");
    assert_eq!(
        ev3,
        StreamEvent::TextDelta {
            delta: " world".into()
        }
    );
}
