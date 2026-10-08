//! R1: the session's thinking level rides the request extras as
//! `reasoning-effort`, which a provider extension honors where meaningful
//! (`docs/adr/0035`'s settings shape; no ABI change).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, StopReason, TurnEvent, TurnSink};
use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_session::SessionStore;
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

struct Sink(Vec<TurnEvent>);

impl TurnSink for Sink {
    fn on_event(&mut self, event: TurnEvent) {
        self.0.push(event);
    }
}

struct Prompt;

impl PermissionPrompt for Prompt {
    fn ask(&mut self, _action: &Action) -> Decision {
        Decision::Once
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        true
    }
}

fn scratch(name: &str) -> PathBuf {
    lca_testkit::scratch_path(name)
}

#[tokio::test]
async fn the_thinking_level_rides_the_request_extras() {
    let root = scratch("reasoning-effort");
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
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(1, 1, 0, 0)))
        .build();
    let config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 1,
        retry_base_delay: Duration::ZERO,
        max_iterations: 10,
        reasoning_effort: Some("high".to_string()),
        ..AgentConfig::default()
    };
    let mut prompt = Prompt;
    let mut agent = Agent::new(
        &store,
        &session,
        &provider,
        &mut tools,
        grants,
        &mut prompt,
        None,
        config,
    );
    let outcome = agent
        .run_turn("hi", &mut Sink(Vec::new()), &CancelFlag::new())
        .await;
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let request = provider.requests().pop().expect("a provider call");
    assert_eq!(
        request.extras.get("reasoning-effort").map(String::as_str),
        Some("high"),
        "the level is in the extras the provider sees"
    );
}

// Verifies: gh #41 (a signature-requiring fake's turn persists the
// signature and the level it ran at on the assistant record, so
// replay resends both).
#[tokio::test]
async fn the_turn_persists_the_thinking_signature_and_level() {
    let root = scratch("reasoning-signature");
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
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.reasoning("because")
                .thinking_signature("sig-bytes")
                .text("ok")
                .usage(fake_usage(1, 1, 0, 0))
        })
        .build();
    let config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 1,
        retry_base_delay: Duration::ZERO,
        max_iterations: 10,
        reasoning_effort: Some("high".to_string()),
        ..AgentConfig::default()
    };
    let mut prompt = Prompt;
    let mut agent = Agent::new(
        &store,
        &session,
        &provider,
        &mut tools,
        grants,
        &mut prompt,
        None,
        config,
    );
    let outcome = agent
        .run_turn("hi", &mut Sink(Vec::new()), &CancelFlag::new())
        .await;
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let records = store.read(&session).expect("records").records;
    let assistant = records
        .iter()
        .find_map(|record| match record {
            lca_protocol::Record::Assistant {
                reasoning_signature,
                provider_thinking_level,
                ..
            } => Some((reasoning_signature.clone(), provider_thinking_level.clone())),
            _ => None,
        })
        .expect("an assistant record");
    assert_eq!(assistant.0.as_deref(), Some("sig-bytes"));
    assert_eq!(assistant.1.as_deref(), Some("high"));
}

// Verifies: gh #41 (the turn's resolved budget rides the request
// extras beside the level, for budget-taking vendors).
#[tokio::test]
async fn the_thinking_budget_rides_the_request_extras() {
    let root = scratch("reasoning-budget");
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
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(1, 1, 0, 0)))
        .build();
    let config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 1,
        retry_base_delay: Duration::ZERO,
        max_iterations: 10,
        reasoning_effort: Some("high".to_string()),
        thinking_budget: Some(16384),
        ..AgentConfig::default()
    };
    let mut prompt = Prompt;
    let mut agent = Agent::new(
        &store,
        &session,
        &provider,
        &mut tools,
        grants,
        &mut prompt,
        None,
        config,
    );
    let outcome = agent
        .run_turn("hi", &mut Sink(Vec::new()), &CancelFlag::new())
        .await;
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let request = provider.requests().pop().expect("a provider call");
    assert_eq!(
        request
            .extras
            .get("thinking-budget-tokens")
            .map(String::as_str),
        Some("16384"),
        "the budget rides beside the level"
    );
}
