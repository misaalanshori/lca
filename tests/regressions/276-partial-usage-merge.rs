//! GitHub #276: `Usage` was assigned as a whole struct at every stream
//! accumulator site, so a provider that reports its counters in more than
//! one stream event had the later event's zeros wipe the earlier event's
//! numbers. Anthropic's SSE lifecycle is exactly that shape: `message_start`
//! carries `input_tokens` / `cache_read_input_tokens` /
//! `cache_creation_input_tokens` (with `output_tokens` 0), and
//! `message_delta` carries `output_tokens` (with the prompt counters absent,
//! which the decoder maps to 0).
//!
//! The consequence was not cosmetic. `usage_prompt_tokens` (= input + cache)
//! read 0, so `maybe_compact`'s threshold never fired — a long Anthropic
//! session grew until the provider rejected it (FR-SESS-4 broken) — and
//! every `Record::Assistant` persisted zeros, under-reporting the footer,
//! `/session`, and `/usage`.
//!
//! The fix is one seam: `lca_protocol::Usage::absorb` merges per field (a
//! later NON-ZERO value wins; zero means "not reported in this event"),
//! used at every accumulator site.
//!
//! Verifies: the whole turn through the agent loop — a split-usage stream
//! still lands the merged counters on the assistant record, the prompt
//! count stays non-zero, and auto-compaction fires off it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_core::{Agent, AgentConfig, ExtensionRegistry, StopReason, TurnEvent, TurnSink};
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_protocol::{CommandEffect, DispatchError};
use lca_protocol::{Record, Usage};
use lca_session::{SessionStore, ViewMode};
use lca_testkit::FakeProvider;
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct CollectingSink {
    _events: Vec<TurnEvent>,
}

impl TurnSink for CollectingSink {
    fn on_event(&mut self, event: TurnEvent) {
        self._events.push(event);
    }
}

/// A minimal compaction strategy, so the threshold path has something to
/// call when the merged prompt count crosses it.
struct CompactDouble;

impl ExtensionDispatch for CompactDouble {
    fn name(&self) -> &str {
        "compact-double"
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::Compaction]
    }
    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.name().to_string(),
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
        let summary = format!("compacted {} records", records.len());
        Box::pin(std::future::ready(Ok(summary)))
    }
    fn transform_messages(
        &self,
        messages: Vec<lca_protocol::ChatMessage>,
    ) -> DispatchFuture<
        'static,
        Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
    > {
        Box::pin(std::future::ready(Ok(Ok(messages))))
    }
    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
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

struct NoPrompt;

impl PermissionPrompt for NoPrompt {
    fn ask(&mut self, _action: &Action) -> Decision {
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

struct Harness {
    root: PathBuf,
    store: SessionStore,
    session: lca_session::Session,
    grants: Arc<Mutex<GrantStore>>,
    tools: ToolExecutor,
    provider: FakeProvider,
    config: AgentConfig,
}

fn harness(name: &str, provider: FakeProvider, config: AgentConfig) -> Harness {
    let root = lca_testkit::scratch_path(name);
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
        Some(Duration::from_secs(30)),
    );
    Harness {
        root,
        store,
        session,
        grants,
        tools,
        provider,
        config,
    }
}

async fn run_turn(h: &mut Harness, input: &str) -> lca_core::TurnOutcome {
    let mut sink = CollectingSink::default();
    let mut prompt = NoPrompt;
    let mut agent = Agent::new(
        &h.store,
        &h.session,
        &h.provider,
        &mut h.tools,
        h.grants.clone(),
        &mut prompt,
        None,
        h.config.clone(),
    );
    agent.run_turn(input, &mut sink, &CancelFlag::new()).await
}

fn config() -> AgentConfig {
    AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 3,
        retry_base_delay: Duration::ZERO,
        max_iterations: 50,
        ..AgentConfig::default()
    }
}

