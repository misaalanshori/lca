//! Agent loop tests: the core turn flow against the fake provider, with
//! every FR it verifies named on the test.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lca_core::{
    Agent, AgentConfig, ExtensionRegistry, StopReason, TurnEvent, TurnOutcome, TurnSink,
};
use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_protocol::{CommandEffect, Record, ToolCall, ToolResultStatus};
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
    root: PathBuf,
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

// Verifies: FR-TOOL-3 - a read outside the workspace reaches the permission
// prompt before it runs, and a denial returns to the model as a denied result.
#[tokio::test]
async fn reading_outside_the_workspace_asks_before_it_runs() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("read", r#"{"path":"../outside.txt"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("cannot").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("read-out", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![Decision::Denied],
        asked: vec![],
    };

    let outcome = turn(&mut h, "read it", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(prompt.asked.len(), 1, "the outside read asks first");
    assert!(
        prompt.asked[0].contains("outside.txt"),
        "the exact path is shown: {:?}",
        prompt.asked
    );
    assert!(
        sink.events.iter().any(
            |e| matches!(e, TurnEvent::ToolFinished(r) if r.status == ToolResultStatus::Denied)
        ),
        "the denial reaches the model"
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
    // With no compaction record the boundary grows up to (but never
    // includes) the current turn's message: [system, first] was sent
    // last call, `second` is new (the growth docs/providers/
    // antigravity.md relies on; a compaction record then anchors it).
    assert_eq!(request.stable_prefix, 2, "grows to the previous message");
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

// Verifies: FR-PROV-1 (the core contains no vendor-specific model logic:
// no vendor endpoints, model families, or wire formats appear in it; the
// provider name it defaults to is configuration, not logic)
#[test]
fn core_has_no_vendor_specific_model_logic() {
    let source = include_str!("../src/lib.rs").to_lowercase();
    for vendor_signal in [
        "api.openai.com",
        "anthropic",
        "gpt-4",
        "claude-",
        "chatgpt",
        "generativelanguage",
    ] {
        assert!(
            !source.contains(vendor_signal),
            "vendor signal `{vendor_signal}` in lca-core"
        );
    }
}

// Verifies: FR-PROV-2 (where a provider extension is enabled, its models
// are listed for the picker)
#[test]
fn model_listings_flow_from_the_provider() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("x").usage(fake_usage(1, 1, 0, 0)))
        .build();
    let models = Provider::list_models(&provider);
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "faux-1");
}

// Verifies: NFR-31 (the cache-hit-ratio benchmark) and the Phase 3
// exit test's clause - a scripted clean twenty-turn conversation with
// no compaction and no provider change reports zero cache waste from
// the second turn onward (FR-CACHE-1's absence of misses, ADR-0017's
// whole mechanism against the fake provider's scripted usage).
//
// The threshold was fixed at this Phase 3 exit test, as
// `docs/testing-plan.md` section9 requires: the clean script scores
// about0.97 (each turn's new tokens are paid, the rest comes from the
// cache), so anything under0.90 means caching stopped working - which
// produces a correct answer and a passing turn every time, and shows up
// here instead of on the bill.
const NFR31_MIN_CACHE_HIT_RATIO: f64 = 0.90;

