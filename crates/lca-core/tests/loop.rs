//! Agent loop tests: the core turn flow against the fake provider, with
//! every FR it verifies named on the test.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lca_core::{Agent, AgentConfig, StopReason, TurnEvent, TurnOutcome, TurnSink};
use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_protocol::{Record, ToolResultStatus};
use lca_session::{SessionStore, ViewMode};
use lca_testkit::{FakeProvider, Provider, fake_usage};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-core-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

#[derive(Default)]
struct CollectingSink {
    events: Vec<TurnEvent>,
}

impl TurnSink for CollectingSink {
    fn on_event(&mut self, event: TurnEvent) {
        self.events.push(event);
    }
}

impl CollectingSink {
    fn texts(&self) -> String {
        self.events
            .iter()
            .filter_map(|e| match e {
                TurnEvent::TextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect()
    }

    fn count(&self, predicate: impl Fn(&TurnEvent) -> bool) -> usize {
        self.events.iter().filter(|e| predicate(e)).count()
    }
}

struct Prompt {
    answers: Vec<Decision>,
    asked: Vec<String>,
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

struct Harness {
    store: SessionStore,
    session: lca_session::Session,
    grants: GrantStore,
    project: PathBuf,
    tools: ToolExecutor,
    provider: Arc<FakeProvider>,
    config: AgentConfig,
}

fn harness(name: &str, provider: FakeProvider, config: AgentConfig) -> Harness {
    let root = scratch(name);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = GrantStore::open(&root.join("grants.json")).expect("grants");
    let tools = ToolExecutor::new(
        Arc::new(NativeOps),
        project.clone(),
        project.clone(),
        65536,
        Duration::from_secs(30),
    );
    Harness {
        store,
        session,
        grants,
        project,
        tools,
        provider: Arc::new(provider),
        config,
    }
}

fn default_config() -> AgentConfig {
    AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 3,
        retry_base_delay: Duration::ZERO,
        max_iterations: 50,
        ..AgentConfig::default()
    }
}

async fn turn(
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
        &mut h.grants,
        prompt,
        None,
        h.config.clone(),
    );
    agent.run_turn(input, sink, &CancelFlag::new()).await
}

// Verifies: FR-CORE-4 (partial content renders as it arrives)
#[tokio::test]
async fn streams_text_deltas_as_they_arrive() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("Hello, ")
                .text("world")
                .usage(fake_usage(10, 5, 0, 10))
        })
        .build();
    let mut h = harness("stream", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        sink.count(|e| matches!(e, TurnEvent::TextDelta(_))),
        2,
        "two deltas, in order"
    );
    assert_eq!(sink.texts(), "Hello, world");
    assert!(
        sink.events.iter().any(|e| matches!(
            e,
            TurnEvent::TurnEnded {
                status: lca_core::TurnStatus::Ok,
                ..
            }
        )),
        "turn end is signalled"
    );
}

// Verifies: FR-CORE-8 (token count and cost, including cache fields, are
// recorded on each turn)
#[tokio::test]
async fn records_usage_with_cache_fields_per_turn() {
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
    let mut h = harness("usage", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.usage.input, 40);
    assert_eq!(outcome.usage.cache_read, 1200);

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    let assistant = read
        .records
        .iter()
        .find_map(|r| match r {
            Record::Assistant { usage, content, .. } => Some((usage.clone(), content.clone())),
            _ => None,
        })
        .expect("assistant record");
    let usage = assistant.0.expect("usage on the record");
    assert_eq!(usage.cache_read, 1200);
    assert_eq!(usage.output, 12);
    let text = assistant
        .1
        .iter()
        .find_map(|b| match b {
            lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .expect("text block");
    assert_eq!(text, "done");
}

// The full tool loop: model calls a tool, the result goes back, the model
// finishes (docs/flows.md, "a turn with a tool call").
#[tokio::test]
async fn runs_tool_calls_sequentially_until_the_model_stops() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("read", r#"{"path":"notes.md"}"#)
                .usage(fake_usage(100, 20, 0, 100))
        })
        .turn(|t| {
            t.text("The file contains three notes.")
                .usage(fake_usage(200, 30, 100, 100))
        })
        .build();
    let mut h = harness("tool-loop", provider, default_config());
    std::fs::write(h.project.join("notes.md"), "three notes").expect("write");
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "what is in notes.md?", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert!(
        sink.events.iter().any(|e| matches!(e, TurnEvent::ToolFinished(r) if r.status == ToolResultStatus::Ok && r.content.contains("three notes"))),
        "tool ran and its result surfaced"
    );
    assert!(sink.texts().contains("three notes"));

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    let kinds: Vec<&str> = read.records.iter().map(|r| r.type_tag()).collect();
    assert_eq!(
        kinds,
        vec![
            "session-start",
            "user",
            "assistant",
            "tool-call",
            "tool-result",
            "assistant"
        ],
        "the log tells the whole turn"
    );

    // The tool result reaches the next provider call, keyed by call id.
    let request = h.provider.last_request().expect("request recorded");
    assert!(
        request
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("call-0")),
        "tool results round-trip into the next request by call id"
    );
}

