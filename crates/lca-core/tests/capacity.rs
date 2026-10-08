//! Turn-loop capacity retries (gh #202): transient overload backs
//! off and resumes; anything else still fails fast.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_core::TurnEvent;
use lca_testkit::{FakeProvider, fake_usage};
// Verifies: gh #202 - a capacity failure retries even when the provider
// marked it non-retryable (the central `is_capacity_error` override);
// the turn succeeds after the backoff instead of ending in error.
#[tokio::test]
async fn a_capacity_failure_retries_despite_a_non_retryable_flag() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.error("Selected model is at capacity", false)
                .usage(fake_usage(1, 0, 0, 0))
        })
        .turn(|t| t.text("recovered").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut h = harness("capacity-retry", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        sink.count(|e| matches!(e, TurnEvent::RetryScheduled { .. })),
        1,
        "one capacity retry scheduled before success"
    );
    assert!(sink.texts().contains("recovered"));
}

// Verifies: gh #202 - a non-capacity failure with a non-retryable flag
// still ends the turn immediately (the override is capacity-only).
#[tokio::test]
async fn a_plain_failure_with_a_non_retryable_flag_still_fails_fast() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.error("invalid api key", false)
                .usage(fake_usage(1, 0, 0, 0))
        })
        .build();
    let mut h = harness("capacity-scope", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Error);
    assert_eq!(
        sink.count(|e| matches!(e, TurnEvent::RetryScheduled { .. })),
        0,
        "no retry for a non-capacity refusal"
    );
}

// Verifies: gh #202 receipt - a mock 529 journey: the provider reports
// HTTP 529 as non-retryable, the turn still backs off and resumes.
#[tokio::test]
async fn a_mock_529_journey_retries_and_resumes() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.error("provider returned HTTP 529: Overloaded", false)
                .usage(fake_usage(1, 0, 0, 0))
        })
        .turn(|t| t.text("resumed").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut h = harness("mock-529", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        sink.count(|e| matches!(e, TurnEvent::RetryScheduled { .. })),
        1,
        "the 529 schedules exactly one retry before resuming"
    );
    assert!(sink.texts().contains("resumed"));
}
