//! Compaction through the loop: the threshold crossing, the cache baseline it
//! resets, the shipped strategy via the real completion backend, the manual
//! trigger, and a model switch's effect on the backend.
//!
//! Gh #548 phase 4 split `tests/loop_phases.rs` by subject, so each of these rows
//! now reports as `loop_compaction::<row>`, stated in the cycle report.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::phases::PhaseDouble;
use common::phases::phase_config;
use common::phases::registry_with;
use common::phases::scratch_roots;
use common::*;
use lca_core::{Agent, ExtensionRegistry};
use lca_permissions::GrantStore;
use lca_protocol::Record;
use lca_session::{SessionStore, ViewMode};
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::CompletionBackend as _;
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

// Verifies: FR-SESS-4 and FR-SESS-5 (crossing the configured threshold
// invokes the compaction extension, once), FR-CTX-1 (the summary is
// written as a durable record and reused across a restart without
// another invocation), and FR-CACHE-5's recomputation (the stable
// boundary now lands after the summary). Phase 3 built the measurement;
// this is the reset mechanism completing ADR-0017.
#[tokio::test]
async fn crossing_the_threshold_compacts_once_and_the_summary_survives_a_restart() {
    // One big turn (crosses0.5 of a10k window), then small turns.
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(5000, 10, 4000, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(PhaseDouble::strategy("phase-double", calls.clone()));
    let mut h = harness(
        "compact-once",
        provider,
        phase_config(registry, 10_000, 0.5),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };

    // Turn1: no previous usage, nothing crosses.
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing to cross yet");
    // Turn2: turn1's usage crosses -> compact before the provider call.
    turn(&mut h, "second", &mut sink, &mut prompt).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "crossing invokes the strategy exactly once (FR-SESS-4)"
    );
    // Turn3: post-compaction usage is small -> no second invocation
    // (FR-CTX-1: reused until usage next crosses).
    turn(&mut h, "third", &mut sink, &mut prompt).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "reused, not recomputed");

    let records = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records;
    let compactions: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            Record::Compaction {
                summary,
                strategy,
                first_kept_id,
                ..
            } => Some((summary, strategy, first_kept_id)),
            _ => None,
        })
        .collect();
    assert_eq!(compactions.len(), 1, "one durable record (FR-CTX-1)");
    assert_eq!(compactions[0].0, "compacted 2 records", "the range it saw");
    assert!(
        !compactions[0].2.is_empty(),
        "the kept boundary is anchored (gh #36 phase 1)"
    );
    assert_eq!(
        compactions[0].1, "phase-double",
        "FR-SESS-5's strategy name"
    );

    // The summary reaches the model: assembly surfaces it and marks the
    // cache boundary right after it (FR-CACHE-5).
    let assembled = lca_core::assemble(&records, "sys");
    let summary_index = assembled
        .messages
        .iter()
        .position(|message| message.content.iter().any(
            |block| matches!(block, lca_protocol::ContentBlock::Text { text } if text.contains("compacted 2 records")),
        ))
        .expect("the summary is a message the model sees");
    assert_eq!(
        assembled.stable_prefix,
        summary_index + 1,
        "everything through the summary is the stable prefix"
    );

    // Restart: a fresh store over the same directory still shows the
    // summary, and the strategy is not asked again (exit test clause2).
    let store2 = SessionStore::new(h.root.join("data"));
    let session2 = store2
        .session(&h.project, h.session.id())
        .expect("session survives");
    let records2 = store2
        .read_with(&session2, ViewMode::Display)
        .expect("reread")
        .records;
    assert_eq!(
        records2
            .iter()
            .filter(|record| matches!(record, Record::Compaction { .. }))
            .count(),
        1,
        "the record persisted"
    );
    let mut agent = Agent::new(
        &store2,
        &session2,
        h.provider.as_ref(),
        &mut h.tools,
        h.grants.clone(),
        &mut prompt,
        None,
        h.config.clone(),
    );
    let outcome = agent
        .run_turn("after restart", &mut sink, &CancelFlag::new())
        .await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a resumed session reuses the summary (FR-CTX-1)"
    );
}

