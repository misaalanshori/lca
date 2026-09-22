//! Tool-call accumulator tests: the host joins argument fragments keyed by
//! call id and reports every failure path (ADR-0004, FR-PROV-7, FR-PROV-8).

use lca_protocol::StreamEvent;
use lca_provider::{ProtocolError, ToolCallAccumulator};

fn start(call_id: &str, name: &str) -> StreamEvent {
    StreamEvent::ToolCallStart {
        call_id: call_id.into(),
        name: name.into(),
    }
}

fn delta(call_id: &str, fragment: &str) -> StreamEvent {
    StreamEvent::ToolCallArgDelta {
        call_id: call_id.into(),
        delta: fragment.into(),
    }
}

#[test]
fn joins_fragments_into_one_complete_call() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "read"));
    acc.handle(delta("c1", r#"{"pa"#));
    acc.handle(delta("c1", r#"th":"a.txt"}"#));
    acc.handle(StreamEvent::ToolCallEnd {
        call_id: "c1".into(),
    });
    let (calls, errors) = acc.finish(false);
    assert!(errors.is_empty());
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].call_id, "c1");
    assert_eq!(calls[0].name, "read");
    assert_eq!(calls[0].arguments, r#"{"path":"a.txt"}"#);
}

// Verifies: FR-PROV-8 (a delta with no open start is discarded and a
// protocol error is recorded)
#[test]
fn discards_a_delta_with_no_open_start() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(delta("c1", "orphan"));
    let (calls, errors) = acc.finish(false);
    assert!(calls.is_empty(), "the fragment never becomes a call");
    assert_eq!(errors.len(), 1);
    assert!(matches!(&errors[0], ProtocolError::DeltaWithoutStart { call_id } if call_id == "c1"));
}

#[test]
fn end_without_a_start_is_a_protocol_error() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(StreamEvent::ToolCallEnd {
        call_id: "c1".into(),
    });
    let (calls, errors) = acc.finish(false);
    assert!(calls.is_empty());
    assert!(matches!(&errors[..], [ProtocolError::EndWithoutStart { call_id }] if call_id == "c1"));
}

#[test]
fn two_starts_for_one_id_record_an_error() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "read"));
    acc.handle(start("c1", "grep"));
    acc.handle(StreamEvent::ToolCallEnd {
        call_id: "c1".into(),
    });
    let (calls, errors) = acc.finish(false);
    assert_eq!(errors.len(), 1);
    assert!(matches!(&errors[0], ProtocolError::DuplicateStart { call_id } if call_id == "c1"));
    assert_eq!(calls.len(), 1, "one call survives, with the last name");
    assert_eq!(calls[0].name, "grep");
}

// The flows document: a stream that ends with a tool call still open is a
// protocol error; the call is discarded rather than run with partial
// arguments.
#[test]
fn stream_ending_with_an_open_call_discards_it() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "shell"));
    acc.handle(delta("c1", r#"{"command":"ech"#));
    let (calls, errors) = acc.finish(true);
    assert!(calls.is_empty(), "partial arguments never run");
    assert!(errors.iter().any(
        |e| matches!(e, ProtocolError::StreamEndedWithOpenCall { call_id } if call_id == "c1")
    ));
}

#[test]
fn one_stream_can_carry_several_calls() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "read"));
    acc.handle(delta("c1", r#"{"path":"a"}"#));
    acc.handle(StreamEvent::ToolCallEnd {
        call_id: "c1".into(),
    });
    acc.handle(start("c2", "write"));
    acc.handle(delta("c2", r#"{"path":"b"}"#));
    acc.handle(StreamEvent::ToolCallEnd {
        call_id: "c2".into(),
    });
    let (calls, errors) = acc.finish(false);
    assert!(errors.is_empty());
    let ids: Vec<&str> = calls.iter().map(|c| c.call_id.as_str()).collect();
    assert_eq!(ids, vec!["c1", "c2"], "order follows first start");
}

#[test]
fn fragments_that_are_not_json_are_reported_after_joining() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "read"));
    acc.handle(delta("c1", "not json"));
    acc.handle(StreamEvent::ToolCallEnd {
        call_id: "c1".into(),
    });
    let (calls, _) = acc.finish(false);
    assert_eq!(calls.len(), 1);
    assert!(
        lca_provider::validate_arguments(&calls[0]).is_err(),
        "the host parses the joined string (ADR-0004)"
    );
}
