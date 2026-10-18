//! Gh #359: the Responses driver dropped its trailing buffer at EOF and
//! stayed silent on a 200 body carrying zero SSE frames.
//!
//! Two gaps at end of stream, both of the same class as gh #15 (a non-SSE
//! body read as a silent empty success):
//!
//! 1. `drain_frames` fired only on `\n\n`. A body whose final frame lost its
//!    terminator (a truncated transfer, a proxy that dropped the tail) was
//!    discarded instead of parsed or reported.
//! 2. A 200 with no frames at all produced no events, so the turn ended with
//!    an empty assistant message and no error — indistinguishable from a
//!    model that said nothing.
//!
//! These rows drive the **real** driver over a chunk-controlled
//! `ProviderCap`, so the assertions are about what a caller sees from
//! `next_event`, not about an internal helper.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Mutex;

use lca_protocol::{CapabilityError, ProviderCap, StreamEvent};
use lca_wire_openai::ResponseStreamDriver;

/// A provider whose body arrives as the exact chunks the test hands it.
struct ChunkCap {
    chunks: Mutex<Vec<Vec<u8>>>,
    status: u16,
}

impl ChunkCap {
    fn new(status: u16, chunks: Vec<&[u8]>) -> ChunkCap {
        ChunkCap {
            chunks: Mutex::new(chunks.into_iter().map(|c| c.to_vec()).collect()),
            status,
        }
    }
}

impl ProviderCap for ChunkCap {
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
        Ok(self.status)
    }

    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut chunks = self.chunks.lock().expect("chunks");
        if chunks.is_empty() {
            return Ok(None);
        }
        Ok(Some(chunks.remove(0)))
    }

    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_get(&self, _key: &str) -> Option<String> {
        None
    }

    fn credentials_set(&self, _key: &str, _value: &str) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_delete(&self, _key: &str) -> Result<(), CapabilityError> {
        Ok(())
    }
}

/// Every event the driver yields, errors included as `Err` messages.
fn drive(cap: &ChunkCap) -> (Vec<StreamEvent>, Vec<String>) {
    let mut driver = ResponseStreamDriver::open(cap, "http://x/y", &[], b"{}", &|| {})
        .expect("the fake opens successfully");
    let mut events = Vec::new();
    let mut errors = Vec::new();
    while let Some(next) = driver.next_event() {
        match next {
            Ok(event) => {
                // A transport error can also arrive as an event; keep both
                // shapes so an assertion cannot miss one.
                if let StreamEvent::Error { message, .. } = &event {
                    errors.push(message.clone());
                }
                events.push(event);
            }
            Err(failure) => errors.push(failure.message),
        }
    }
    (events, errors)
}

/// The text the driver produced.
fn text_of(events: &[StreamEvent]) -> String {
    let mut text = String::new();
    for event in events {
        if let StreamEvent::TextDelta { delta } = event {
            text.push_str(delta);
        }
    }
    text
}

fn delta_body(delta: &str) -> String {
    serde_json::json!({"type": "response.output_text.delta", "delta": delta}).to_string()
}

// Verifies: gh #359 acceptance 1 - a body whose last frame never got its
// terminator still delivers its content. Before the fix these bytes were
// dropped silently.
#[test]
fn a_truncated_tail_still_delivers_its_content() {
    let complete = format!("data: {}\n\n", delta_body("first"));
    let tail = format!("data: {}", delta_body("second"));
    let cap = ChunkCap::new(200, vec![complete.as_bytes(), tail.as_bytes()]);
    let (events, errors) = drive(&cap);
    assert_eq!(
        text_of(&events),
        "firstsecond",
        "the truncated tail's delta still arrives: {errors:?}"
    );
    assert!(
        errors.is_empty(),
        "a parseable tail is content, not an error: {errors:?}"
    );
}

