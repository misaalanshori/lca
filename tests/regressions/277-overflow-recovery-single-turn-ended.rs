//! Regression rows for gh #277: `TurnEnded` fires once, when the turn
//! actually ends - including across context-overflow recovery.
//!
//! The defect: the overflow-recovery branch emitted `TurnEvent::Error` *and*
//! `TurnEvent::TurnEnded` and only then tried `compact_before_turn`. Two
//! consequences:
//!
//! - When recovery succeeded, the turn `continue`d and could stream more
//!   events *after* `TurnEnded` - a consumer that treats `TurnEnded` as "this
//!   turn is over" saw a second half of a finished turn.
//! - When recovery was refused (compaction disabled, or too little to
//!   summarize), execution fell through and emitted `Error` + `TurnEnded` a
//!   **second** time.
//!
//! The recovery's own signal is `CompactionStarted`/`CompactionEnded { reason:
//! "overflow" }`, which `compact_before_turn` already emits.
//!
//! Verifies: gh #277 - exactly one `TurnEnded` per turn, and no turn events
//! after it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, ExtensionRegistry, TurnEvent, TurnSink, TurnStatus};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_protocol::Record;
use lca_session::{SessionStore, ViewMode};
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

#[derive(Default)]
struct Sink(Vec<TurnEvent>);

impl TurnSink for Sink {
    fn on_event(&mut self, event: TurnEvent) {
        self.0.push(event);
    }
}

struct Prompt;

impl PermissionPrompt for Prompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(&format!("regression-277-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

use crate::doubles_275::Double;

struct Harness {
    store: SessionStore,
    session: lca_session::Session,
    provider: FakeProvider,
    tools: ToolExecutor,
    grants: Arc<Mutex<GrantStore>>,
    config: AgentConfig,
}

/// `overflow_messages` is how many leading turns answer with a
/// context-length error before the provider cooperates.
async fn harness(
    name: &str,
    registry: ExtensionRegistry,
    warm_turns: usize,
    overflow_turns: usize,
) -> Harness {
    let root = scratch(name);
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
    // Warm turns succeed so the log has a range worth compacting; then the
    // overflow turns; then a cooperating turn for the retry.
    let mut builder = FakeProvider::builder();
    for _ in 0..warm_turns {
        builder = builder.turn(|t| t.text("ok").usage(fake_usage(100, 10, 0, 0)));
    }
    for _ in 0..overflow_turns {
        builder = builder.turn(|t| {
            t.error(
                "provider returned HTTP 400: this model's maximum context length is 8192 tokens",
                false,
            )
            .usage(fake_usage(9000, 10, 0, 0))
        });
    }
    builder = builder
        .turn(|t| t.text("recovered").usage(fake_usage(100, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(100, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(100, 10, 0, 0)));
    let provider = builder.build();
    let config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        // The overflow branch owns recovery, so the retry layer must not
        // swallow the error first (same shape as `compaction_phases.rs`).
        retry_limit: 0,
        retry_base_delay: Duration::ZERO,
        max_iterations: 50,
        model_context_window: 10_000,
        // A threshold that never fires on its own: only the overflow path
        // may compact here.
        compaction_threshold: 0.99,
        compaction_reserve_tokens: 0,
        compaction_keep_recent_tokens: 0,
        extensions: Arc::new(registry),
        ..AgentConfig::default()
    };
    Harness {
        store,
        session,
        provider,
        tools,
        grants,
        config,
    }
}

async fn run(h: &mut Harness, input: &str) -> (Sink, lca_core::TurnOutcome) {
    let mut sink = Sink::default();
    let mut prompt = Prompt;
    let mut agent = Agent::new(
        &h.store,
        &h.session,
        &h.provider,
        &mut h.tools,
        h.grants.clone(),
        &mut prompt,
        None,
        h.config.clone(),
    );
    let outcome = agent.run_turn(input, &mut sink, &CancelFlag::new()).await;
    (sink, outcome)
}

fn count_turn_ended(sink: &Sink) -> usize {
    sink.0
        .iter()
        .filter(|event| matches!(event, TurnEvent::TurnEnded { .. }))
        .count()
}

fn count_errors(sink: &Sink) -> usize {
    sink.0
        .iter()
        .filter(|event| matches!(event, TurnEvent::Error { .. }))
        .count()
}

/// The index of the last `TurnEnded`, so a test can prove nothing came after.
fn last_turn_ended_index(sink: &Sink) -> Option<usize> {
    sink.0
        .iter()
        .rposition(|event| matches!(event, TurnEvent::TurnEnded { .. }))
}

/// Overflow, successful recovery, then a successful retry: exactly one
/// `TurnEnded`, and it is the last event.
#[test]
fn a_recovered_overflow_emits_turn_ended_once_at_the_end() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut registry = ExtensionRegistry::new();
        registry.register(Arc::new(Double::summarizer()));
        let mut h = harness("recovered", registry, 2, 1).await;
        // Two warm turns give the compaction a range to work on.
        let (_s, o) = run(&mut h, "first").await;
        assert_eq!(o.status, TurnStatus::Ok, "{:?}", o.error);
        let (_s, o) = run(&mut h, "second").await;
        assert_eq!(o.status, TurnStatus::Ok, "{:?}", o.error);

        let (sink, outcome) = run(&mut h, "third").await;
        assert_eq!(
            outcome.status,
            TurnStatus::Ok,
            "recovery succeeded: {:?}",
            outcome.error
        );
        assert_eq!(
            count_turn_ended(&sink),
            1,
            "one TurnEnded for a recovered turn, got {:?}",
            sink.0
        );
        assert_eq!(
            last_turn_ended_index(&sink),
            Some(sink.0.len() - 1),
            "nothing is emitted after TurnEnded: {:?}",
            sink.0
        );
        assert_eq!(
            count_errors(&sink),
            0,
            "a recovered turn surfaces no error: {:?}",
            sink.0
        );
        // The recovery is marked by the compaction events, which is how a
        // consumer knows a recoverable failure happened at all.
        let started = sink
            .0
            .iter()
            .filter(|event| matches!(event, TurnEvent::CompactionStarted { .. }))
            .count();
        assert_eq!(started, 1, "the overflow recovery is visible: {:?}", sink.0);
        // And the log carries the compaction record.
        let compactions = h
            .store
            .read_with(&h.session, ViewMode::Display)
            .expect("records")
            .records
            .iter()
            .filter(|record| matches!(record, Record::Compaction { .. }))
            .count();
        assert_eq!(compactions, 1, "the compaction landed in the log");
    });
}