// Verifies: FR-CONC-2 (tool calls within a turn run sequentially, in the
// order the model emitted them)
#[tokio::test]
async fn executes_tool_calls_in_order() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("write", r#"{"path":"a.txt","content":"A"}"#)
                .tool_call("write", r#"{"path":"b.txt","content":"B"}"#)
                .usage(fake_usage(50, 40, 0, 50))
        })
        .turn(|t| t.text("both written").usage(fake_usage(90, 10, 50, 0)))
        .build();
    let mut h = harness("order", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "write both", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let started: Vec<String> = sink
        .events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::ToolStarted(call) => Some(call.name.clone() + ":" + &call.arguments),
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), 2);
    assert!(
        started[0].contains("a.txt"),
        "first call first: {started:?}"
    );
    assert!(started[1].contains("b.txt"));
}

// Verifies: FR-CORE-6 (retryable transport errors retry with backoff up to
// the configured limit)
#[tokio::test]
async fn retries_retryable_errors_up_to_the_limit() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.error("connection reset", true)
                .usage(fake_usage(1, 0, 0, 0))
        })
        .turn(|t| {
            t.error("connection reset", true)
                .usage(fake_usage(1, 0, 0, 0))
        })
        .turn(|t| t.text("finally").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut h = harness("retry", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        sink.count(|e| matches!(e, TurnEvent::RetryScheduled { .. })),
        2,
        "two retries scheduled before success"
    );
    assert!(sink.texts().contains("finally"));
    match sink.events.iter().find_map(|e| match e {
        TurnEvent::RetryScheduled { attempt, max, .. } => Some((*attempt, *max)),
        _ => None,
    }) {
        Some((1, 3)) => {}
        other => panic!("first retry is attempt 1 of 3, got {other:?}"),
    }
}

// Verifies: FR-CORE-7 (after the retry limit the error shows and control
// returns; the session stays usable)
#[tokio::test]
async fn surfaces_the_error_after_exhausting_retries_and_keeps_the_session() {
    let provider = FakeProvider::builder()
        .turn(|t| t.error("gateway down", true).usage(fake_usage(1, 0, 0, 0)))
        .turn(|t| t.error("gateway down", true).usage(fake_usage(1, 0, 0, 0)))
        .turn(|t| t.text("recovered").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut h = harness("retry-exhaust", provider, {
        let mut config = default_config();
        config.retry_limit = 1;
        config
    });
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Error);
    assert_eq!(outcome.stop_reason, StopReason::Error);
    assert!(
        outcome
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("gateway down")
    );
    assert!(
        sink.events
            .iter()
            .any(|e| matches!(e, TurnEvent::Error { .. })),
        "the error is shown to the interface"
    );

    // The session is still open: the next turn works (FR-CORE-7).
    let outcome = turn(&mut h, "try again", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok, "session survives");
    assert!(sink.texts().contains("recovered"));
}

// Verifies: FR-CORE-9 (a turn past the tool-call iteration limit ends with
// an iteration-limit error)
#[tokio::test]
async fn ends_with_an_iteration_limit_error() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("write", r#"{"path":"a.txt","content":"A"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| {
            t.tool_call("write", r#"{"path":"b.txt","content":"B"}"#)
                .usage(fake_usage(20, 10, 10, 10))
        })
        .turn(|t| t.text("never reached").usage(fake_usage(30, 5, 10, 0)))
        .build();
    let mut h = harness("iteration", provider, {
        let mut config = default_config();
        config.max_iterations = 1;
        config
    });
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "loop", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Error);
    assert_eq!(outcome.stop_reason, StopReason::IterationLimit);
    assert_eq!(h.provider.call_count(), 2, "no third provider call");
    let workspace = &h.project;
    assert!(workspace.join("a.txt").is_file(), "first round ran");
    assert!(!workspace.join("b.txt").exists(), "second round never ran");
}

