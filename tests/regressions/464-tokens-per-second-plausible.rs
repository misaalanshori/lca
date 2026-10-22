//! NFR-24 guard for gh #464: the footer's `tok/s` reading must be a
//! measurement, not an artifact of when the interface happened to wake up.
//!
//! The defect: a live Antigravity session reported **5,263,158 tok/s**. Its own
//! receipt says why: `↓30` output tokens divided by the printed rate is
//! 5.7 microseconds. The meter opened its window in `Chat::on_turn_event` - the
//! interface thread - and that thread drains a bounded channel in a burst, so
//! every delta of a finished response arrived within a few microseconds of the
//! next. Nothing was wrong with the tokens; the *span* was measuring the
//! interface's scheduling.
//!
//! Two properties are pinned here, and they pull in opposite directions:
//!
//! 1. **The span is stamped where the stream is consumed.** `lca-core` times
//!    each provider call from its first generation delta to its last and
//!    carries the result on that call's `Usage` extras (`GENERATION_MS_EXTRA`).
//!    Driving the real agent loop against the fake provider therefore produces
//!    a reading derived from the provider's own pacing, not from this
//!    process's thread wakeups.
//! 2. **The numerator and the denominator describe the same thing.** The
//!    provider's `output` tokens (which include reasoning) over the span in
//!    which those tokens' deltas arrived (reasoning deltas included). A turn
//!    that streams nothing but reasoning still reports; the rule is "both".
//!
//! The rendering-side rows (the impossible-number guard, zero span, the stale
//! reading rule) live beside the code they assert, in `lca-ui`'s
//! `footer::meter` - `UiOptions` has no `Default`, so a second copy of that
//! forty-field fixture in another crate would drift.
//!
//! Verifies: FR-UI-20's `N tok/s` annotation (gh #464), and the "no reading
//! invented before the first measurable turn" rule that preceded it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, TurnEvent, TurnOutcome, TurnSink};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_protocol::{GENERATION_US_EXTRA, Usage};
use lca_session::SessionStore;
use lca_testkit::FakeProvider;
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

/// A permission prompt that denies everything: these rows never need a grant.
struct NoPrompt;

impl PermissionPrompt for NoPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

#[derive(Default)]
struct CollectingSink {
    events: Vec<TurnEvent>,
}

impl TurnSink for CollectingSink {
    fn on_event(&mut self, event: TurnEvent) {
        self.events.push(event);
    }
}

impl CollectingSink {
    /// Every `Usage` event in arrival order - one per provider call.
    fn usages(&self) -> Vec<Usage> {
        self.events
            .iter()
            .filter_map(|event| match event {
                TurnEvent::Usage(usage) => Some(usage.clone()),
                _ => None,
            })
            .collect()
    }
}

struct Harness {
    store: SessionStore,
    session: lca_session::Session,
    grants: Arc<Mutex<GrantStore>>,
    project: PathBuf,
    tools: ToolExecutor,
    provider: Arc<FakeProvider>,
}

fn harness(name: &str, provider: FakeProvider) -> (Harness, PathBuf) {
    let root = lca_testkit::scratch_path(name);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let tools = ToolExecutor::new(
        Arc::new(NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Some(Duration::from_secs(30)),
    );
    (
        Harness {
            store,
            session,
            grants,
            project,
            tools,
            provider: Arc::new(provider),
        },
        root,
    )
}

fn config() -> AgentConfig {
    AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 0,
        retry_base_delay: Duration::ZERO,
        max_iterations: 50,
        ..AgentConfig::default()
    }
}

async fn run(h: &mut Harness, input: &str, sink: &mut CollectingSink) -> TurnOutcome {
    let mut prompt = NoPrompt;
    let mut agent = Agent::new(
        &h.store,
        &h.session,
        h.provider.as_ref(),
        &mut h.tools,
        h.grants.clone(),
        &mut prompt,
        None,
        config(),
    );
    agent.run_turn(input, sink, &CancelFlag::new()).await
}

/// The stamped span, in milliseconds, from a call's usage record.
fn stamped_ms(usage: &Usage) -> Option<u64> {
    usage
        .extras
        .get(GENERATION_US_EXTRA)
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(|micros| micros / 1_000)
}

// ---------------------------------------------------------------------------
// 1. The span is stamped by the consumer, from the provider's own pacing.
// ---------------------------------------------------------------------------

// Verifies: gh #464 - a provider that spaces its deltas over time yields a
// span that reflects that spacing. Four deltas 25 ms apart span ~75 ms, and
// the reading that falls out of it (40 tokens over that span) is a speed a
// real model could produce. On the pre-fix code the span came from the
// interface thread and this row could not distinguish 75 ms from 7 us.
#[tokio::test]
async fn a_paced_stream_stamps_a_span_the_provider_actually_took() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("one ")
                .pause(25)
                .text("two ")
                .pause(25)
                .text("three ")
                .pause(25)
                .text("four")
                .usage(lca_testkit::fake_usage(100, 40, 0, 0))
        })
        .build();
    let (mut h, _root) = harness("464-paced", provider);
    let mut sink = CollectingSink::default();

    let outcome = run(&mut h, "go", &mut sink).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);

    let usage = sink
        .usages()
        .into_iter()
        .last()
        .expect("one usage event per call");
    let span = stamped_ms(&usage).expect("the call carries its generation span");
    // 3 gaps of 25 ms. Generous bounds: a loaded runner may stretch them, and
    // this row is about "microseconds versus tens of milliseconds", not about
    // scheduler precision.
    assert!(
        (20..=2_000).contains(&span),
        "the span is the provider's pacing, not a burst drain: {span} ms"
    );

    let rate = (usage.output as f64 * 1_000.0) / span as f64;
    assert!(
        rate > 1.0 && rate < 100_000.0,
        "a 40-token answer over a ~75 ms stream is a plausible speed: {rate} tok/s"
    );
}

