//! Gh #390 gap 1 in the turn loop: a truncated response ends the turn as
//! `Length`, does not dispatch its half-streamed call, and does not re-attempt it.
//!
//! Split from `loop.rs` (gate 11's 1,200-line ceiling): the rows are one subject,
//! so they read better as a file than as a tail.

mod common;

use common::*;
use lca_core::{StopReason, TurnEvent};
use lca_session::ViewMode;
use lca_testkit::{FakeProvider, fake_usage};

// ===========================================================================
// Gh #390 gap 1: a truncation ends the turn, and does not re-attempt the cut call.
// ===========================================================================

// Verifies: gh #390 gap 1 (**the row this issue is about**) - **a response
// truncated mid-tool-call ends the turn as `Length`.** Three things were wrong
// without this: the turn reported a normal stop (so nothing downstream could tell
// an answer was cut), the half-streamed call's JSON-prefix arguments were treated
// as runnable, and - because nothing marked the response as cut - the loop
// re-requested and re-attempted the same incomplete call. A live session showed the
// identical `write a.txt error` card repeating until it was interrupted.
#[tokio::test]
async fn a_truncated_mid_tool_call_ends_the_turn_as_length() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            // Exactly the wire shape: an open call with a partial argument
            // string, then the truncation fact. The budget repeats so a looping
            // turn would be caught rather than merely slowed.
            t.text("Let me write that.")
                .closed_tool_call_with_partial_arguments(
                    "write",
                    "{\"path\": \"a.txt\", \"content\": \"hel",
                )
                .truncated()
                .usage(fake_usage(10, 5, 0, 10))
        })
        .turn(|t| {
            t.text("let me write that.")
                .closed_tool_call_with_partial_arguments(
                    "write",
                    "{\"path\": \"a.txt\", \"content\": \"hel",
                )
                .truncated()
                .usage(fake_usage(10, 5, 0, 10))
        })
        .build();
    let mut h = harness("truncated-mid-tool", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "write a file", &mut sink, &mut prompt).await;
    assert_eq!(
        outcome.stop_reason,
        StopReason::Length,
        "the turn reports the truncation, not a normal stop: {:?}",
        outcome.error
    );
    // The half-streamed call never ran: no tool was dispatched.
    assert_eq!(
        sink.count(|e| matches!(e, TurnEvent::ToolStarted(_))),
        0,
        "a call whose arguments were cut mid-stream must not be dispatched"
    );
    // ...and the turn did not re-request: one call, not one per attempt.
    assert_eq!(
        h.provider.call_count(),
        1,
        "the truncated response is not re-attempted"
    );
    // The partial text is still the user's: it is the usable part.
    assert!(
        sink.texts().contains("Let me write that."),
        "the partial answer survives: {:?}",
        sink.texts()
    );
    assert!(
        sink.events.iter().any(|e| matches!(
            e,
            TurnEvent::TurnEnded {
                stop_reason: StopReason::Length,
                ..
            }
        )),
        "and the interface is told why"
    );
}

// Verifies: gh #390 gap 1 - **a truncation with no tool call in flight** reports
// the same way (the control). Without it, the mid-tool row could be passing for the
// wrong reason - an open-call protocol error rather than the truncation itself.
#[tokio::test]
async fn a_truncated_text_only_response_ends_as_length() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("half an ans")
                .truncated()
                .usage(fake_usage(10, 5, 0, 10))
        })
        .build();
    let mut h = harness("truncated-text", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "say something", &mut sink, &mut prompt).await;
    assert_eq!(outcome.stop_reason, StopReason::Length);
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok, "not a failure");
    assert_eq!(
        sink.texts(),
        "half an ans",
        "the partial answer is delivered"
    );
    assert_eq!(h.provider.call_count(), 1);
}

// Verifies: gh #390 gap 1 - **a truncated response does not poison the next turn.**
// The fact belongs to the turn that was cut; the following one is a fresh request
// and must succeed normally. A flag stored on the agent rather than carried on the
// response would leak.
#[tokio::test]
async fn a_truncated_turn_does_not_poison_the_next_one() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("cut answ")
                .truncated()
                .usage(fake_usage(10, 5, 0, 10))
        })
        .turn(|t| t.text("full answer").usage(fake_usage(10, 5, 0, 10)))
        .build();
    let mut h = harness("truncated-next", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let first = turn(&mut h, "one", &mut sink, &mut prompt).await;
    assert_eq!(first.stop_reason, StopReason::Length);
    let second = turn(&mut h, "two", &mut sink, &mut prompt).await;
    assert_eq!(
        second.stop_reason,
        StopReason::Stop,
        "the next turn is a normal stop"
    );
    assert!(sink.texts().contains("full answer"));
}

// Verifies: gh #390 gap 1 - **a truncated response's call is not persisted.** Its
// arguments are the prefix the model managed to stream, so the JSON is incomplete;
// writing it to the session log would let a resume replay and offer to run it.
#[tokio::test]
async fn a_truncated_responses_call_is_not_persisted() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("writing")
                .closed_tool_call_with_partial_arguments("write", "{\"path\": \"a.txt\"")
                .truncated()
                .usage(fake_usage(10, 5, 0, 10))
        })
        .build();
    let mut h = harness("truncated-persist", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "write", &mut sink, &mut prompt).await;
    assert_eq!(outcome.stop_reason, StopReason::Length);
    // The audit view is the same record stream the log holds, read the way every
    // other row in this file reads it.
    let log = format!(
        "{:?}",
        h.store
            .read_with(&h.session, ViewMode::Audit)
            .expect("the session records")
            .records
    );
    assert!(
        !log.contains("a.txt"),
        "the half-streamed call's arguments never reach the log:\n{log}"
    );
    assert!(
        log.contains("writing"),
        "the text that did arrive is recorded"
    );
}
