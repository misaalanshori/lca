//! Retries and their exhaustion, the iteration limit, cancellation, the
//! workspace-boundary refusal, the provider-purity rows, and the NFR-31
//! cache-witness benchmark.
//!
//! Gh #548 phase 4 split `tests/loop.rs` by subject, so each of these rows now
//! reports as `loop_limits::<row>`, stated in the cycle report.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_core::{Agent, AgentConfig, StopReason, TurnEvent};
use lca_protocol::{Record, ToolResultStatus};
use lca_session::ViewMode;
use lca_testkit::{FakeProvider, Provider, fake_usage};
use lca_tools::CancelFlag;
use std::time::Duration;

// Verifies: FR-CORE-6 (retryable transport errors retry with backoff up to
// the configured limit)
#[tokio::test]
async fn retries_retryable_errors_up_to_the_limit() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.error("connection reset", true)
                .usage(fake_usage(1, 0, 0, 0))
        })
        .turn(|t| {
            t.error("connection reset", true)
                .usage(fake_usage(1, 0, 0, 0))
        })
        .turn(|t| t.text("finally").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut h = harness("retry", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        sink.count(|e| matches!(e, TurnEvent::RetryScheduled { .. })),
        2,
        "two retries scheduled before success"
    );
    assert!(sink.texts().contains("finally"));
    match sink.events.iter().find_map(|e| match e {
        TurnEvent::RetryScheduled { attempt, max, .. } => Some((*attempt, *max)),
        _ => None,
    }) {
        Some((1, 3)) => {}
        other => panic!("first retry is attempt 1 of 3, got {other:?}"),
    }
}

// Verifies: FR-CORE-7 (after the retry limit the error shows and control
// returns; the session stays usable)
#[tokio::test]
async fn surfaces_the_error_after_exhausting_retries_and_keeps_the_session() {
    let provider = FakeProvider::builder()
        .turn(|t| t.error("gateway down", true).usage(fake_usage(1, 0, 0, 0)))
        .turn(|t| t.error("gateway down", true).usage(fake_usage(1, 0, 0, 0)))
        .turn(|t| t.text("recovered").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut h = harness("retry-exhaust", provider, {
        let mut config = default_config();
        config.retry_limit = 1;
        config
    });
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Error);
    assert_eq!(outcome.stop_reason, StopReason::Error);
    assert!(
        outcome
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("gateway down")
    );
    assert!(
        sink.events
            .iter()
            .any(|e| matches!(e, TurnEvent::Error { .. })),
        "the error is shown to the interface"
    );

    // The session is still open: the next turn works (FR-CORE-7).
    let outcome = turn(&mut h, "try again", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok, "session survives");
    assert!(sink.texts().contains("recovered"));
}

// Verifies: FR-CORE-9 (a turn past the tool-call iteration limit ends with
// an iteration-limit error)
#[tokio::test]
async fn ends_with_an_iteration_limit_error() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("write", r#"{"path":"a.txt","content":"A"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| {
            t.tool_call("write", r#"{"path":"b.txt","content":"B"}"#)
                .usage(fake_usage(20, 10, 10, 10))
        })
        .turn(|t| t.text("never reached").usage(fake_usage(30, 5, 10, 0)))
        .build();
    let mut h = harness("iteration", provider, {
        let mut config = default_config();
        config.max_iterations = 1;
        config
    });
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "loop", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Error);
    assert_eq!(outcome.stop_reason, StopReason::IterationLimit);
    assert_eq!(h.provider.call_count(), 2, "no third provider call");
    let workspace = &h.project;
    assert!(workspace.join("a.txt").is_file(), "first round ran");
    assert!(!workspace.join("b.txt").exists(), "second round never ran");
}

// Verifies: FR-CORE-9's 2026-10-02 annotation - with the default config
// (max_iterations = 0 = unlimited) a turn runs past100 tool rounds to a
// natural finish. The configured path is covered by
// `ends_with_an_iteration_limit_error` above (explicit limit still stops).
#[tokio::test]
async fn the_default_config_runs_past_one_hundred_tool_rounds() {
    let mut provider = FakeProvider::builder();
    for round in 0..101 {
        provider = provider.turn(move |t| {
            t.tool_call(
                "write",
                &format!(r#"{{"path":"round.txt","content":"{round}"}}"#),
            )
            .usage(fake_usage(1, 1, 0, 1))
        });
    }
    let provider = provider
        .turn(|t| {
            t.text("finished one hundred and one rounds")
                .usage(fake_usage(1, 1, 0, 0))
        })
        .build();
    // The default under test is `AgentConfig::default`'s: everything else
    // the harness needs, nothing about the cap.
    let mut h = harness(
        "default-iterations",
        provider,
        AgentConfig {
            provider: "fake".to_string(),
            model: "faux-1".to_string(),
            retry_limit: 3,
            retry_base_delay: Duration::ZERO,
            ..AgentConfig::default()
        },
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "keep going", &mut sink, &mut prompt).await;
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "the default does not brake: {:?}",
        outcome.error
    );
    assert_ne!(outcome.stop_reason, StopReason::IterationLimit);
    assert!(sink.texts().contains("finished one hundred and one rounds"));
    assert_eq!(
        h.provider.call_count(),
        102,
        "101 tool rounds plus the final answer"
    );
}