// Verifies: FR-CORE-5 and FR-CONC-3 (cancel stops the in-flight request;
// completed session records are kept)
#[tokio::test]
async fn cancellation_stops_the_stream_and_keeps_completed_records() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("first")
                .pause(5_000)
                .text("second")
                .usage(fake_usage(10, 5, 0, 10))
        })
        .build();
    let mut h = harness("cancel", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let cancel = CancelFlag::new();
    let canceller = {
        let flag = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            flag.cancel();
        })
    };
    let mut agent = Agent::new(
        &h.store,
        &h.session,
        h.provider.as_ref(),
        &mut h.tools,
        &mut h.grants,
        &mut prompt,
        None,
        h.config.clone(),
    );
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        agent.run_turn("hi", &mut sink, &cancel),
    )
    .await
    .expect("returns promptly");
    canceller.await.expect("canceller");
    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    assert!(sink.texts().contains("first"), "received before cancel");

    let read = h.store.read(&h.session).expect("read");
    assert!(
        read.records
            .iter()
            .any(|r| matches!(r, Record::User { .. })),
        "the user record survives (FR-CORE-5)"
    );
    assert!(
        !read
            .records
            .iter()
            .any(|r| matches!(r, Record::Assistant { .. })),
        "no half-written assistant record"
    );
}

// A denied shell command is recorded as denied and the model continues
// (FR-TOOL-3 through the core; a hook denial that never prompts is
// FR-CORE-10, covered when hooks land in Phase 2).
#[tokio::test]
async fn denied_commands_reach_the_model_as_denied_results() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"rm -rf /"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("understood").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("deny", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![Decision::Denied],
        asked: vec![],
    };

    let outcome = turn(&mut h, "clean up", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(prompt.asked.len(), 1, "the prompt showed the exact command");
    assert!(
        prompt.asked[0].contains("rm -rf /"),
        "FR-UI-4: exact command"
    );
    assert!(
        sink.events.iter().any(
            |e| matches!(e, TurnEvent::ToolFinished(r) if r.status == ToolResultStatus::Denied)
        )
    );

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    assert!(
        read.records.iter().any(|r| matches!(
            r,
            Record::ToolResult {
                status: ToolResultStatus::Denied,
                ..
            }
        )),
        "denial recorded in the log"
    );
    assert!(
        read.records
            .iter()
            .any(|r| matches!(r, Record::Permission { .. })),
        "permission decision recorded"
    );
}

// Verifies: FR-CACHE-5 (the stable-prefix boundary travels on every
// completion call; without a compaction record the dynamic suffix starts at
// the session start, so the prefix is zero)
#[tokio::test]
async fn passes_the_stable_prefix_on_every_completion_call() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("one").usage(fake_usage(10, 5, 0, 10)))
        .turn(|t| t.text("two").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("prefix", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    turn(&mut h, "first", &mut sink, &mut prompt).await;
    turn(&mut h, "second", &mut sink, &mut prompt).await;

    let request = h.provider.last_request().expect("request recorded");
    assert_eq!(request.stable_prefix, 0, "no compaction record yet");
    assert_eq!(request.model, "faux-1");
    let roles: Vec<&str> = request
        .messages
        .iter()
        .map(|m| match m.role {
            lca_protocol::MessageRole::System => "system",
            lca_protocol::MessageRole::User => "user",
            lca_protocol::MessageRole::Assistant => "assistant",
            lca_protocol::MessageRole::Tool => "tool",
        })
        .collect();
    assert_eq!(roles.first(), Some(&"system"), "system message leads");
    assert_eq!(roles.last(), Some(&"user"), "the new user message is last");
    assert!(
        !request.tools.is_empty(),
        "tool specs travel with the request"
    );
    let _ = h.provider.name();
}

// FR-PROV-2's data path: the model picker reads the provider's models.
#[test]
fn model_listings_flow_from_the_provider() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("x").usage(fake_usage(1, 1, 0, 0)))
        .build();
    let models = Provider::list_models(&provider);
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "faux-1");
}