/// Overflow with compaction *disabled*: recovery is impossible, so the turn
/// ends with an error - reported once, with one `TurnEnded`.
#[test]
fn an_unrecoverable_overflow_reports_once() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut h = harness("unrecoverable", ExtensionRegistry::new(), 0, 1).await;
        // No compaction strategy is registered, so recovery cannot run.
        let (sink, outcome) = run(&mut h, "first").await;
        assert_eq!(outcome.status, TurnStatus::Error);
        assert_eq!(
            count_turn_ended(&sink),
            1,
            "exactly one TurnEnded, got {:?}",
            sink.0
        );
        assert_eq!(
            count_errors(&sink),
            1,
            "the failure is reported exactly once: {:?}",
            sink.0
        );
        assert_eq!(
            last_turn_ended_index(&sink),
            Some(sink.0.len() - 1),
            "TurnEnded is last"
        );
    });
}

/// Overflow twice in one turn: the first recovers, the second does not, and
/// the turn still emits one `TurnEnded` and one `Error`.
#[test]
fn a_second_overflow_surfaces_without_a_duplicate_turn_ended() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut registry = ExtensionRegistry::new();
        registry.register(Arc::new(Double::summarizer()));
        // Every turn overflows: the first recovery runs, the retry also
        // overflows, and `overflow_recovered` is already true.
        let mut h = harness("twice", registry, 2, 2).await;
        let (_s, o) = run(&mut h, "first").await;
        assert_eq!(o.status, TurnStatus::Ok, "{:?}", o.error);
        let (_s, o) = run(&mut h, "second").await;
        assert_eq!(o.status, TurnStatus::Ok, "{:?}", o.error);

        let (sink, outcome) = run(&mut h, "third").await;
        assert_eq!(
            outcome.status,
            TurnStatus::Error,
            "a repeated overflow is not recovered again"
        );
        assert_eq!(
            count_turn_ended(&sink),
            1,
            "one TurnEnded even though two overflow attempts happened: {:?}",
            sink.0
        );
        assert_eq!(
            count_errors(&sink),
            1,
            "one Error for the turn's real failure: {:?}",
            sink.0
        );
        assert_eq!(
            last_turn_ended_index(&sink),
            Some(sink.0.len() - 1),
            "TurnEnded is last"
        );
    });
}

/// A plain successful turn emits one `TurnEnded` and no `Error` - the row
/// that would catch a fix that suppressed the normal end.
#[test]
fn a_plain_turn_emits_one_turn_ended() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut h = harness("plain", ExtensionRegistry::new(), 0, 0).await;
        let (sink, outcome) = run(&mut h, "hello").await;
        assert_eq!(outcome.status, TurnStatus::Ok);
        assert_eq!(count_turn_ended(&sink), 1);
        assert_eq!(count_errors(&sink), 0);
        assert_eq!(last_turn_ended_index(&sink), Some(sink.0.len() - 1));
    });
}