#[tokio::test]
async fn twenty_clean_turns_report_zero_cache_waste_and_hold_the_ratio() {
    // The canonical script: turn one writes the cache, every later turn
    // reads the whole previous prompt and pays for a little new input.
    let mut builder = FakeProvider::builder();
    let mut previous_prompt = 0u64;
    for turn in 0..20u64 {
        let (input, cache_read, cache_write) = if turn == 0 {
            (1000, 0, 500)
        } else {
            (50, previous_prompt, 0)
        };
        previous_prompt = input + cache_read + cache_write;
        let usage = fake_usage(input, 10, cache_read, cache_write);
        builder = builder.turn(move |t| t.text("ok").usage(usage));
    }
    let provider = builder.build();
    let mut h = harness("cache-ratio", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    for index in 0..20 {
        let outcome = turn(&mut h, &format!("turn {index}"), &mut sink, &mut prompt).await;
        assert_eq!(
            outcome.status,
            lca_core::TurnStatus::Ok,
            "turn {index} completes"
        );
    }

    let records = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records;

    // Zero waste from the second turn onward: no miss anywhere (the
    // first turn has no predecessor, so it can never count).
    let totals = lca_session::compute_cache_waste(&records, 1024);
    assert_eq!(totals.miss_count, 0, "no counted miss: {totals:?}");
    assert_eq!(totals.missed_tokens, 0, "zero wasted tokens: {totals:?}");

    // The ratio itself: cache reads over total prompt, turns2..20.
    let mut reads = 0u64;
    let mut prompts = 0u64;
    let mut turns = 0u64;
    for record in &records {
        if let Record::Assistant {
            usage: Some(usage), ..
        } = record
        {
            let prompt = usage.input + usage.cache_read + usage.cache_write + usage.cache_write_1h;
            if turns > 0 {
                // skip the first turn (measured "from turn two onward")
                reads += usage.cache_read;
                prompts += prompt;
            }
            turns += 1;
        }
    }
    assert!(turns >= 20, "twenty scripted turns recorded: {turns}");
    assert!(prompts > 0, "the conversation had prompt tokens");
    let ratio = reads as f64 / prompts as f64;
    assert!(
        ratio >= NFR31_MIN_CACHE_HIT_RATIO,
        "cache-hit ratio {ratio:.4} below the NFR-31 threshold {NFR31_MIN_CACHE_HIT_RATIO}"
    );
}

// ---------------------------------------------------------------------------
// Phase 4: compaction, context transform, skills, boundary narrowing
// ---------------------------------------------------------------------------

use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::{ChatMessage, DispatchError};
use lca_tools::CompletionBackend as _;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What the transform double does per call.
#[derive(Clone, Copy)]
enum TransformKind {
    /// Pass everything through.
    Pass,
    /// Append a skill marker to the last message (skills injection).
    SkillsInject,
    /// Refuse every call with a fixed reason (FR-CTX-3).
    Reject(&'static str),
    /// Rewrite an early message from the second call on (FR-CACHE-6).
    NarrowFromSecond,
}

/// One double covering both new worlds: compaction counts its calls and
/// returns a summary that proves which range it saw; transforms behave
/// per `kind`.
struct PhaseDouble {
    label: &'static str,
    worlds: Vec<World>,
    compact_calls: Arc<AtomicUsize>,
    transform_calls: Arc<AtomicUsize>,
    kind: TransformKind,
}

impl PhaseDouble {
    fn strategy(label: &'static str, calls: Arc<AtomicUsize>) -> PhaseDouble {
        PhaseDouble {
            label,
            worlds: vec![World::Compaction],
            compact_calls: calls,
            transform_calls: Arc::new(AtomicUsize::new(0)),
            kind: TransformKind::Pass,
        }
    }

    fn transform(label: &'static str, kind: TransformKind, calls: Arc<AtomicUsize>) -> PhaseDouble {
        PhaseDouble {
            label,
            worlds: vec![World::ContextTransform],
            compact_calls: Arc::new(AtomicUsize::new(0)),
            transform_calls: calls,
            kind,
        }
    }
}

impl ExtensionDispatch for PhaseDouble {
    fn name(&self) -> &str {
        self.label
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<World> {
        self.worlds.clone()
    }

    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.label.to_string(),
            world: "tool",
        })))
    }

    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Ok(CommandEffect::None)
    }

    fn compact(
        &self,
        records: &[lca_protocol::Record],
    ) -> DispatchFuture<'static, Result<String, DispatchError>> {
        self.compact_calls.fetch_add(1, Ordering::SeqCst);
        // The summary names the range it saw, which is how the tests
        // prove what the host handed over and what later reads reuse.
        let summary = format!("compacted {} records", records.len());
        Box::pin(std::future::ready(Ok(summary)))
    }

    fn transform_messages(
        &self,
        mut messages: Vec<ChatMessage>,
    ) -> DispatchFuture<'static, Result<Result<Vec<ChatMessage>, String>, DispatchError>> {
        self.transform_calls.fetch_add(1, Ordering::SeqCst);
        let call = self.transform_calls.load(Ordering::SeqCst);
        let outcome = match self.kind {
            TransformKind::Pass => Ok(messages),
            TransformKind::SkillsInject => {
                // Appended, never in-place: skills handling adds the
                // matched instructions as their own message, which is
                // why the cache boundary never sees a rewrite.
                messages.push(ChatMessage::text(
                    lca_protocol::MessageRole::User,
                    "[skill: test-skill] Follow the skill.",
                ));
                Ok(messages)
            }
            TransformKind::Reject(reason) => Err(reason.to_string()),
            TransformKind::NarrowFromSecond => {
                if call == 2
                    && let Some(message) = messages.iter_mut().find(|message| {
                        message.content.iter().any(
                            |block| matches!(block, lca_protocol::ContentBlock::Text { text } if text.contains("compacted")),
                        )
                    })
                {
                    // A rewrite of the compacted summary: the durable,
                    // claimed-region divergence case FR-CACHE-6 exists
                    // for (never persisted - FR-CTX-4).
                    message.content.push(lca_protocol::ContentBlock::Text {
                        text: " [rewritten]".to_string(),
                    });
                }
                Ok(messages)
            }
        };
        Box::pin(std::future::ready(Ok(outcome)))
    }

    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
    }

    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a lca_protocol::PostToolObservation,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

