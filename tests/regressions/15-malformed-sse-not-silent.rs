//! Released defect (0.1.x): a provider that returned HTTP 200 with a body
//! that was not server-sent events read as a silent empty success (exit 0, no
//! answer). The SSE decoder skipped every unrecognized line and never noticed
//! that it had seen no frame at all.
//!
//! Verifies: FR-CORE-4.

use lca_protocol::StreamEvent;
use openai_compatible::parse_sse;

fn events(body: &[u8]) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    parse_sse(body, &mut |event| events.push(event));
    events
}

#[test]
fn a_non_sse_body_is_an_error_not_an_empty_answer() {
    let events = events(b"this is not SSE at all\n");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::Error { .. })),
        "a non-SSE body must surface: {events:?}"
    );
}

#[test]
fn a_well_formed_empty_stream_is_not_an_error() {
    let events = events(b"data: {\"choices\":[{\"delta\":{}}]}\n\ndata: [DONE]\n\n");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, StreamEvent::Error { .. })),
        "an empty but valid stream is a valid empty answer: {events:?}"
    );
}
