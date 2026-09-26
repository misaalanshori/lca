//! Released defect (0.1.x): a plain conversation recorded and displayed a
//! `cache-boundary-narrowed` extension event on every turn after the first.
//! The boundary correctly ends at the previous request — the provider cached
//! only that — but messages appended since then were treated as a divergence
//! (`previous.get(index).unwrap_or(true)`). FR-CACHE-6's event is for content
//! that was sent and then changed, not for normal growth.
//!
//! This guards the user-visible half: a multi-turn conversation with no
//! transform and no compaction leaves no `cache-boundary-narrowed` record.
//! The rewritten-content case is guarded in
//! `crates/lca-core/tests/loop.rs::stable_region_divergence_narrows_the_boundary_once_and_settles`.
//!
//! Verifies: FR-CACHE-6.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, TurnEvent, TurnSink};
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

/// Headless-equivalent: every ask is denied (nothing here needs one).
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
    let dir = std::env::temp_dir().join(format!(
        "lca-regression-plain-boundary-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

#[tokio::test]
async fn a_plain_conversation_records_no_boundary_divergence() {
    let root = scratch("plain");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let mut tools = ToolExecutor::new(
        Arc::new(NativeOps),
        project.clone(),
        project.clone(),
        65536,
        Duration::from_secs(30),
    );
    let provider = FakeProvider::builder()
        .turn(|t| t.text("one").usage(fake_usage(100, 10, 0, 0)))
        .turn(|t| t.text("two").usage(fake_usage(120, 10, 100, 0)))
        .turn(|t| t.text("three").usage(fake_usage(140, 10, 120, 0)))
        .build();
    let config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 3,
        retry_base_delay: Duration::ZERO,
        max_iterations: 50,
        model_context_window: 100_000,
        compaction_threshold: 0.99,
        ..AgentConfig::default()
    };
    let mut prompt = Prompt;
    for input in ["first", "second", "third"] {
        let mut sink = Sink::default();
        let mut agent = Agent::new(
            &store,
            &session,
            &provider,
            &mut tools,
            grants.clone(),
            &mut prompt,
            None,
            config.clone(),
        );
        let outcome = agent.run_turn(input, &mut sink, &CancelFlag::new()).await;
        assert_eq!(outcome.status, lca_core::TurnStatus::Ok, "{input}");
    }

    let events = store
        .read_with(&session, ViewMode::Display)
        .expect("records")
        .records
        .iter()
        .filter(|record| {
            matches!(record, Record::ExtensionEvent { event, .. } if event == "cache-boundary-narrowed")
        })
        .count();
    assert_eq!(
        events, 0,
        "a plain conversation must not record a boundary divergence"
    );
    // The provider saw three requests, and the boundary grew with each.
    assert_eq!(provider.call_count(), 3);
    let last = provider.last_request().expect("a request");
    assert!(last.stable_prefix > 0 && last.stable_prefix < last.messages.len());
}