// Verifies: FR-CORE-5 and FR-CONC-3 (cancel stops the in-flight request;
// completed session records are kept)
#[tokio::test]
async fn cancellation_stops_the_stream_and_keeps_completed_records() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("first")
                .pause(5_000)
                .text("second")
                .usage(fake_usage(10, 5, 0, 10))
        })
        .build();
    let mut h = harness("cancel", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let cancel = CancelFlag::new();
    let canceller = {
        let flag = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            flag.cancel();
        })
    };
    let mut agent = Agent::new(
        &h.store,
        &h.session,
        h.provider.as_ref(),
        &mut h.tools,
        h.grants.clone(),
        &mut prompt,
        None,
        h.config.clone(),
    );
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        agent.run_turn("hi", &mut sink, &cancel),
    )
    .await
    .expect("returns promptly");
    canceller.await.expect("canceller");
    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    assert!(sink.texts().contains("first"), "received before cancel");

    let read = h.store.read(&h.session).expect("read");
    assert!(
        read.records
            .iter()
            .any(|r| matches!(r, Record::User { .. })),
        "the user record survives (FR-CORE-5)"
    );
    assert!(
        !read
            .records
            .iter()
            .any(|r| matches!(r, Record::Assistant { .. })),
        "no half-written assistant record"
    );
}

// Verifies: FR-PERM-27 (ADR-0042) - a read outside the workspace does not
// prompt in ask mode and leaves no permission record, while a deny rule
// still refuses it without prompting and records the refusal.
#[tokio::test]
async fn reading_outside_the_workspace_does_not_prompt_unless_a_rule_denies() {
    // Two scripted turns: an outside read, then a text reply; repeated for
    // the deny-rule half of the test.
    fn read_turn(t: lca_testkit::TurnBuilder) -> lca_testkit::TurnBuilder {
        t.tool_call("read", r#"{"path":"../outside.txt"}"#)
            .usage(fake_usage(10, 10, 0, 10))
    }
    let provider = FakeProvider::builder()
        .turn(read_turn)
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 10, 0)))
        .turn(read_turn)
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("read-out", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    // No rule: the read runs, nothing is asked, nothing is recorded.
    let outcome = turn(&mut h, "read it", &mut sink, &mut prompt).await;
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "error: {:?}",
        outcome.error
    );
    assert!(
        prompt.asked.is_empty(),
        "a read outside the workspace does not prompt: {:?}",
        prompt.asked
    );
    let log = h.store.read(&h.session).expect("read");
    assert!(
        !log.records
            .iter()
            .any(|r| matches!(r, Record::Permission { .. })),
        "no user decision was made, so none is recorded: {:?}",
        log.records
    );

    // A project deny rule still refuses it, without prompting, and the
    // refusal is recorded like any rule denial.
    let project = h.project.clone();
    h.grants
        .lock()
        .expect("grants")
        .add_rule(
            &project,
            lca_permissions::RuleScope::Project,
            lca_permissions::RuleDecision::Deny,
            "*outside.txt",
        )
        .expect("deny rule");
    let mut sink = CollectingSink::default();
    let outcome = turn(&mut h, "read it again", &mut sink, &mut prompt).await;
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "error: {:?}",
        outcome.error
    );
    assert!(
        prompt.asked.is_empty(),
        "a deny rule refuses without prompting: {:?}",
        prompt.asked
    );
    assert!(
        sink.events.iter().any(
            |e| matches!(e, TurnEvent::ToolFinished(r) if r.status == ToolResultStatus::Denied)
        ),
        "the denial reaches the model"
    );
    let log = h.store.read(&h.session).expect("read");
    assert!(
        log.records.iter().any(|r| matches!(
            r,
            Record::Permission {
                decision: lca_protocol::PermissionDecision::Denied,
                ..
            }
        )),
        "the rule denial is recorded"
    );
}

