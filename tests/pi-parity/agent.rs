//! Agent-loop parity: retry-then-recover, fatal errors that leave the
//! session open, and per-turn usage with cache fields. Driven by the
//! scripted fake provider, no network, no cost.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, TurnEvent, TurnOutcome, TurnStatus};
use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_session::{SessionStore, ViewMode};
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

#[derive(Default)]
struct CollectingSink {
    events: Vec<TurnEvent>,
}

impl lca_core::TurnSink for CollectingSink {
    fn on_event(&mut self, event: TurnEvent) {
        self.events.push(event);
    }
}

struct Prompt;

impl PermissionPrompt for Prompt {
    fn ask(&mut self, action: &Action) -> Decision {
        let _ = action.display();
        Decision::Denied
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

struct Harness {
    store: SessionStore,
    session: lca_session::Session,
    grants: Arc<Mutex<GrantStore>>,
    tools: ToolExecutor,
    provider: Arc<FakeProvider>,
    config: AgentConfig,
}

fn harness(name: &str, provider: FakeProvider) -> Harness {
    let root = lca_testkit::scratch_path(&format!("pi-parity-agent-{name}"));
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "parity").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let tools = ToolExecutor::new(
        Arc::new(NativeOps::default()),
        project.clone(),
        project,
        65536,
        Duration::from_secs(30),
    );
    Harness {
        store,
        session,
        grants,
        tools,
        provider: Arc::new(provider),
        config: AgentConfig {
            provider: "fake".to_string(),
            model: "faux-1".to_string(),
            retry_limit: 3,
            retry_base_delay: Duration::ZERO,
            max_iterations: 50,
            ..AgentConfig::default()
        },
    }
}

async fn turn(h: &mut Harness, input: &str, sink: &mut CollectingSink) -> TurnOutcome {
    let mut prompt = Prompt;
    let cancel = CancelFlag::new();
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
    agent.run_turn(input, sink, &cancel).await
}

// Verifies: pi:packages/coding-agent/test/agent-session-retry.test.ts (a
// retryable transport error retries with backoff; recovery completes the
// turn instead of failing it).
#[tokio::test]
async fn pi_parity_retryable_error_retries_then_succeeds() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.error("connection reset", true)
                .usage(fake_usage(1, 0, 0, 0))
        })
        .turn(|t| t.text("recovered").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut h = harness("retry", provider);
    let mut sink = CollectingSink::default();
    let outcome = turn(&mut h, "hi", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Ok);
    assert!(
        sink.events
            .iter()
            .any(|e| matches!(e, TurnEvent::RetryScheduled { attempt: 1, .. })),
        "the retry is announced, not silent"
    );
    assert_eq!(h.provider.call_count(), 2, "one retry, then the answer");
}

// Verifies: pi:packages/coding-agent/test/agent-session-retry.test.ts (an
// error past the retry budget surfaces with the session left open for the
// next turn).
#[tokio::test]
async fn pi_parity_fatal_error_keeps_the_session_open() {
    let provider = FakeProvider::builder()
        .turn(|t| t.error("bad request", false).usage(fake_usage(1, 0, 0, 0)))
        .turn(|t| t.text("next turn").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut h = harness("fatal", provider);
    let mut sink = CollectingSink::default();
    let outcome = turn(&mut h, "hi", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Error);

    let mut sink = CollectingSink::default();
    let again = turn(&mut h, "still here", &mut sink).await;
    assert_eq!(
        again.status,
        TurnStatus::Ok,
        "the session survives the failure"
    );
    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    assert_eq!(
        read.records
            .iter()
            .filter(|r| matches!(r, lca_protocol::Record::User { .. }))
            .count(),
        2,
        "both turns are durable"
    );
}

// Verifies: pi:packages/coding-agent/docs/json.md#agent-and-turn-events
// (a turn's usage report carries cache counts; the record keeps them).
#[tokio::test]
async fn pi_parity_turn_records_usage_with_cache_fields() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("done").usage(lca_protocol::Usage {
                input: 40,
                output: 12,
                cache_read: 1200,
                cache_write: 0,
                ..Default::default()
            })
        })
        .build();
    let mut h = harness("usage", provider);
    let mut sink = CollectingSink::default();
    let outcome = turn(&mut h, "hi", &mut sink).await;
    assert_eq!(outcome.usage.input, 40);
    assert_eq!(outcome.usage.cache_read, 1200);

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    let usage = read
        .records
        .iter()
        .find_map(|r| match r {
            lca_protocol::Record::Assistant {
                usage: Some(usage), ..
            } => Some(usage.clone()),
            _ => None,
        })
        .expect("usage on the assistant record");
    assert_eq!(usage.cache_read, 1200);
    assert_eq!(usage.output, 12);
}
