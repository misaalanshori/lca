//! GitHub #345: `workspace_scoped` read the git subcommand from
//! `toks[i + 1]`, so a global flag before it (`git --no-pager push`,
//! `git -C . push`) shifted the subcommand past the egress and config
//! checks. A trusted folder then auto-approved network egress with no
//! prompt (a hostile repository's exfiltration path, threat model's
//! untrusted-project story).
//!
//! Verifies: end to end through a whole turn - a trusted project must
//! PROMPT, not auto-approve, `git --no-pager push`, and the tool result
//! reports the denial with a `permission` record written.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, TurnEvent, TurnSink};
use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};
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

/// Records every prompt and denies it: a prompt appearing at all is the
/// proof the analyzer refused to auto-approve.
struct RecordingPrompt {
    actions: Vec<Action>,
}

impl PermissionPrompt for RecordingPrompt {
    fn ask(&mut self, action: &Action) -> Decision {
        self.actions.push(action.clone());
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

fn fixture(command: &str) -> Fixture {
    let root = lca_testkit::scratch_path("regression-345-git-flags");
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    // TRUSTED folder: the whole defect lives in this fast path.
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
            t.tool_call("shell", &format!(r#"{{"command":"{command}"}}"#))
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

async fn run_turn(fixture: &mut Fixture, input: &str) -> (Sink, RecordingPrompt) {
    let mut sink = Sink::default();
    let mut prompt = RecordingPrompt { actions: vec![] };
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
            TurnEvent::ToolFinished(result) => Some(result.status),
            _ => None,
        })
        .collect()
}

// Verifies: gh #345 - `git --no-pager push` in a trusted project asks
// the user; it must never auto-approve egress.
#[tokio::test]
async fn a_trusted_folder_prompts_for_git_push_behind_a_global_flag() {
    let mut f = fixture("git --no-pager push");
    let (sink, prompt) = run_turn(&mut f, "run git --no-pager push").await;
    assert_eq!(
        prompt.actions.len(),
        1,
        "the shell action must reach the prompt, not the trust fast path"
    );
    assert!(
        matches!(&prompt.actions[0], Action::Shell { command, .. }
            if command.contains("--no-pager push")),
        "{:?}",
        prompt.actions[0]
    );
    assert_eq!(tool_statuses(&sink), vec![ToolResultStatus::Denied]);
    let records = permission_records(&f);
    assert_eq!(records.len(), 1, "the denial is recorded");
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

// Verifies: gh #345 - `git -C . push` (a flag with an argument) behaves
// the same; the subcommand sits past both the flag and its value.
#[tokio::test]
async fn a_trusted_folder_prompts_for_git_push_after_dash_c() {
    let mut f = fixture("git -C . push origin main");
    let (sink, prompt) = run_turn(&mut f, "run git -C . push").await;
    assert_eq!(prompt.actions.len(), 1, "must prompt");
    assert_eq!(tool_statuses(&sink), vec![ToolResultStatus::Denied]);
}

// Verifies: gh #345 - a benign, classifiable git command keeps the trust
// fast path (no prompt), so the fix does not over-review ordinary work.
#[tokio::test]
async fn a_trusted_folder_still_auto_approves_a_benign_git_read() {
    let mut f = fixture("git --no-pager log --oneline -1");
    // A real repository so the command also succeeds, not just stays silent.
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&f.project)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git")
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "user.email=t@e",
        "-c",
        "user.name=t",
        "commit",
        "-q",
        "-m",
        "init",
        "--allow-empty",
    ]);
    let (sink, prompt) = run_turn(&mut f, "run git log").await;
    assert_eq!(prompt.actions.len(), 0, "benign git stays silent");
    assert_eq!(
        tool_statuses(&sink),
        vec![ToolResultStatus::Ok],
        "the command runs"
    );
}
