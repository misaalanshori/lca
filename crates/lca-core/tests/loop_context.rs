//! The context chain: the skills merge, a transform rejection, the stable
//! boundary narrowed by rewritten content, a plain conversation that never
//! diverges, and tool arguments validated before dispatch.
//!
//! Gh #548 phase 4 split `tests/loop_phases.rs` by subject, so each of these rows
//! now reports as `loop_context::<row>`, stated in the cycle report.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::phases::PhaseDouble;
use common::phases::TransformKind;
use common::phases::count_extension_events;
use common::phases::phase_config;
use common::phases::registry_two;
use common::phases::registry_with;
use common::*;
use lca_core::{ExtensionRegistry, TurnEvent};
use lca_protocol::{Record, ToolResultStatus};
use lca_session::ViewMode;
use lca_testkit::{FakeProvider, fake_usage};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

// Verifies: FR-CTX-2 (the chain runs before the provider call) and the
// Phase 4 exit clause4: skills handling injects matched instructions
// through the chain on an ordinary turn without affecting the cache
// boundary - the boundary is the one FR-CACHE-5 computes from the most
// recent compaction record, and the injection rides outside it.
#[tokio::test]
async fn skills_inject_through_the_chain_without_moving_the_cache_boundary() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(5000, 10, 4000, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let strategy_calls = Arc::new(AtomicUsize::new(0));
    let transform_calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_two(
        PhaseDouble::strategy("phase-double", strategy_calls.clone()),
        PhaseDouble::transform(
            "skills",
            TransformKind::SkillsInject,
            transform_calls.clone(),
        ),
    );
    let mut h = harness(
        "skills-inject",
        provider,
        phase_config(registry, 10_000, 0.5),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    // Turn2 crosses the threshold and compacts first; the boundary is
    // everything through the summary (FR-CACHE-5).
    turn(&mut h, "second", &mut sink, &mut prompt).await;
    turn(&mut h, "third", &mut sink, &mut prompt).await;
    assert_eq!(strategy_calls.load(Ordering::SeqCst), 1, "compacted once");
    assert!(
        transform_calls.load(Ordering::SeqCst) >= 3,
        "every provider call transforms"
    );

    let request = h.provider.last_request().expect("the last request");
    let records = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records;
    let natural = lca_core::assemble(&records, &h.config.system_prompt);
    assert_eq!(
        request.stable_prefix, natural.stable_prefix,
        "the injection did not move the boundary (exit clause4)"
    );
    assert!(request.stable_prefix >= 2, "a boundary exists to protect");
    let marker = "[skill: test-skill]";
    assert!(
        request.messages[request.stable_prefix..]
            .iter()
            .any(|message| message.content.iter().any(
                |block| matches!(block, lca_protocol::ContentBlock::Text { text } if text.contains(marker))
            )),
        "the injection rode in the un-stable region"
    );
    assert!(
        request.messages[..request.stable_prefix]
            .iter()
            .all(|message| message.content.iter().all(
                |block| matches!(block, lca_protocol::ContentBlock::Text { text } if !text.contains(marker))
            )),
        "the stable region is byte-identical"
    );
    assert_eq!(
        request.messages[..request.stable_prefix],
        natural.messages[..natural.stable_prefix],
        "the stable region matches an untransformed assembly"
    );
}

// Verifies: FR-CTX-3 and the Phase 4 exit clause5 - a rejection ends
// the turn with the reason surfaced and the provider never called
// (FR-CTX-4: nothing of the transform reaches the log either).
#[tokio::test]
async fn a_transform_rejection_ends_the_turn_before_the_provider() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("never sent").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(PhaseDouble::transform(
        "rejecter",
        TransformKind::Reject("skills policy: refused this request"),
        calls.clone(),
    ));
    let mut h = harness(
        "reject-turn",
        provider,
        phase_config(registry, 100_000, 0.9),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    let outcome = turn(&mut h, "hello", &mut sink, &mut prompt).await;
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Error,
        "the turn ended"
    );
    assert_eq!(outcome.stop_reason, lca_core::StopReason::Error);
    let error = outcome.error.expect("the reason is surfaced");
    assert!(error.contains("skills policy: refused"), "{error}");
    assert_eq!(
        h.provider.call_count(),
        0,
        "the provider was never called (FR-CTX-3)"
    );
    assert!(
        sink.events.iter().any(|event| matches!(
            event,
            TurnEvent::TurnEnded {
                status: lca_core::TurnStatus::Error,
                ..
            }
        )),
        "the interface saw the turn end"
    );
    let records = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records;
    assert!(
        !records
            .iter()
            .any(|record| matches!(record, Record::Assistant { .. } | Record::ToolCall { .. })),
        "nothing the transform touched was persisted (FR-CTX-4)"
    );
}

