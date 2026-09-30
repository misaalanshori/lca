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
        Duration::from_secs(30),
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