fn phase_config(registry: ExtensionRegistry, window: u32, threshold: f64) -> AgentConfig {
    AgentConfig {
        extensions: Arc::new(registry),
        model_context_window: window,
        compaction_threshold: threshold,
        ..default_config()
    }
}

fn registry_with(handle: PhaseDouble) -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(handle));
    registry
}

fn registry_two(first: PhaseDouble, second: PhaseDouble) -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(first));
    registry.register(Arc::new(second));
    registry
}

// Verifies: FR-SESS-4 and FR-SESS-5 (crossing the configured threshold
// invokes the compaction extension, once), FR-CTX-1 (the summary is
// written as a durable record and reused across a restart without
// another invocation), and FR-CACHE-5's recomputation (the stable
// boundary now lands after the summary). Phase 3 built the measurement;
// this is the reset mechanism completing ADR-0017.
#[tokio::test]
async fn crossing_the_threshold_compacts_once_and_the_summary_survives_a_restart() {
    // One big turn (crosses0.5 of a10k window), then small turns.
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(5000, 10, 4000, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(PhaseDouble::strategy("phase-double", calls.clone()));
    let mut h = harness(
        "compact-once",
        provider,
        phase_config(registry, 10_000, 0.5),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };

    // Turn1: no previous usage, nothing crosses.
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing to cross yet");
    // Turn2: turn1's usage crosses -> compact before the provider call.
    turn(&mut h, "second", &mut sink, &mut prompt).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "crossing invokes the strategy exactly once (FR-SESS-4)"
    );
    // Turn3: post-compaction usage is small -> no second invocation
    // (FR-CTX-1: reused until usage next crosses).
    turn(&mut h, "third", &mut sink, &mut prompt).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "reused, not recomputed");

    let records = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records;
    let compactions: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            Record::Compaction {
                summary, strategy, ..
            } => Some((summary, strategy)),
            _ => None,
        })
        .collect();
    assert_eq!(compactions.len(), 1, "one durable record (FR-CTX-1)");
    assert_eq!(compactions[0].0, "compacted 2 records", "the range it saw");
    assert_eq!(
        compactions[0].1, "phase-double",
        "FR-SESS-5's strategy name"
    );

    // The summary reaches the model: assembly surfaces it and marks the
    // cache boundary right after it (FR-CACHE-5).
    let assembled = lca_core::assemble(&records, "sys");
    let summary_index = assembled
        .messages
        .iter()
        .position(|message| message.content.iter().any(
            |block| matches!(block, lca_protocol::ContentBlock::Text { text } if text == "compacted 2 records"),
        ))
        .expect("the summary is a message the model sees");
    assert_eq!(
        assembled.stable_prefix,
        summary_index + 1,
        "everything through the summary is the stable prefix"
    );

    // Restart: a fresh store over the same directory still shows the
    // summary, and the strategy is not asked again (exit test clause2).
    let store2 = SessionStore::new(h.root.join("data"));
    let session2 = store2
        .session(&h.project, h.session.id())
        .expect("session survives");
    let records2 = store2
        .read_with(&session2, ViewMode::Display)
        .expect("reread")
        .records;
    assert_eq!(
        records2
            .iter()
            .filter(|record| matches!(record, Record::Compaction { .. }))
            .count(),
        1,
        "the record persisted"
    );
    let mut agent = Agent::new(
        &store2,
        &session2,
        h.provider.as_ref(),
        &mut h.tools,
        &mut h.grants,
        &mut prompt,
        None,
        h.config.clone(),
    );
    let outcome = agent
        .run_turn("after restart", &mut sink, &CancelFlag::new())
        .await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a resumed session reuses the summary (FR-CTX-1)"
    );
}