// Verifies: FR-CACHE-6 - content inside the stable boundary that
// differs from what actually went out on the previous call narrows the
// boundary to end before the earliest differing message, records one
// extension event rather than rejecting, and settles (no further
// narrowing, no repeat event) once the output stops changing.
#[tokio::test]
async fn stable_region_divergence_narrows_the_boundary_once_and_settles() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(5000, 10, 4000, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let strategy_calls = Arc::new(AtomicUsize::new(0));
    let transform_calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_two(
        PhaseDouble::strategy("phase-double", strategy_calls.clone()),
        PhaseDouble::transform(
            "narrower",
            TransformKind::NarrowFromSecond,
            transform_calls.clone(),
        ),
    );
    let mut h = harness(
        "narrow-boundary",
        provider,
        phase_config(registry, 10_000, 0.5),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };

    // Turn1: no compaction record yet, so the boundary reaches up to
    // (but never includes) this turn's own message: just the system
    // prompt side.
    let outcome = turn(&mut h, "first", &mut sink, &mut prompt).await;
    eprintln!("T1 {:?}", (outcome.status, outcome.error));
    let first = h.provider.last_request().expect("request one");
    assert_eq!(
        first.stable_prefix, 1,
        "grows turn by turn, excludes the new message"
    );

    // Turn2: threshold crossing compacts first; the transform's first
    // rewrite lands on the summary, but nothing was sent before, so
    // there is nothing to compare against yet.
    let outcome = turn(&mut h, "second", &mut sink, &mut prompt).await;
    eprintln!("T2 {:?}", (outcome.status, outcome.error));
    let second = h.provider.last_request().expect("request two");
    assert_eq!(
        second.stable_prefix, 1,
        "post-compaction the boundary still never claims this turn's message"
    );
    assert_eq!(
        count_extension_events(&h, "cache-boundary-narrowed"),
        0,
        "the rewritten summary lies outside the claimed boundary so far"
    );

    // Turn3: the transform stops rewriting, so the stable region now
    // differs from what actually went out on turn2 - narrowed once,
    // recorded once, and the turn still succeeds (never a rejection).
    let outcome = turn(&mut h, "third", &mut sink, &mut prompt).await;
    eprintln!("T3 {:?}", (outcome.status, outcome.error));
    let third = h.provider.last_request().expect("request three");
    // The summary is now claimable and was sent rewritten on turn2 but
    // arrives clean on turn3: the boundary ends before it (FR-CACHE-6).
    assert_eq!(
        third.stable_prefix, 2,
        "the boundary ends before the message that diverged (FR-CACHE-6)"
    );
    assert_eq!(
        count_extension_events(&h, "cache-boundary-narrowed"),
        1,
        "one divergence recorded as an extension event"
    );

    // Turn4: settled - same content as last sent, narrowed no further.
    let outcome = turn(&mut h, "fourth", &mut sink, &mut prompt).await;
    eprintln!("T4 {:?}", (outcome.status, outcome.error));
    let fourth = h.provider.last_request().expect("request four");
    // SANCTIONED CHANGE (gh #275): this was `3`, frozen at the compaction
    // summary. `stable_cap` no longer clamps to `assembled.stable_prefix`
    // forever after a compaction, so the boundary advances across the
    // post-compaction conversation: on turn 4 the previous request had sent
    // 5 messages through turn 3's user message, and all 5 still match, so 5
    // is the claimable prefix. `3` was the defect - everything after the
    // summary was permanently un-cacheable.
    assert_eq!(
        fourth.stable_prefix, 5,
        "post-compaction the boundary advances across what was already sent"
    );
    assert_eq!(
        count_extension_events(&h, "cache-boundary-narrowed"),
        1,
        "no repeat event once settled"
    );
    // Raw log: the compacted turn's assistant is hidden from the
    // display view by design, but every turn did succeed.
    let records = h.store.read(&h.session).expect("records").records;
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record, Record::Assistant { .. }))
            .count(),
        4,
        "every turn kept succeeding: divergence records, never rejects"
    );
}

// Verifies: FR-CACHE-6 - a plain conversation (no transform, no compaction)
// never reports a boundary divergence. The boundary legitimately ends at the
// previous request, because the provider cached only that; messages appended
// since then are growth, not a divergence, and must not raise the
// `cache-boundary-narrowed` event on every turn.
#[tokio::test]
async fn a_plain_conversation_never_reports_a_boundary_divergence() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("one").usage(fake_usage(100, 10, 0, 0)))
        .turn(|t| t.text("two").usage(fake_usage(120, 10, 100, 0)))
        .turn(|t| t.text("three").usage(fake_usage(140, 10, 120, 0)))
        .build();
    let mut h = harness(
        "plain-boundary",
        provider,
        phase_config(ExtensionRegistry::new(), 100_000, 0.99),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    for input in ["first", "second", "third"] {
        let outcome = turn(&mut h, input, &mut sink, &mut prompt).await;
        assert_eq!(
            outcome.status,
            lca_core::TurnStatus::Ok,
            "turn {input}: {:?}",
            outcome.error
        );
    }
    assert_eq!(
        count_extension_events(&h, "cache-boundary-narrowed"),
        0,
        "a plain conversation must not report a divergence"
    );
    let last = h.provider.last_request().expect("a request");
    assert!(
        last.stable_prefix > 0,
        "the boundary grows with the conversation"
    );
    assert!(
        last.stable_prefix < last.messages.len(),
        "and never claims the current message"
    );
}

// Verifies: the host validates a tool call's arguments against its schema
// before executing it (extension authoring guide) - an invalid call never
// reaches the tool and no permission prompt is shown for it.
#[tokio::test]
async fn invalid_tool_arguments_never_reach_the_tool() {
    let provider = FakeProvider::builder()
        .turn(|t| t.tool_call("read", "{}").usage(fake_usage(10, 10, 0, 10)))
        .turn(|t| t.text("ok").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("schema", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "read nothing", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert!(
        prompt.asked.is_empty(),
        "no prompt for an invalid call: {:?}",
        prompt.asked
    );
    let results: Vec<_> = sink
        .events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::ToolFinished(result) => Some(result.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, ToolResultStatus::Error);
    assert!(
        results[0].content.contains("invalid arguments"),
        "{}",
        results[0].content
    );
}
