//! Gh #359: the Responses driver dropped its trailing buffer at EOF and was
//! silent on a 200 body with zero SSE frames.
//!
//! `drain_frames` fired only on `\n\n`, and the EOF arm set `finished` without
//! flushing — so a body whose last frame lost its terminator (a truncated
//! transfer, a proxy that ate the tail) had those bytes silently discarded.
//! And a 200 carrying no frames at all produced no events, so the turn ended
//! with an empty assistant message: indistinguishable from a model that said
//! nothing, and the same class as gh #15's non-SSE body read.
//!
//! Both are fixed at the framing level: the tail is parsed (a `data:` line
//! means a frame that lost its terminator, anything else surfaces as a
//! transport error), and a body that never carried a frame fails loudly.
//!
//! Verifies: gh #359 acceptance 1-3 at the wire-contract level.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The framing half of the driver, driven directly: feed these chunks, get
/// the payloads the driver would have parsed plus the tail it would report.
fn frame_and_tail(chunks: &[&[u8]]) -> (Vec<String>, Option<String>) {
    let mut buffer = lca_wire_openai::sse::FrameBuffer::new();
    let mut payloads = Vec::new();
    for chunk in chunks {
        buffer.feed(chunk, &mut |frame| {
            for payload in lca_wire_openai::sse::frame_payloads(frame) {
                payloads.push(payload.to_string());
            }
        });
    }
    (payloads, buffer.take_tail())
}

// Verifies: gh #359 acceptance 1 — a body whose final frame lost its
// terminator still yields that frame's payload, rather than dropping it.
#[test]
fn a_truncated_tail_is_returned_rather_than_dropped() {
    let complete = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\n\n";
    let tail = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"b\"}";
    let (payloads, rest) = frame_and_tail(&[complete.as_bytes(), tail.as_bytes()]);
    assert_eq!(payloads.len(), 1, "the complete frame parsed: {payloads:?}");
    let rest = rest.expect("the tail is returned, not dropped");
    assert!(rest.contains("\"delta\":\"b\""), "{rest:?}");
    // And the tail is a real `data:` frame, so the driver parses it.
    assert_eq!(
        lca_wire_openai::sse::frame_payloads(&rest).count(),
        1,
        "the tail carries one parseable payload"
    );
}

// Verifies: gh #359 acceptance 1 — a tail that is NOT a frame is reported,
// so no received byte disappears without a trace.
#[test]
fn an_unparseable_tail_is_reported() {
    let (_, rest) = frame_and_tail(&[b"<html>oops"]);
    let rest = rest.expect("returned");
    assert_eq!(
        lca_wire_openai::sse::frame_payloads(&rest).count(),
        0,
        "it carries no data: line"
    );
    // The event the driver emits for it names the truncation.
    match lca_wire_openai::sse::truncated_tail_event(&rest) {
        lca_protocol::StreamEvent::Error { message, retryable } => {
            assert!(message.contains("mid-frame"), "{message}");
            assert!(!retryable, "a truncated body will not become a stream");
        }
        other => panic!("expected an error event, got {other:?}"),
    }
}

// Verifies: gh #359 acceptance 2 — the zero-frame failure is a non-retryable
// transport error naming what actually happened.
#[test]
fn a_zero_frame_body_is_a_named_transport_failure() {
    let failure = lca_wire_openai::sse::no_frames_failure("");
    assert_eq!(failure.class, "transport");
    assert!(!failure.retryable);
    assert!(
        failure.message.contains("no server-sent event frames"),
        "{}",
        failure.message
    );
}

// Verifies: gh #359 acceptance 1 — the EOF flush parses what parses: a
// complete frame followed by a partial one yields the complete payload AND
// keeps the partial for the final pass, so neither is lost.
#[test]
fn a_complete_frame_plus_a_partial_one_loses_neither() {
    let body = "data: {\"a\":1}\n\ndata: {\"b\":2}\n";
    let (payloads, rest) = frame_and_tail(&[body.as_bytes()]);
    assert_eq!(payloads, vec!["{\"a\":1}".to_string()]);
    // The partial frame kept its bytes (the trailing newline is not a
    // terminator on its own).
    assert_eq!(rest.as_deref(), Some("data: {\"b\":2}\n"));
}
