//! GitHub #259: allow/deny rules never applied to in-workspace reads
//! because `turn/execute.rs` only called `authorize()` when
//! `required_permission` returned `Some` — and ordinary in-workspace
//! reads returned `None`. A stored `deny` rule watched a read run
//! `ok` with no record (FR-PERM-21: a deny rule SHALL refuse).
//!
//! Direction (brief): ordinary in-workspace reads route through
//! `authorize()` for rule matching with silence preserved — deny
//! refuses + records, allow allows + records, neither stays silent
//! (no modal, no record). Sensitive paths keep #215 behavior.
//!
//! Verifies: end to end through whole turns — deny refuses, allow
//! allows + records, no-rule stays silent, sensitive still prompts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, TurnEvent, TurnSink};
use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, RuleDecision, RuleScope,
};
use lca_protocol::{PermissionDecision, Record, ToolResultStatus};
use lca_session::SessionStore;
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

#[derive(Default)]
struct Sink(Vec<TurnEvent>);

impl TurnSink for Sink {
    fn on_event(&mut self, event: TurnEvent) {
        self.0.push(event);
    }
}

/// Nothing here should ever reach a human: any prompt is a failure.
struct SilentPrompt {
    asked: usize,
}

impl PermissionPrompt for SilentPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        self.asked += 1;
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

struct Fixture {
    store: SessionStore,
    session: lca_session::Session,
    grants: Arc<Mutex<GrantStore>>,
    project: PathBuf,
    tools: ToolExecutor,
    provider: FakeProvider,
    config: AgentConfig,
}

fn fixture(name: &str, read_path: &str) -> Fixture {
    let root = lca_testkit::scratch_path(&format!("regression-259-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    std::fs::write(project.join("muffin-recipe.txt"), "flour sugar").expect("write");
    std::fs::write(project.join(".env"), "SECRET=hunter2").expect("write");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    grants.lock().expect("lock").trust_for_session(&project);
    let tools = ToolExecutor::new(
        Arc::new(NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Some(Duration::from_secs(30)),
    );
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("read", &format!(r#"{{"path":"{read_path}"}}"#))
                .usage(fake_usage(10, 5, 0, 10))
        })
        .turn(|t| t.text("done").usage(fake_usage(10, 5, 0, 10)))
        .build();
    let config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 3,
        retry_base_delay: Duration::ZERO,
        max_iterations: 50,
        ..AgentConfig::default()
    };
    Fixture {
        store,
        session,
        grants,
        project,
        tools,
        provider,
        config,
    }
}

async fn run_turn(fixture: &mut Fixture, input: &str) -> (Sink, SilentPrompt) {
    let mut sink = Sink::default();
    let mut prompt = SilentPrompt { asked: 0 };
    let mut agent = Agent::new(
        &fixture.store,
        &fixture.session,
        &fixture.provider,
        &mut fixture.tools,
        fixture.grants.clone(),
        &mut prompt,
        None,
        fixture.config.clone(),
    );
    let outcome = agent.run_turn(input, &mut sink, &CancelFlag::new()).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok, "{outcome:?}");
    (sink, prompt)
}

fn permission_records(fixture: &Fixture) -> Vec<Record> {
    fixture
        .store
        .read(&fixture.session)
        .expect("read log")
        .records
        .into_iter()
        .filter(|record| matches!(record, Record::Permission { .. }))
        .collect()
}

fn tool_statuses(sink: &Sink) -> Vec<ToolResultStatus> {
    sink.0
        .iter()
        .filter_map(|event| match event {
            TurnEvent::ToolFinished(result) => Some(result.status.clone()),
            _ => None,
        })
        .collect()
}

fn rule(fixture: &Fixture, decision: RuleDecision, pattern: &str) {
    fixture
        .grants
        .lock()
        .expect("lock")
        .add_rule(&fixture.project, RuleScope::Project, decision, pattern)
        .expect("rule");
}

// Verifies: gh #259 - a deny rule refuses an in-workspace read (the
// live proof: a stored deny watched the read run `ok`).
#[tokio::test]
async fn deny_rule_refuses_an_in_workspace_read() {
    let mut f = fixture("deny", "muffin-recipe.txt");
    rule(&f, RuleDecision::Deny, "*muffin-recipe.txt");
    let (sink, prompt) = run_turn(&mut f, "read the recipe").await;
    assert_eq!(prompt.asked, 0, "a deny rule never prompts");
    assert_eq!(tool_statuses(&sink), vec![ToolResultStatus::Denied]);
    let records = permission_records(&f);
    assert_eq!(records.len(), 1, "the refusal is recorded");
    assert!(
        matches!(
            records[0],
            Record::Permission {
                decision: PermissionDecision::Denied,
                ..
            }
        ),
        "{:?}",
        records[0]
    );
}

// Verifies: gh #259 - an allow rule allows and records (silently: the
// rule is the decision, no modal).
#[tokio::test]
async fn allow_rule_allows_and_records() {
    let mut f = fixture("allow", "muffin-recipe.txt");
    rule(&f, RuleDecision::Allow, "*muffin-recipe.txt");
    let (sink, prompt) = run_turn(&mut f, "read the recipe").await;
    assert_eq!(prompt.asked, 0, "an allow rule never prompts");
    assert_eq!(tool_statuses(&sink), vec![ToolResultStatus::Ok]);
    let records = permission_records(&f);
    assert_eq!(records.len(), 1, "the rule allowance is recorded");
    assert!(
        matches!(
            records[0],
            Record::Permission {
                decision: PermissionDecision::Once,
                ..
            }
        ),
        "{:?}",
        records[0]
    );
}

// Verifies: gh #259 - with no rule matching, an in-workspace read is
// exactly as silent as today (no modal, no record).
#[tokio::test]
async fn no_rule_read_stays_silent() {
    let mut f = fixture("silent", "muffin-recipe.txt");
    let (sink, prompt) = run_turn(&mut f, "read the recipe").await;
    assert_eq!(prompt.asked, 0, "no modal");
    assert_eq!(tool_statuses(&sink), vec![ToolResultStatus::Ok]);
    assert!(
        permission_records(&f).is_empty(),
        "no record without a decision"
    );
}

// Verifies: gh #259 keeps #215 - a sensitive read still prompts even
// with no rule matching.
#[tokio::test]
async fn sensitive_still_prompts() {
    let mut f = fixture("sensitive", ".env");
    let (sink, prompt) = run_turn(&mut f, "read the env file").await;
    assert_eq!(prompt.asked, 1, "sensitive still prompts");
    assert_eq!(tool_statuses(&sink), vec![ToolResultStatus::Denied]);
    assert_eq!(
        permission_records(&f).len(),
        1,
        "the prompt decision is recorded"
    );
}