// Verifies: FR-CACHE-2 end to end - a counted miss before compaction,
// the baseline reset exactly on the compaction record, and zero waste
// on every turn after it (the Phase 3 exit test's third clause).
#[tokio::test]
async fn the_cache_baseline_resets_exactly_on_the_compaction_record() {
    // t1 reports caching (write), t2 reads none of the old prompt: a
    //1500-token miss above the1024 floor, before any compaction. t3
    // crosses the threshold, t4 compacts first, then stays small.
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(900, 10, 0, 600)))
        .turn(|t| t.text("ok").usage(fake_usage(2000, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(3000, 10, 2000, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(PhaseDouble::strategy("phase-double", calls.clone()));
    let mut h = harness("cache-reset", provider, phase_config(registry, 10_000, 0.5));
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    for index in 0..5 {
        let outcome = turn(&mut h, &format!("turn {index}"), &mut sink, &mut prompt).await;
        assert_eq!(outcome.status, lca_core::TurnStatus::Ok, "turn {index}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "compacted exactly once");

    // Waste is measured over the whole log (audit view): suppressed
    // records still cost real money, and the compaction record itself
    // resets the baseline where it sits (FR-CACHE-2).
    let records = h.store.read(&h.session).expect("records").records;
    let compaction_id = records
        .iter()
        .find_map(|record| match record {
            Record::Compaction { id, .. } => Some(id.clone()),
            _ => None,
        })
        .expect("a compaction record");
    let misses = lca_session::collect_cache_misses(&records, 1024);
    assert!(
        !misses.is_empty(),
        "the scripted miss before compaction was counted"
    );
    for miss in &misses {
        assert!(
            miss.record_id < compaction_id,
            "miss {} landed after the compaction record {}",
            miss.record_id,
            compaction_id
        );
    }
    // The totals a user sees reset with it: nothing after the record
    // counted (FR-CACHE-1's zero-from-here, measured on the records).
    let after: Vec<Record> = records
        .iter()
        .skip_while(|record| record.id() != Some(compaction_id.as_str()))
        .cloned()
        .collect();
    assert!(
        lca_session::collect_cache_misses(&after, 1024).is_empty(),
        "zero waste on every turn after (exit test clause3)"
    );
}

// Verifies: FR-CTX-2 (the chain runs before the provider call) and the
// Phase 4 exit clause4: skills handling injects matched instructions
// through the chain on an ordinary turn without affecting the cache
// boundary - the boundary is the one FR-CACHE-5 computes from the most
// recent compaction record, and the injection rides outside it.
#[tokio::test]
async fn skills_inject_through_the_chain_without_moving_the_cache_boundary() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(5000, 10, 4000, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let strategy_calls = Arc::new(AtomicUsize::new(0));
    let transform_calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_two(
        PhaseDouble::strategy("phase-double", strategy_calls.clone()),
        PhaseDouble::transform(
            "skills",
            TransformKind::SkillsInject,
            transform_calls.clone(),
        ),
    );
    let mut h = harness(
        "skills-inject",
        provider,
        phase_config(registry, 10_000, 0.5),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    // Turn2 crosses the threshold and compacts first; the boundary is
    // everything through the summary (FR-CACHE-5).
    turn(&mut h, "second", &mut sink, &mut prompt).await;
    turn(&mut h, "third", &mut sink, &mut prompt).await;
    assert_eq!(strategy_calls.load(Ordering::SeqCst), 1, "compacted once");
    assert!(
        transform_calls.load(Ordering::SeqCst) >= 3,
        "every provider call transforms"
    );

    let request = h.provider.last_request().expect("the last request");
    let records = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records;
    let natural = lca_core::assemble(&records, &h.config.system_prompt);
    assert_eq!(
        request.stable_prefix, natural.stable_prefix,
        "the injection did not move the boundary (exit clause4)"
    );
    assert!(request.stable_prefix >= 2, "a boundary exists to protect");
    let marker = "[skill: test-skill]";
    assert!(
        request.messages[request.stable_prefix..]
            .iter()
            .any(|message| message.content.iter().any(
                |block| matches!(block, lca_protocol::ContentBlock::Text { text } if text.contains(marker))
            )),
        "the injection rode in the un-stable region"
    );
    assert!(
        request.messages[..request.stable_prefix]
            .iter()
            .all(|message| message.content.iter().all(
                |block| matches!(block, lca_protocol::ContentBlock::Text { text } if !text.contains(marker))
            )),
        "the stable region is byte-identical"
    );
    assert_eq!(
        request.messages[..request.stable_prefix],
        natural.messages[..natural.stable_prefix],
        "the stable region matches an untransformed assembly"
    );
}

// Verifies: FR-CTX-3 and the Phase 4 exit clause5 - a rejection ends
// the turn with the reason surfaced and the provider never called
// (FR-CTX-4: nothing of the transform reaches the log either).
#[tokio::test]
async fn a_transform_rejection_ends_the_turn_before_the_provider() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("never sent").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(PhaseDouble::transform(
        "rejecter",
        TransformKind::Reject("skills policy: refused this request"),
        calls.clone(),
    ));
    let mut h = harness(
        "reject-turn",
        provider,
        phase_config(registry, 100_000, 0.9),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    let outcome = turn(&mut h, "hello", &mut sink, &mut prompt).await;
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Error,
        "the turn ended"
    );
    assert_eq!(outcome.stop_reason, lca_core::StopReason::Error);
    let error = outcome.error.expect("the reason is surfaced");
    assert!(error.contains("skills policy: refused"), "{error}");
    assert_eq!(
        h.provider.call_count(),
        0,
        "the provider was never called (FR-CTX-3)"
    );
    assert!(
        sink.events.iter().any(|event| matches!(
            event,
            TurnEvent::TurnEnded {
                status: lca_core::TurnStatus::Error,
                ..
            }
        )),
        "the interface saw the turn end"
    );
    let records = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records;
    assert!(
        !records
            .iter()
            .any(|record| matches!(record, Record::Assistant { .. } | Record::ToolCall { .. })),
        "nothing the transform touched was persisted (FR-CTX-4)"
    );
}

// Verifies: FR-CACHE-6 - content inside the stable boundary that
// differs from what actually went out on the previous call narrows the
// boundary to end before the earliest differing message, records one
// extension event rather than rejecting, and settles (no further
// narrowing, no repeat event) once the output stops changing.
#[tokio::test]
async fn stable_region_divergence_narrows_the_boundary_once_and_settles() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(5000, 10, 4000, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let strategy_calls = Arc::new(AtomicUsize::new(0));
    let transform_calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_two(
        PhaseDouble::strategy("phase-double", strategy_calls.clone()),
        PhaseDouble::transform(
            "narrower",
            TransformKind::NarrowFromSecond,
            transform_calls.clone(),
        ),
    );
    let mut h = harness(
        "narrow-boundary",
        provider,
        phase_config(registry, 10_000, 0.5),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };

    // Turn1: no compaction record yet, so the boundary reaches up to
    // (but never includes) this turn's own message: just the system
    // prompt side.
    let outcome = turn(&mut h, "first", &mut sink, &mut prompt).await;
    eprintln!("T1 {:?}", (outcome.status, outcome.error));
    let first = h.provider.last_request().expect("request one");
    assert_eq!(
        first.stable_prefix, 1,
        "grows turn by turn, excludes the new message"
    );

    // Turn2: threshold crossing compacts first; the transform's first
    // rewrite lands on the summary, but nothing was sent before, so
    // there is nothing to compare against yet.
    let outcome = turn(&mut h, "second", &mut sink, &mut prompt).await;
    eprintln!("T2 {:?}", (outcome.status, outcome.error));
    let second = h.provider.last_request().expect("request two");
    assert_eq!(
        second.stable_prefix, 1,
        "post-compaction the boundary still never claims this turn's message"
    );
    assert_eq!(
        count_extension_events(&h, "cache-boundary-narrowed"),
        0,
        "the rewritten summary lies outside the claimed boundary so far"
    );

    // Turn3: the transform stops rewriting, so the stable region now
    // differs from what actually went out on turn2 - narrowed once,
    // recorded once, and the turn still succeeds (never a rejection).
    let outcome = turn(&mut h, "third", &mut sink, &mut prompt).await;
    eprintln!("T3 {:?}", (outcome.status, outcome.error));
    let third = h.provider.last_request().expect("request three");
    // The summary is now claimable and was sent rewritten on turn2 but
    // arrives clean on turn3: the boundary ends before it (FR-CACHE-6).
    assert_eq!(
        third.stable_prefix, 2,
        "the boundary ends before the message that diverged (FR-CACHE-6)"
    );
    assert_eq!(
        count_extension_events(&h, "cache-boundary-narrowed"),
        1,
        "one divergence recorded as an extension event"
    );

    // Turn4: settled - same content as last sent, narrowed no further.
    let outcome = turn(&mut h, "fourth", &mut sink, &mut prompt).await;
    eprintln!("T4 {:?}", (outcome.status, outcome.error));
    let fourth = h.provider.last_request().expect("request four");
    assert_eq!(
        fourth.stable_prefix, 3,
        "settled: content matches what was sent, narrowing goes no further"
    );
    assert_eq!(
        count_extension_events(&h, "cache-boundary-narrowed"),
        1,
        "no repeat event once settled"
    );
    // Raw log: the compacted turn's assistant is hidden from the
    // display view by design, but every turn did succeed.
    let records = h.store.read(&h.session).expect("records").records;
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record, Record::Assistant { .. }))
            .count(),
        4,
        "every turn kept succeeding: divergence records, never rejects"
    );
}

fn count_extension_events(h: &Harness, event: &str) -> usize {
    h.store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records
        .iter()
        .filter(|record| matches!(record, Record::ExtensionEvent { event: found, .. } if found == event))
        .count()
}

// Verifies: the Phase 4 exit test's first two clauses against the REAL
// default strategy and the REAL completion backend (not doubles):
// crossing the configured threshold triggers `compaction-default`, it
// asks the active provider through the `completion` capability, the
// model's answer is the summary, and the summarization usage lands on
// the durable record (capability catalog: spend shows in session cost).
#[tokio::test]
async fn the_default_strategy_compacts_through_the_real_completion_backend() {
    // Script order: turn1's reply (big usage - the crossing), the
    // summarization the strategy will ask for, then small replies.
    let provider = FakeProvider::builder()
        .turn(|t| t.text("first reply").usage(fake_usage(5000, 40, 4000, 0)))
        .turn(|t| {
            t.text("SYNTHESIS: the parser was added and tested.")
                .usage(fake_usage(1000, 40, 4000, 0))
        })
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let provider = Arc::new(provider);
    // The completion backend wraps the SAME provider the agent talks
    // to: the capability's star topology with the host at the center.
    let backend = Arc::new(lca_core::ext_provider::ProviderBackend::new(
        provider.clone(),
        "faux-1",
        "session-under-test",
    ));
    let cap = Arc::new(lca_tools::Capabilities::new(
        "compaction-default",
        compaction_default::manifest_grants(),
        scratch_roots("default-strategy"),
        Arc::new(std::sync::Mutex::new(Prompt {
            answers: Vec::new(),
            asked: Vec::new(),
        })),
        Arc::new(std::sync::Mutex::new(
            GrantStore::open(&scratch("default-strategy-grants").join("grants.json"))
                .expect("grants"),
        )),
        scratch("default-strategy-project"),
        None,
    ));
    cap.set_completion(backend.clone());
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(compaction_default::CompactionDefault::new(
        cap.clone(),
    )));
    let mut config = phase_config(registry, 10_000, 0.5);
    config.completion_backend = Some(backend.clone());

    let root = scratch("default-strategy");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = GrantStore::open(&root.join("grants.json")).expect("grants");
    let mut tools = ToolExecutor::new(
        Arc::new(NativeOps),
        project.clone(),
        project.clone(),
        65536,
        Duration::from_secs(30),
    );
    let mut grants = grants;
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    // Turn1: no previous usage, nothing crosses; its reply's usage is
    // what the next turn measures.
    let outcome = {
        let mut agent = Agent::new(
            &store,
            &session,
            provider.as_ref(),
            &mut tools,
            &mut grants,
            &mut prompt,
            None,
            config.clone(),
        );
        agent
            .run_turn("big first turn", &mut sink, &CancelFlag::new())
            .await
    };
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "{:?}",
        outcome.error
    );

    // Turn2: turn1's usage crosses the threshold; the default strategy
    // asks the active provider through `completion` for the summary.
    let outcome = {
        let mut agent = Agent::new(
            &store,
            &session,
            provider.as_ref(),
            &mut tools,
            &mut grants,
            &mut prompt,
            None,
            config.clone(),
        );
        agent
            .run_turn("second turn", &mut sink, &CancelFlag::new())
            .await
    };
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "{:?}",
        outcome.error
    );

    let records = store
        .read_with(&session, ViewMode::Display)
        .expect("records")
        .records;
    let compaction = records
        .iter()
        .find_map(|record| match record {
            Record::Compaction {
                summary,
                strategy,
                usage,
                replaced_from,
                replaced_to,
                ..
            } => Some((
                summary.clone(),
                strategy.clone(),
                usage.clone(),
                replaced_from.clone(),
                replaced_to.clone(),
            )),
            _ => None,
        })
        .expect("the threshold triggered the default strategy (FR-SESS-4)");
    assert_eq!(
        compaction.0, "SYNTHESIS: the parser was added and tested.",
        "the model's answer is the summary"
    );
    assert_eq!(
        compaction.1, "compaction-default",
        "FR-SESS-5's strategy name"
    );
    let usage = compaction
        .2
        .expect("the summarization usage is on the record");
    assert_eq!(
        usage.input, 1000,
        "billed input excludes the fake's cache read"
    );
    assert_eq!(
        usage.cache_read, 4000,
        "the completion call's cache fields carried"
    );
    assert!(!compaction.3.is_empty(), "a range was named");
    assert_ne!(compaction.3, compaction.4, "from != to");

    // Turn3: reuses it - no second invocation, no recompute (FR-CTX-1,
    // exit clause2).
    let outcome = {
        let mut agent = Agent::new(
            &store,
            &session,
            provider.as_ref(),
            &mut tools,
            &mut grants,
            &mut prompt,
            None,
            config.clone(),
        );
        agent
            .run_turn("third turn", &mut sink, &CancelFlag::new())
            .await
    };
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let records = store
        .read_with(&session, ViewMode::Display)
        .expect("records")
        .records;
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r, Record::Compaction { .. }))
            .count(),
        1,
        "the summary is reused (exit clause2)"
    );

    // The backend drained: a later compaction would attribute its own
    // usage fresh (spend lands per record, not cumulatively).
    assert!(
        backend.take_usage().is_none(),
        "usage was drained onto the record"
    );
}

