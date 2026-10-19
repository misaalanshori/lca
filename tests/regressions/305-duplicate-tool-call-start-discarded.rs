//! Regression rows for gh #305: a duplicated `ToolCallStart` must poison the
//! call, not silently rebrand it.
//!
//! The defect: `ToolCallAccumulator::handle` recorded `ProtocolError::
//! DuplicateStart` for a second `ToolCallStart` on an open `call_id`, then
//! did `existing.name = name; existing.arguments.clear();`. The follow-up
//! `ToolCallEnd` therefore moved a *truncated* call into `closed` - a call
//! whose arguments were assembled from fragments that belonged to a
//! different (earlier) start. The host ran it. A provider that restarts a
//! call mid-stream consequently produced a tool invocation the model never
//! asked for, holding whatever fragments happened to arrive after the reset.
//!
//! A protocol error was reported either way, so nothing was silent in the
//! logs - the danger was that the turn continued as though the stream were
//! fine.
//!
//! Verifies: gh #305 - a poisoned call is discarded, and the protocol errors
//! still describe what happened.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

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

fn end(call_id: &str) -> StreamEvent {
    StreamEvent::ToolCallEnd {
        call_id: call_id.into(),
    }
}

/// The exact sequence from the issue: the second start arrives after a
/// partial argument, so the joined arguments would be a fragment stitched
/// across a reset boundary.
#[test]
fn the_issue_sequence_yields_no_runnable_call() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("a", "read"));
    acc.handle(delta("a", r#"{"pa"#));
    acc.handle(start("a", "read"));
    acc.handle(delta("a", r#"th":"x"}"#));
    acc.handle(end("a"));
    let (calls, errors) = acc.finish(false);
    assert!(
        calls.is_empty(),
        "a duplicated start discards the call: {} survived as {:?}",
        calls.len(),
        calls
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ProtocolError::DuplicateStart { call_id } if call_id == "a")),
        "the protocol error is still reported: {errors:?}"
    );
}

/// The original shape: two starts with different names, no deltas between.
/// The old code kept the last name and ran it.
#[test]
fn a_duplicated_start_with_a_new_name_does_not_rename_the_call() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "read"));
    acc.handle(start("c1", "grep"));
    acc.handle(delta("c1", r#"{"path":"a"}"#));
    acc.handle(end("c1"));
    let (calls, _) = acc.finish(false);
    assert!(
        calls.is_empty(),
        "neither `read` nor `grep` runs: {:?}",
        calls
    );
}

/// A start with no deltas between the two starts, then an end: the call had
/// no arguments at all, so it would have failed JSON parsing - but it must
/// still be discarded, not reported as a call.
#[test]
fn a_duplicated_start_with_no_arguments_is_still_discarded() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "read"));
    acc.handle(start("c1", "read"));
    acc.handle(end("c1"));
    let (calls, errors) = acc.finish(false);
    assert!(calls.is_empty());
    assert_eq!(
        errors
            .iter()
            .filter(|e| matches!(e, ProtocolError::DuplicateStart { .. }))
            .count(),
        1,
        "one duplicate is one error: {errors:?}"
    );
}

/// A delta after the duplicate start must not become a `DeltaWithoutStart`:
/// the call is still tracked (so the stream's own vocabulary stays
/// consistent), it is just never runnable.
#[test]
fn fragments_after_a_duplicate_start_do_not_add_a_second_error() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "read"));
    acc.handle(start("c1", "read"));
    acc.handle(delta("c1", r#"{"path":"a"}"#));
    acc.handle(end("c1"));
    let (calls, errors) = acc.finish(false);
    assert!(calls.is_empty());
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, ProtocolError::DeltaWithoutStart { .. })),
        "the delta belongs to a tracked (poisoned) call, not an orphan: {errors:?}"
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, ProtocolError::EndWithoutStart { .. })),
        "the end belongs to a tracked (poisoned) call: {errors:?}"
    );
}

/// Poisoning is per call: a healthy call in the same stream still runs.
#[test]
fn a_healthy_call_beside_a_poisoned_one_still_runs() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("bad", "read"));
    acc.handle(start("bad", "read"));
    acc.handle(end("bad"));
    acc.handle(start("good", "read"));
    acc.handle(delta("good", r#"{"path":"a"}"#));
    acc.handle(end("good"));
    let (calls, _) = acc.finish(false);
    assert_eq!(calls.len(), 1, "exactly the healthy call: {calls:?}");
    assert_eq!(calls[0].call_id, "good");
    assert_eq!(calls[0].arguments, r#"{"path":"a"}"#);
}

/// A poisoned call still open at the end of the stream is reported once, not
/// twice: `DuplicateStart` already described the problem, so a second
/// `StreamEndedWithOpenCall` for the same id would double-count it.
#[test]
fn a_poisoned_call_left_open_at_eof_is_not_reported_twice() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "read"));
    acc.handle(start("c1", "read"));
    let (calls, errors) = acc.finish(true);
    assert!(calls.is_empty());
    assert_eq!(
        errors
            .iter()
            .filter(|e| matches!(e, ProtocolError::DuplicateStart { .. }))
            .count(),
        1,
        "one duplicate error: {errors:?}"
    );
    assert_eq!(
        errors
            .iter()
            .filter(|e| matches!(e, ProtocolError::StreamEndedWithOpenCall { .. }))
            .count(),
        0,
        "the duplicate already explained it: {errors:?}"
    );
}

/// A healthy call left open at EOF is still reported - the poisoning rule
/// must not weaken the existing guard.
#[test]
fn a_healthy_call_left_open_at_eof_is_still_reported() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "shell"));
    acc.handle(delta("c1", r#"{"command":"ech"#));
    let (calls, errors) = acc.finish(true);
    assert!(calls.is_empty(), "partial arguments never run");
    assert!(
        errors.iter().any(
            |e| matches!(e, ProtocolError::StreamEndedWithOpenCall { call_id } if call_id == "c1")
        ),
        "the open call is reported: {errors:?}"
    );
}

/// Repeated duplicates of one id produce one error per duplicate, and the
/// call stays discarded throughout.
#[test]
fn three_starts_for_one_id_are_all_poisoned() {
    let mut acc = ToolCallAccumulator::default();
    acc.handle(start("c1", "read"));
    acc.handle(delta("c1", r#"{"pa"#));
    acc.handle(start("c1", "read"));
    acc.handle(delta("c1", r#"th":"#));
    acc.handle(start("c1", "read"));
    acc.handle(delta("c1", r#""x"}"#));
    acc.handle(end("c1"));
    let (calls, errors) = acc.finish(false);
    assert!(calls.is_empty(), "no stitched call: {calls:?}");
    assert_eq!(
        errors
            .iter()
            .filter(|e| matches!(e, ProtocolError::DuplicateStart { .. }))
            .count(),
        2,
        "two duplicates, two errors: {errors:?}"
    );
}