// Verifies: gh #359 acceptance 1 - a tail that is not a frame at all is
// reported, never dropped. Received bytes must not vanish silently.
#[test]
fn an_unparseable_tail_surfaces_a_transport_error() {
    let complete = format!("data: {}\n\n", delta_body("ok"));
    let cap = ChunkCap::new(200, vec![complete.as_bytes(), b"this is not a frame"]);
    let (events, errors) = drive(&cap);
    assert_eq!(text_of(&events), "ok", "the good frame still lands");
    assert!(
        errors.iter().any(|error| error.contains("mid-frame")),
        "the dropped tail is named: {errors:?}"
    );
}

// Verifies: gh #359 acceptance 2 - a 200 with zero frames fails loudly. An
// empty assistant message must never be the report of a body that was not a
// stream.
#[test]
fn a_zero_frame_body_is_a_transport_error() {
    let cap = ChunkCap::new(200, vec![b"<html>not a stream</html>"]);
    let (events, errors) = drive(&cap);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("no server-sent event frames")),
        "the zero-frame body is reported: {errors:?}"
    );
    assert!(
        text_of(&events).is_empty(),
        "and it contributes no text: {events:?}"
    );
}

// Verifies: gh #359 - an entirely empty 200 body is the same case: loud, not
// an empty success.
#[test]
fn an_empty_body_is_a_transport_error() {
    let cap = ChunkCap::new(200, vec![]);
    let (_, errors) = drive(&cap);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("no server-sent event frames")),
        "{errors:?}"
    );
}

// Verifies: gh #359 acceptance 3 + 4 - the normal path is unchanged: a
// well-formed stream delivers its text, its usage, and no error.
#[test]
fn a_well_formed_stream_is_unchanged() {
    let first = format!("data: {}\n\n", delta_body("hello "));
    let second = format!("data: {}\n\n", delta_body("world"));
    let usage = serde_json::json!({
        "type": "response.completed",
        "response": {"usage": {"input_tokens": 11, "output_tokens": 4}},
    });
    let usage_frame = format!("data: {usage}\n\n");
    let cap = ChunkCap::new(
        200,
        vec![first.as_bytes(), second.as_bytes(), usage_frame.as_bytes()],
    );
    let (events, errors) = drive(&cap);
    assert_eq!(text_of(&events), "hello world");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::Usage { .. })),
        "usage still arrives: {events:?}"
    );
    assert!(errors.is_empty(), "no error on a good stream: {errors:?}");
}

// Verifies: gh #337 at the DRIVER level (the framing matrix lives in
// `utf8_straddle.rs`, which covers the shared splitter): a codepoint split
// across two body chunks arrives intact through the real driver.
#[test]
fn a_split_codepoint_arrives_intact_through_the_driver() {
    let body = format!("data: {}\n\n", delta_body("中文 😀"));
    let bytes = body.as_bytes();
    let start = body.find('中').expect("in body");
    // Split inside the 3-byte codepoint, one byte in.
    let at = start + 1;
    let cap = ChunkCap::new(200, vec![&bytes[..at], &bytes[at..]]);
    let (events, errors) = drive(&cap);
    assert_eq!(text_of(&events), "中文 😀", "{errors:?}");
    assert!(
        !text_of(&events).contains('\u{FFFD}'),
        "no replacement chars"
    );
}

// Verifies: gh #359 - the driver's error is a transport failure with the
// class the headless envelope reads, and it is not retried (the body was not
// a stream; a second attempt sees the same bytes).
#[test]
fn the_zero_frame_error_is_a_non_retryable_transport_failure() {
    let cap = ChunkCap::new(200, vec![b""]);
    let mut driver =
        ResponseStreamDriver::open(&cap, "http://x/y", &[], b"{}", &|| {}).expect("opens");
    let mut saw_error_event = false;
    while let Some(next) = driver.next_event() {
        match next {
            Ok(StreamEvent::Error { retryable, .. }) => {
                saw_error_event = true;
                assert!(!retryable, "a non-stream body will not become one");
            }
            Ok(_) => {}
            Err(failure) => {
                assert_eq!(failure.class, "transport", "{failure:?}");
                assert!(!failure.retryable);
            }
        }
    }
    assert!(
        saw_error_event,
        "the failure reaches the caller as an event"
    );
}
