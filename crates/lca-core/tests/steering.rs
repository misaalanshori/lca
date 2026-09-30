//! Steering: prompts submitted while a turn runs (ADR-0038, FR-CORE-11/12).
//!
//! A steer joins the turn's input at the next model-call boundary; queued
//! messages keep their order; the stable cache prefix never moves under
//! injection; the submit-mode marker travels on the session record and in
//! the message `extras` extensions see.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, StopReason, TurnEvent, TurnSink};
use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_protocol::{QueuedMessage, Record, SteerQueue, SubmitMode, TurnStatus};
use lca_session::{SessionStore, ViewMode};
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

fn scratch(name: &str) -> PathBuf {
    lca_testkit::scratch_path(name)
}

/// A sink that pushes a queued message the moment a tool starts, so the
/// steer lands mid-turn at a deterministic point.
struct SteeringSink {
    events: Vec<TurnEvent>,
    steer: SteerQueue,
    to_push: Vec<(String, SubmitMode)>,
    pushed: usize,
}

impl TurnSink for SteeringSink {
    fn on_event(&mut self, event: TurnEvent) {
        if matches!(event, TurnEvent::ToolStarted(_))
            && let Some((text, mode)) = self.to_push.get(self.pushed).cloned()
        {
            self.steer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(QueuedMessage { text, mode });
            self.pushed += 1;
        }
        self.events.push(event);
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

struct Harness {
    root: PathBuf,
    store: SessionStore,
    session: lca_session::Session,
    grants: Arc<Mutex<GrantStore>>,
    tools: ToolExecutor,
    provider: Arc<FakeProvider>,
    config: AgentConfig,
    steer: SteerQueue,
}

fn harness(name: &str, provider: FakeProvider) -> Harness {
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
        Duration::from_secs(30),
    );
    let steer = lca_protocol::steer_queue();
    let config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 3,
        retry_base_delay: Duration::ZERO,
        max_iterations: 50,
        steer: steer.clone(),
        ..AgentConfig::default()
    };
    Harness {
        root,
        store,
        session,
        grants,
        tools,
        provider: Arc::new(provider),
        config,
        steer,
    }
}

/// Two tool rounds then a final text: three provider calls, so a steer
/// queued after round 1 can be observed crossing into call 2.
fn three_round_provider() -> FakeProvider {
    FakeProvider::builder()
        .turn(|t| {
            t.tool_call("read", "{\"path\":\"a\"}")
                .usage(fake_usage(10, 5, 0, 0))
        })
        .turn(|t| {
            t.tool_call("read", "{\"path\":\"b\"}")
                .usage(fake_usage(10, 5, 0, 0))
        })
        .turn(|t| t.text("done").usage(fake_usage(10, 5, 0, 0)))
        .build()
}

