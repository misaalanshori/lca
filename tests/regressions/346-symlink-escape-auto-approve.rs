//! GitHub #346: `shell::path_ok` normalized lexically only, so a
//! workspace symlink pointing outside (`ln -s /etc ./etc_link`) made
//! `cat ./etc_link/secret` lexically inside and folder trust
//! auto-approved an out-of-workspace read with no prompt.
//!
//! Verifies: end to end through a whole turn - a trusted project with a
//! symlink to an outside directory must PROMPT for a read through it, the
//! denied read's content never lands in the tool result, and an ordinary
//! in-workspace read still stays silent.

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

/// Builds a trusted project with `etc_link -> <root>/outside`, where
/// `outside/secret` holds a known string the denied read must never leak.
/// The symlink itself is unix-only (Windows needs Developer Mode); the
/// happy-path test below runs everywhere.
fn fixture(command: &str) -> Fixture {
    let root = lca_testkit::scratch_path("regression-346-symlink");
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside).expect("mkdir");
    std::fs::write(outside.join("secret"), "TOP-SECRET-CANARY").expect("write");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, project.join("etc_link")).expect("symlink");
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

fn tool_results(sink: &Sink) -> Vec<&lca_protocol::ToolResult> {
    sink.0
        .iter()
        .filter_map(|event| match event {
            TurnEvent::ToolFinished(result) => Some(result),
            _ => None,
        })
        .collect()
}

// Verifies: gh #346 - a read through a workspace symlink to outside
// asks the user; it must never auto-approve.
#[cfg(unix)]
#[tokio::test]
async fn a_trusted_folder_prompts_for_a_read_through_an_outside_symlink() {
    let mut f = fixture("cat ./etc_link/secret");
    let (sink, prompt) = run_turn(&mut f, "read through the link").await;
    assert_eq!(
        prompt.actions.len(),
        1,
        "the shell action must reach the prompt, not the trust fast path"
    );
    assert!(
        matches!(&prompt.actions[0], Action::Shell { command, .. }
            if command.contains("etc_link")),
        "{:?}",
        prompt.actions[0]
    );
    let results = tool_results(&sink);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, ToolResultStatus::Denied);
    // The denied read's content must never reach the transcript.
    assert!(
        !results[0].content.contains("TOP-SECRET-CANARY"),
        "the outside content leaked into the result: {}",
        results[0].content
    );
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

// Verifies: gh #346 - an ordinary in-workspace read keeps the trust fast
// path (no prompt), so the fix does not over-review normal work.
#[tokio::test]
async fn a_trusted_folder_still_auto_approves_an_in_workspace_read() {
    let mut f = fixture("cat notes.txt");
    std::fs::write(f.project.join("notes.txt"), "hello").expect("write");
    let (sink, prompt) = run_turn(&mut f, "read a normal file").await;
    assert_eq!(prompt.actions.len(), 0, "an in-workspace read stays silent");
    assert_eq!(tool_results(&sink)[0].status, ToolResultStatus::Ok);
}