// Verifies: FR-CACHE-2 end to end - a counted miss before compaction,
// the baseline reset exactly on the compaction record, and zero waste
// on every turn after it (the Phase 3 exit test's third clause).
#[tokio::test]
async fn the_cache_baseline_resets_exactly_on_the_compaction_record() {
    // t1 reports caching (write), t2 reads none of the old prompt: a
    //1500-token miss above the1024 floor, before any compaction. t3
    // crosses the threshold, t4 compacts first, then stays small.
    // (gh #36 phase 1: the trigger is strict `>` on `window -
    // reserve`, so the crossing sits one token past the boundary.)
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(900, 10, 0, 600)))
        .turn(|t| t.text("ok").usage(fake_usage(2000, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(3000, 10, 2001, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(PhaseDouble::strategy("phase-double", calls.clone()));
    let mut h = harness("cache-reset", provider, phase_config(registry, 10_000, 0.5));
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    for index in 0..5 {
        let outcome = turn(&mut h, &format!("turn {index}"), &mut sink, &mut prompt).await;
        assert_eq!(outcome.status, lca_core::TurnStatus::Ok, "turn {index}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "compacted exactly once");

    // Waste is measured over the whole log (audit view): suppressed
    // records still cost real money, and the compaction record itself
    // resets the baseline where it sits (FR-CACHE-2).
    let records = h.store.read(&h.session).expect("records").records;
    let compaction_id = records
        .iter()
        .find_map(|record| match record {
            Record::Compaction { id, .. } => Some(id.clone()),
            _ => None,
        })
        .expect("a compaction record");
    let misses = lca_session::collect_cache_misses(&records, 1024);
    assert!(
        !misses.is_empty(),
        "the scripted miss before compaction was counted"
    );
    for miss in &misses {
        assert!(
            miss.record_id < compaction_id,
            "miss {} landed after the compaction record {}",
            miss.record_id,
            compaction_id
        );
    }
    // The totals a user sees reset with it: nothing after the record
    // counted (FR-CACHE-1's zero-from-here, measured on the records).
    let after: Vec<Record> = records
        .iter()
        .skip_while(|record| record.id() != Some(compaction_id.as_str()))
        .cloned()
        .collect();
    assert!(
        lca_session::collect_cache_misses(&after, 1024).is_empty(),
        "zero waste on every turn after (exit test clause3)"
    );
}

// Verifies: the Phase 4 exit test's first two clauses against the REAL
// default strategy and the REAL completion backend (not doubles):
// crossing the configured threshold triggers `compaction-default`, it
// asks the active provider through the `completion` capability, the
// model's answer is the summary, and the summarization usage lands on
// the durable record (capability catalog: spend shows in session cost).
#[tokio::test]
async fn the_default_strategy_compacts_through_the_real_completion_backend() {
    // Script order: turn1's reply (big usage - the crossing), the
    // summarization the strategy will ask for, then small replies.
    let provider = FakeProvider::builder()
        .turn(|t| t.text("first reply").usage(fake_usage(5000, 40, 4000, 0)))
        .turn(|t| {
            t.text("SYNTHESIS: the parser was added and tested.")
                .usage(fake_usage(1000, 40, 4000, 0))
        })
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let provider = Arc::new(provider);
    // The completion backend wraps the SAME provider the agent talks
    // to: the capability's star topology with the host at the center.
    let backend = Arc::new(lca_core::ext_provider::ProviderBackend::new(
        provider.clone(),
        "faux-1",
        "session-under-test",
    ));
    let cap = Arc::new(lca_tools::Capabilities::new(
        "compaction-default",
        compaction_default::manifest_grants(),
        scratch_roots("default-strategy"),
        Arc::new(std::sync::Mutex::new(Prompt {
            answers: Vec::new(),
            asked: Vec::new(),
        })),
        Arc::new(std::sync::Mutex::new(
            GrantStore::open(&scratch("default-strategy-grants").join("grants.json"))
                .expect("grants"),
        )),
        scratch("default-strategy-project"),
        None,
    ));
    cap.set_completion(backend.clone());
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(compaction_default::CompactionDefault::new(
        cap.clone(),
    )));
    let mut config = phase_config(registry, 10_000, 0.5);
    config.completion_backend = Some(backend.clone());

    let root = scratch("default-strategy");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let mut tools = ToolExecutor::new(
        Arc::new(NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Some(Duration::from_secs(30)),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    // Turn1: no previous usage, nothing crosses; its reply's usage is
    // what the next turn measures.
    let outcome = {
        let mut agent = Agent::new(
            &store,
            &session,
            provider.as_ref(),
            &mut tools,
            grants.clone(),
            &mut prompt,
            None,
            config.clone(),
        );
        agent
            .run_turn("big first turn", &mut sink, &CancelFlag::new())
            .await
    };
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "{:?}",
        outcome.error
    );

    // Turn2: turn1's usage crosses the threshold; the default strategy
    // asks the active provider through `completion` for the summary.
    let outcome = {
        let mut agent = Agent::new(
            &store,
            &session,
            provider.as_ref(),
            &mut tools,
            grants.clone(),
            &mut prompt,
            None,
            config.clone(),
        );
        agent
            .run_turn("second turn", &mut sink, &CancelFlag::new())
            .await
    };
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "{:?}",
        outcome.error
    );

    let records = store
        .read_with(&session, ViewMode::Display)
        .expect("records")
        .records;
    let compaction = records
        .iter()
        .find_map(|record| match record {
            Record::Compaction {
                summary,
                strategy,
                usage,
                replaced_from,
                replaced_to,
                ..
            } => Some((
                summary.clone(),
                strategy.clone(),
                usage.clone(),
                replaced_from.clone(),
                replaced_to.clone(),
            )),
            _ => None,
        })
        .expect("the threshold triggered the default strategy (FR-SESS-4)");
    assert_eq!(
        compaction.0, "SYNTHESIS: the parser was added and tested.",
        "the model's answer is the summary"
    );
    assert_eq!(
        compaction.1, "compaction-default",
        "FR-SESS-5's strategy name"
    );
    let usage = compaction
        .2
        .expect("the summarization usage is on the record");
    assert_eq!(
        usage.input, 1000,
        "billed input excludes the fake's cache read"
    );
    assert_eq!(
        usage.cache_read, 4000,
        "the completion call's cache fields carried"
    );
    assert!(!compaction.3.is_empty(), "a range was named");
    assert_ne!(compaction.3, compaction.4, "from != to");

    // Turn3: reuses it - no second invocation, no recompute (FR-CTX-1,
    // exit clause2).
    let outcome = {
        let mut agent = Agent::new(
            &store,
            &session,
            provider.as_ref(),
            &mut tools,
            grants.clone(),
            &mut prompt,
            None,
            config.clone(),
        );
        agent
            .run_turn("third turn", &mut sink, &CancelFlag::new())
            .await
    };
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let records = store
        .read_with(&session, ViewMode::Display)
        .expect("records")
        .records;
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r, Record::Compaction { .. }))
            .count(),
        1,
        "the summary is reused (exit clause2)"
    );

    // The backend drained: a later compaction would attribute its own
    // usage fresh (spend lands per record, not cumulatively).
    assert!(
        backend.take_usage().is_none(),
        "usage was drained onto the record"
    );
}

// Verifies: FR-SESS-5 (the manual trigger - the /compact built-in's
// engine - compacts exclusively through the compaction world: the
// strategy runs, the durable record lands with its name, and no
// threshold was crossed to get there - the window here is zero, which
// disables the automatic path entirely).
#[tokio::test]
async fn the_manual_trigger_compacts_through_the_world_without_a_threshold() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(5000, 10, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(PhaseDouble::strategy("phase-double", calls.clone()));
    // window zero: the threshold path (FR-SESS-4) can never fire, so
    // every invocation below is the manual one's doing.
    let mut h = harness("manual-compact", provider, phase_config(registry, 0, 0.0));
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "zero window: no automatic compaction"
    );

    let store = std::sync::Arc::new(SessionStore::new(h.root.join("data")));
    let session = store.session(&h.project, h.session.id()).expect("reattach");
    let summary = lca_core::compact_now(
        store.clone(),
        session.clone(),
        h.config.extensions.clone(),
        h.config.completion_backend.clone(),
        Some(h.config.system_prompt.clone()),
    )
    .expect("manual compaction runs");
    assert!(
        summary.contains("compacted"),
        "the strategy's summary comes back: {summary}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the compaction world ran, once"
    );

    let records = store
        .read_with(&session, ViewMode::Display)
        .expect("read")
        .records;
    let named: Vec<&str> = records
        .iter()
        .filter_map(|record| match record {
            Record::Compaction { strategy, .. } => Some(strategy.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(named, ["phase-double"], "FR-SESS-5's strategy name");

    // A session with nothing worth replacing answers an error and
    // never reaches the strategy.
    let fresh = store.create_session(&h.project, "fresh").expect("fresh");
    let err = lca_core::compact_now(
        store.clone(),
        fresh,
        h.config.extensions.clone(),
        h.config.completion_backend.clone(),
        None,
    )
    .expect_err("one message cannot compact into itself");
    assert!(!err.is_empty(), "the refusal explains itself");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the empty session never reached the strategy"
    );
}

// The completion backend asks whichever model the session currently
// uses (ProviderBackend's own contract); /model changes that model for
// the session, compaction included. Called sync, the way the
// compaction strategy reaches it: through a blocking region
// (ADR-0014), never from inside an async poll.
#[test]
fn the_completion_backend_follows_a_model_change() {
    let fake = Arc::new(
        FakeProvider::builder()
            .turn(|t| t.text("hi").usage(fake_usage(10, 2, 0, 0)))
            .turn(|t| t.text("hi again").usage(fake_usage(8, 2, 0, 0)))
            .build(),
    );
    let backend =
        lca_core::ext_provider::ProviderBackend::new(fake.clone(), "startup-model", "sess-1");
    let messages = vec![lca_protocol::ChatMessage::text(
        lca_protocol::MessageRole::User,
        "hello",
    )];
    backend
        .complete(&messages)
        .expect("the scripted turn answers");
    assert_eq!(
        fake.last_request().expect("request").model,
        "startup-model",
        "before any change"
    );

    backend.set_model("switched-model");
    backend
        .complete(&messages)
        .expect("the scripted turn answers");
    assert_eq!(
        fake.last_request().expect("request").model,
        "switched-model",
        "the session's model after /model"
    );
}