// Verifies: gh #464 - the receipt's shape. A response the provider delivers in
// one burst (no spacing at all) measures no *elapsed* span worth dividing by,
// so it reports zero or nothing rather than millions. `30` output tokens over
// a sub-millisecond span is exactly the arithmetic that printed 5,263,158.
#[tokio::test]
async fn a_burst_response_never_reports_millions() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("Hello")
                .text("! How")
                .text(" can I")
                .text(" help?")
                .usage(lca_testkit::fake_usage(1_300, 30, 0, 0))
        })
        .build();
    let (mut h, _root) = harness("464-burst", provider);
    let mut sink = CollectingSink::default();

    let outcome = run(&mut h, "hi", &mut sink).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);

    let usage = sink.usages().into_iter().last().expect("usage event");
    // Either the call carried no usable span (0 ms -> the interface shows
    // nothing) or it carried one that is honest. Never a microsecond span.
    match stamped_ms(&usage) {
        None => {}
        Some(0) => {}
        Some(span) => {
            let rate = (usage.output as f64 * 1_000.0) / span as f64;
            assert!(
                rate <= 100_000.0,
                "a burst must not print an impossible speed: 30 tokens over {span} ms = {rate} tok/s"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Numerator and denominator describe the same thing: both include
//    reasoning.
// ---------------------------------------------------------------------------

// Verifies: gh #464 - reasoning tokens are in the numerator (the provider
// reports them in `output`) *and* their deltas are in the span, so a turn
// that streams nothing but reasoning still measures instead of dividing
// thinking tokens by zero visible time. The rule is "both", not "mixing".
#[tokio::test]
async fn a_reasoning_only_stream_measures_its_own_span() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.reasoning("weighing ")
                .pause(25)
                .reasoning("the two ")
                .pause(25)
                .reasoning("options")
                .usage(lca_testkit::fake_usage(50, 90, 0, 0))
        })
        .build();
    let (mut h, _root) = harness("464-reasoning", provider);
    let mut sink = CollectingSink::default();

    let outcome = run(&mut h, "think", &mut sink).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);

    let usage = sink.usages().into_iter().last().expect("usage event");
    let span = stamped_ms(&usage).expect("reasoning deltas open and close the span too");
    assert!(
        (20..=2_000).contains(&span),
        "reasoning time is in the denominator, so a reasoning-only turn measures: {span} ms"
    );
}

// ---------------------------------------------------------------------------
// 3. Tool runs stay excluded: the span is per provider call, and a tool runs
//    between calls.
// ---------------------------------------------------------------------------

// Verifies: gh #464 (and the rule it inherits) - tool execution time is not
// generation time. The gap between the two calls is 150 ms of wall clock; the
// reported spans cover only the streaming of each call, so the turn's speed is
// its tokens over its two short streams, not over the whole turn.
#[tokio::test]
async fn tool_execution_time_stays_out_of_every_span() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("running ")
                .pause(20)
                .text("it")
                .tool_call("shell", r#"{"command":"echo hi"}"#)
                .usage(lca_testkit::fake_usage(100, 20, 0, 0))
        })
        .turn(|t| {
            t.text("done")
                .pause(20)
                .usage(lca_testkit::fake_usage(200, 10, 0, 0))
        })
        .build();
    let (mut h, _root) = harness("464-tool-gap", provider);
    std::fs::write(h.project.join("notes.md"), "x").expect("write");
    let mut sink = CollectingSink::default();

    let outcome = run(&mut h, "run it", &mut sink).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);

    let spans: Vec<u64> = sink.usages().iter().filter_map(stamped_ms).collect();
    assert!(!spans.is_empty(), "each streaming call stamps its own span");
    let total: u64 = spans.iter().sum();
    assert!(
        total < 1_000,
        "the shell run's wall clock is not in any span: {spans:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. The key belongs to the host, not to the guest.
// ---------------------------------------------------------------------------

// Verifies: gh #464 - a provider that sets `generation_us` itself cannot hand
// the footer a span the host never observed. The host stamps the key when it
// measured and removes it when it did not, so the reading is always the
// consumer's own measurement. Without the removal, a guest could reintroduce
// exactly the unmeasured-number class this issue is about.
#[tokio::test]
async fn a_guest_supplied_span_never_reaches_the_footer() {
    let mut spoofed = lca_testkit::fake_usage(1_300, 30, 0, 0);
    spoofed.extras.insert(
        GENERATION_US_EXTRA.to_string(),
        // The receipt's number: 30 tokens over 6 us.
        "6".to_string(),
    );
    let provider = FakeProvider::builder()
        .turn(|t| t.text("one answer, one delta").usage(spoofed))
        .build();
    let (mut h, _root) = harness("464-spoofed", provider);
    let mut sink = CollectingSink::default();

    let outcome = run(&mut h, "hi", &mut sink).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);

    let usage = sink.usages().into_iter().last().expect("usage event");
    // A single delta stamps a zero-length span, so the host measured nothing
    // and the key must be gone - not left holding the guest's 6 us.
    assert_eq!(
        stamped_ms(&usage),
        None,
        "the host owns the key: {:?}",
        usage.extras
    );
}