fn messages_text(request: &lca_provider::CompletionRequest) -> String {
    request
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|block| match block {
            lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: FR-CORE-11 - a steer joins the turn's input at the next
// model-call boundary, not the current stream.
#[tokio::test]
async fn a_steer_joins_the_next_call_not_the_current_one() {
    let provider = three_round_provider();
    let mut h = harness("steer-boundary", provider);
    let mut sink = SteeringSink {
        events: Vec::new(),
        steer: h.steer.clone(),
        to_push: vec![("steer-1".to_string(), SubmitMode::Steer)],
        pushed: 0,
    };
    let mut prompt = Prompt;
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
    let outcome = agent.run_turn("start", &mut sink, &CancelFlag::new()).await;
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let requests = h.provider.requests();
    assert_eq!(requests.len(), 3, "three provider calls");
    assert!(
        !messages_text(&requests[0]).contains("steer-1"),
        "the steer is not in the call that was already running"
    );
    assert!(
        messages_text(&requests[1]).contains("steer-1"),
        "the steer joins the next call"
    );
    assert!(
        sink.events.iter().any(|e| matches!(
            e,
            TurnEvent::UserInjected { text, mode } if text == "steer-1" && mode == "steer"
        )),
        "the interface is told the steer crossed the boundary"
    );
}

// Verifies: FR-CORE-11 - queued messages keep their submission order.
#[tokio::test]
async fn queued_messages_keep_their_order() {
    let provider = three_round_provider();
    let mut h = harness("steer-order", provider);
    let mut sink = SteeringSink {
        events: Vec::new(),
        steer: h.steer.clone(),
        to_push: vec![
            ("first".to_string(), SubmitMode::Steer),
            ("second".to_string(), SubmitMode::Steer),
        ],
        pushed: 0,
    };
    let mut prompt = Prompt;
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
    agent.run_turn("start", &mut sink, &CancelFlag::new()).await;

    let text = messages_text(h.provider.requests().last().expect("a final call"));
    let first = text.find("first").expect("first present");
    let second = text.find("second").expect("second present");
    assert!(first < second, "order preserved:\n{text}");
}

// Verifies: FR-CORE-11 / ADR-0017 - steered messages extend the list
// without moving previously sent content (the stable prefix keeps its
// value).
#[tokio::test]
async fn the_stable_prefix_does_not_move_under_injection() {
    let provider = three_round_provider();
    let mut h = harness("steer-prefix", provider);
    let mut sink = SteeringSink {
        events: Vec::new(),
        steer: h.steer.clone(),
        to_push: vec![("steer-1".to_string(), SubmitMode::Steer)],
        pushed: 0,
    };
    let mut prompt = Prompt;
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
    agent.run_turn("start", &mut sink, &CancelFlag::new()).await;

    let requests = h.provider.requests();
    let first = &requests[0];
    let second = &requests[1];
    // Every message the first call sent is still present, in the same
    // place, on the next call: injection appends, it never rewrites.
    for (index, message) in first.messages.iter().enumerate() {
        assert_eq!(
            second.messages.get(index).map(|m| format!("{m:?}")),
            Some(format!("{message:?}")),
            "message {index} did not move"
        );
    }
    // The stable prefix of the second call still covers the first call's
    // prefix: previously cached content stays cacheable.
    assert!(second.stable_prefix >= first.stable_prefix);
}

// Verifies: FR-CORE-11 / ADR-0038 - the submit-mode marker travels on the
// session record and in the message `extras` extensions see.
#[tokio::test]
async fn the_submit_mode_marker_is_recorded_and_visible_to_extensions() {
    let provider = three_round_provider();
    let mut h = harness("steer-marker", provider);
    let mut sink = SteeringSink {
        events: Vec::new(),
        steer: h.steer.clone(),
        to_push: vec![("steer-1".to_string(), SubmitMode::Steer)],
        pushed: 0,
    };
    let mut prompt = Prompt;
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
    agent.run_turn("start", &mut sink, &CancelFlag::new()).await;

    // The record carries the marker.
    let records = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("read")
        .records;
    assert!(
        records.iter().any(|r| matches!(
            r,
            Record::User { content, queue: Some(marker), .. }
                if content == "steer-1" && marker == "steer"
        )),
        "the session log records the queued message and its mode"
    );
    // The transform input's message carries it in `extras`.
    let request = h.provider.requests().last().cloned().expect("a call");
    assert!(
        request.messages.iter().any(|m| {
            m.extras.get("queue").map(String::as_str) == Some("steer")
                && messages_text_of(m).contains("steer-1")
        }),
        "the marker is in the message extras an extension receives"
    );
}

fn messages_text_of(message: &lca_protocol::ChatMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: FR-CORE-12 - an aborted turn leaves its queue to the
// interface, which restores it to the editor (the UI test in
// `crates/lca-ui/src/chat.rs` covers the restore itself).
#[tokio::test]
async fn a_cancelled_turn_reports_cancellation() {
    let provider = FakeProvider::builder()
        .turn(|t| t.pause(10_000).text("never").usage(fake_usage(1, 1, 0, 0)))
        .build();
    let mut h = harness("steer-cancel", provider);
    let mut sink = SteeringSink {
        events: Vec::new(),
        steer: h.steer.clone(),
        to_push: Vec::new(),
        pushed: 0,
    };
    let cancel = CancelFlag::new();
    let cancel_clone = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        cancel_clone.cancel();
    });
    let mut prompt = Prompt;
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
    let outcome = agent.run_turn("start", &mut sink, &cancel).await;
    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    assert_eq!(outcome.status, TurnStatus::Ok);
    let _ = h.root;
}