/// The Anthropic two-event shape: the prompt half first (`message_start`),
/// the output half last (`message_delta`).
fn anthropic_split_turn(
    builder: lca_testkit::TurnBuilder,
    input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
) -> lca_testkit::TurnBuilder {
    builder
        .usage_event(Usage {
            input,
            cache_read,
            cache_write,
            ..Default::default()
        })
        .usage(Usage {
            output,
            ..Default::default()
        })
}

// Verifies: gh #276 (the delta's zeros never erase the start's prompt
// counters on the persisted record)
#[tokio::test]
async fn a_split_usage_stream_persists_the_merged_counters() {
    let provider = FakeProvider::builder()
        .turn(|t| anthropic_split_turn(t.text("answer"), 1200, 4000, 300, 87))
        .build();
    let mut h = harness("276-merged", provider, config());

    let outcome = run_turn(&mut h, "hi").await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        (
            outcome.usage.input,
            outcome.usage.output,
            outcome.usage.cache_read,
            outcome.usage.cache_write
        ),
        (1200, 87, 4000, 300),
        "the merged report, not the last event"
    );
    assert_eq!(
        outcome.usage.prompt_tokens(),
        1200 + 4000 + 300,
        "prompt tokens are the MERGED report"
    );

    let read = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records");
    let assistant = read
        .records
        .iter()
        .rev()
        .find_map(|record| match record {
            Record::Assistant { usage, .. } => usage.clone(),
            _ => None,
        })
        .expect("an assistant record");
    assert_eq!(
        (assistant.input, assistant.output, assistant.cache_read),
        (1200, 87, 4000),
        "the session log carries the merged usage (footer, /session, /usage read this)"
    );
    assert!(
        assistant.prompt_tokens() > 0,
        "a zero prompt count is the defect: it disables the compaction trigger"
    );
}

// Verifies: gh #276 (a single-event provider is unaffected - the OpenAI
// shape absorbs to exactly itself, so its records are byte-identical)
#[tokio::test]
async fn a_single_usage_event_provider_is_unchanged() {
    let reported = Usage {
        input: 500,
        output: 60,
        cache_read: 900,
        cache_write: 100,
        cache_write_1h: 5,
        cost: 0.021,
        ..Default::default()
    };
    let provider = FakeProvider::builder()
        .turn({
            let reported = reported.clone();
            move |t| t.text("answer").usage(reported)
        })
        .build();
    let mut h = harness("276-single", provider, config());

    let outcome = run_turn(&mut h, "hi").await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        outcome.usage, reported,
        "one full report absorbs to itself: no behaviour change for OpenAI-shaped providers"
    );
}

// Verifies: gh #276 + FR-SESS-4 (auto-compaction fires off the merged
// prompt count - the defect's real consequence)
#[tokio::test]
async fn auto_compaction_fires_off_a_split_usage_stream() {
    // 9000 prompt tokens over a 10_000 window at threshold 0.5: the
    // second turn's pre-flight check must see 9000, not 0.
    let provider = FakeProvider::builder()
        .turn(|t| anthropic_split_turn(t.text("ok"), 7000, 2000, 0, 10))
        .turn(|t| {
            t.text("summarized").usage(Usage {
                input: 50,
                output: 10,
                ..Default::default()
            })
        })
        .build();
    let mut cfg = config();
    cfg.model_context_window = 10_000;
    cfg.compaction_threshold = 0.5;
    cfg.compaction_keep_recent_tokens = 0;
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(CompactDouble));
    cfg.extensions = Arc::new(registry);
    let mut h = harness("276-compaction", provider, cfg);

    run_turn(&mut h, "first").await;
    let outcome = run_turn(&mut h, "second").await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let read = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records");
    assert!(
        read.records
            .iter()
            .any(|record| matches!(record, Record::Compaction { .. })),
        "the merged prompt count crossed the threshold and compaction ran; \
         a wiped record reads 0 and this branch is unreachable"
    );
    // Silence the unused-field lint on the harness's root (kept so the
    // scratch dir lives as long as the test).
    assert!(h.root.exists());
}
