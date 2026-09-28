//! Shared helpers for the agent-loop test files (`loop.rs`,
//! `loop_phases.rs`): the fake-provider harness, the collecting sink, and
//! the scripted permission prompt.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
#![allow(dead_code)] // a shared helper module: each test crate uses a subset.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, TurnEvent, TurnOutcome, TurnSink};
use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_session::SessionStore;
use lca_testkit::FakeProvider;
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

pub fn scratch(name: &str) -> PathBuf {
    lca_testkit::scratch_path(name)
}

#[derive(Default)]
pub struct CollectingSink {
    pub events: Vec<TurnEvent>,
}

impl TurnSink for CollectingSink {
    fn on_event(&mut self, event: TurnEvent) {
        self.events.push(event);
    }
}

impl CollectingSink {
    pub fn texts(&self) -> String {
        self.events
            .iter()
            .filter_map(|e| match e {
                TurnEvent::TextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn count(&self, predicate: impl Fn(&TurnEvent) -> bool) -> usize {
        self.events.iter().filter(|e| predicate(e)).count()
    }
}

pub struct Prompt {
    pub answers: Vec<Decision>,
    pub asked: Vec<String>,
}

impl PermissionPrompt for Prompt {
    fn ask(&mut self, action: &Action) -> Decision {
        self.asked.push(action.display());
        self.answers.pop().unwrap_or(Decision::Denied)
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

pub struct Harness {
    pub root: PathBuf,
    pub store: SessionStore,
    pub session: lca_session::Session,
    pub grants: Arc<Mutex<GrantStore>>,
    pub project: PathBuf,
    pub tools: ToolExecutor,
    pub provider: Arc<FakeProvider>,
    pub config: AgentConfig,
}

pub fn harness(name: &str, provider: FakeProvider, config: AgentConfig) -> Harness {
    let root = scratch(name);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let tools = ToolExecutor::new(
        Arc::new(NativeOps),
        project.clone(),
        project.clone(),
        65536,
        Duration::from_secs(30),
    );
    Harness {
        root,
        store,
        session,
        grants,
        project,
        tools,
        provider: Arc::new(provider),
        config,
    }
}

pub fn default_config() -> AgentConfig {
    AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 3,
        retry_base_delay: Duration::ZERO,
        max_iterations: 50,
        ..AgentConfig::default()
    }
}

pub async fn turn(
    h: &mut Harness,
    input: &str,
    sink: &mut CollectingSink,
    prompt: &mut Prompt,
) -> TurnOutcome {
    let mut agent = Agent::new(
        &h.store,
        &h.session,
        h.provider.as_ref(),
        &mut h.tools,
        h.grants.clone(),
        prompt,
        None,
        h.config.clone(),
    );
    agent.run_turn(input, sink, &CancelFlag::new()).await
}