/// Roots for a throwaway Capabilities engine (pattern of the other
/// fixtures in this file).
fn scratch_roots(name: &str) -> lca_permissions::ScopeRoots {
    let root = scratch(name);
    lca_permissions::ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    }
}

// Verifies: FR-SESS-5 (the manual trigger - the /compact built-in's
// engine - compacts exclusively through the compaction world: the
// strategy runs, the durable record lands with its name, and no
// threshold was crossed to get there - the window here is zero, which
// disables the automatic path entirely).
#[tokio::test]
async fn the_manual_trigger_compacts_through_the_world_without_a_threshold() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(5000, 10, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(PhaseDouble::strategy("phase-double", calls.clone()));
    // window zero: the threshold path (FR-SESS-4) can never fire, so
    // every invocation below is the manual one's doing.
    let mut h = harness("manual-compact", provider, phase_config(registry, 0, 0.0));
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    };
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "zero window: no automatic compaction"
    );

    let store = std::sync::Arc::new(SessionStore::new(h.root.join("data")));
    let session = store.session(&h.project, h.session.id()).expect("reattach");
    let summary = lca_core::compact_now(
        store.clone(),
        session.clone(),
        h.config.extensions.clone(),
        h.config.completion_backend.clone(),
    )
    .expect("manual compaction runs");
    assert!(
        summary.contains("compacted"),
        "the strategy's summary comes back: {summary}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the compaction world ran, once"
    );

    let records = store
        .read_with(&session, ViewMode::Display)
        .expect("read")
        .records;
    let named: Vec<&str> = records
        .iter()
        .filter_map(|record| match record {
            Record::Compaction { strategy, .. } => Some(strategy.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(named, ["phase-double"], "FR-SESS-5's strategy name");

    // A session with nothing worth replacing answers an error and
    // never reaches the strategy.
    let fresh = store.create_session(&h.project, "fresh").expect("fresh");
    let err = lca_core::compact_now(
        store.clone(),
        fresh,
        h.config.extensions.clone(),
        h.config.completion_backend.clone(),
    )
    .expect_err("one message cannot compact into itself");
    assert!(!err.is_empty(), "the refusal explains itself");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the empty session never reached the strategy"
    );
}

// The completion backend asks whichever model the session currently
// uses (ProviderBackend's own contract); /model changes that model for
// the session, compaction included. Called sync, the way the
// compaction strategy reaches it: through a blocking region
// (ADR-0014), never from inside an async poll.
#[test]
fn the_completion_backend_follows_a_model_change() {
    let fake = Arc::new(
        FakeProvider::builder()
            .turn(|t| t.text("hi").usage(fake_usage(10, 2, 0, 0)))
            .turn(|t| t.text("hi again").usage(fake_usage(8, 2, 0, 0)))
            .build(),
    );
    let backend =
        lca_core::ext_provider::ProviderBackend::new(fake.clone(), "startup-model", "sess-1");
    let messages = vec![lca_protocol::ChatMessage::text(
        lca_protocol::MessageRole::User,
        "hello",
    )];
    backend
        .complete(&messages)
        .expect("the scripted turn answers");
    assert_eq!(
        fake.last_request().expect("request").model,
        "startup-model",
        "before any change"
    );

    backend.set_model("switched-model");
    backend
        .complete(&messages)
        .expect("the scripted turn answers");
    assert_eq!(
        fake.last_request().expect("request").model,
        "switched-model",
        "the session's model after /model"
    );
}