// Verifies: FR-CACHE-5 (the stable-prefix boundary travels on every
// completion call; without a compaction record the dynamic suffix starts at
// the session start, so the prefix is zero)
#[tokio::test]
async fn passes_the_stable_prefix_on_every_completion_call() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("one").usage(fake_usage(10, 5, 0, 10)))
        .turn(|t| t.text("two").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("prefix", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    turn(&mut h, "first", &mut sink, &mut prompt).await;
    turn(&mut h, "second", &mut sink, &mut prompt).await;

    let request = h.provider.last_request().expect("request recorded");
    // With no compaction record the boundary grows up to (but never
    // includes) the current turn's message: [system, first] was sent
    // last call, `second` is new (the growth docs/providers/
    // antigravity.md relies on; a compaction record then anchors it).
    assert_eq!(request.stable_prefix, 2, "grows to the previous message");
    assert_eq!(request.model, "faux-1");
    let roles: Vec<&str> = request
        .messages
        .iter()
        .map(|m| match m.role {
            lca_protocol::MessageRole::System => "system",
            lca_protocol::MessageRole::User => "user",
            lca_protocol::MessageRole::Assistant => "assistant",
            lca_protocol::MessageRole::Tool => "tool",
        })
        .collect();
    assert_eq!(roles.first(), Some(&"system"), "system message leads");
    assert_eq!(roles.last(), Some(&"user"), "the new user message is last");
    assert!(
        !request.tools.is_empty(),
        "tool specs travel with the request"
    );
    let _ = h.provider.name();
}

// Verifies: FR-PROV-1 (the core contains no vendor-specific model logic:
// no vendor endpoints, model families, or wire formats appear in it; the
// provider name it defaults to is configuration, not logic)
#[test]
fn core_has_no_vendor_specific_model_logic() {
    let source = include_str!("../src/lib.rs").to_lowercase();
    for vendor_signal in [
        "api.openai.com",
        "anthropic",
        "gpt-4",
        "claude-",
        "chatgpt",
        "generativelanguage",
    ] {
        assert!(
            !source.contains(vendor_signal),
            "vendor signal `{vendor_signal}` in lca-core"
        );
    }
}

// Verifies: FR-PROV-2 (where a provider extension is enabled, its models
// are listed for the picker)
#[test]
fn model_listings_flow_from_the_provider() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("x").usage(fake_usage(1, 1, 0, 0)))
        .build();
    let models = Provider::list_models(&provider);
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "faux-1");
}

#[tokio::test]
async fn twenty_clean_turns_report_zero_cache_waste_and_hold_the_ratio() {
    // The canonical script: turn one writes the cache, every later turn
    // reads the whole previous prompt and pays for a little new input.
    let mut builder = FakeProvider::builder();
    let mut previous_prompt = 0u64;
    for turn in 0..20u64 {
        let (input, cache_read, cache_write) = if turn == 0 {
            (1000, 0, 500)
        } else {
            (50, previous_prompt, 0)
        };
        previous_prompt = input + cache_read + cache_write;
        let usage = fake_usage(input, 10, cache_read, cache_write);
        builder = builder.turn(move |t| t.text("ok").usage(usage));
    }
    let provider = builder.build();
    let mut h = harness("cache-ratio", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    for index in 0..20 {
        let outcome = turn(&mut h, &format!("turn {index}"), &mut sink, &mut prompt).await;
        assert_eq!(
            outcome.status,
            lca_core::TurnStatus::Ok,
            "turn {index} completes"
        );
    }

    let records = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records;

    // Zero waste from the second turn onward: no miss anywhere (the
    // first turn has no predecessor, so it can never count).
    let totals = lca_session::compute_cache_waste(&records, 1024);
    assert_eq!(totals.miss_count, 0, "no counted miss: {totals:?}");
    assert_eq!(totals.missed_tokens, 0, "zero wasted tokens: {totals:?}");

    // The ratio itself: cache reads over total prompt, turns2..20.
    let mut reads = 0u64;
    let mut prompts = 0u64;
    let mut turns = 0u64;
    for record in &records {
        if let Record::Assistant {
            usage: Some(usage), ..
        } = record
        {
            let prompt = usage.input + usage.cache_read + usage.cache_write + usage.cache_write_1h;
            if turns > 0 {
                // skip the first turn (measured "from turn two onward")
                reads += usage.cache_read;
                prompts += prompt;
            }
            turns += 1;
        }
    }
    assert!(turns >= 20, "twenty scripted turns recorded: {turns}");
    assert!(prompts > 0, "the conversation had prompt tokens");
    let ratio = reads as f64 / prompts as f64;
    assert!(
        ratio >= NFR31_MIN_CACHE_HIT_RATIO,
        "cache-hit ratio {ratio:.4} below the NFR-31 threshold {NFR31_MIN_CACHE_HIT_RATIO}"
    );
}

// Verifies: NFR-31 (the cache-hit-ratio benchmark) and the Phase 3
// exit test's clause - a scripted clean twenty-turn conversation with
// no compaction and no provider change reports zero cache waste from
// the second turn onward (FR-CACHE-1's absence of misses, ADR-0017's
// whole mechanism against the fake provider's scripted usage).
//
// The threshold was fixed at this Phase 3 exit test, as
// `docs/testing-plan.md` section9 requires: the clean script scores
// about0.97 (each turn's new tokens are paid, the rest comes from the
// cache), so anything under0.90 means caching stopped working - which
// produces a correct answer and a passing turn every time, and shows up
// here instead of on the bill.
const NFR31_MIN_CACHE_HIT_RATIO: f64 = 0.90;
