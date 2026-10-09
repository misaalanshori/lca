//! Compaction epic journeys (gh #36 phases 1-2): the phase-1 budget
//! trigger under scripted usage eras and the phase-2 two-compaction
//! iteration. Split from `loop_phases.rs` at the file-size ceiling
//! (gate 11); the doubles here are compaction-only.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_core::ExtensionRegistry;
use lca_protocol::{CommandEffect, Record};
use lca_session::ViewMode;
use lca_testkit::{FakeProvider, fake_usage};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::DispatchError;
use lca_tools::CompletionBackend as _;

/// A compaction-only double: counts calls, records whether each call
/// saw the host's previous-summary marker, and names the range it saw.
struct SeeingDouble {
    calls: Arc<AtomicUsize>,
    saw_previous: Arc<Mutex<Vec<bool>>>,
}

impl ExtensionDispatch for SeeingDouble {
    fn name(&self) -> &str {
        "seeing-double"
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.saw_previous
            .lock()
            .expect("saw_previous")
            .push(records.iter().any(lca_protocol::is_previous_summary));
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

fn seeing_registry(double: SeeingDouble) -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(double));
    registry
}

fn budget_config(
    registry: ExtensionRegistry,
    window: u32,
    threshold: f64,
) -> lca_core::AgentConfig {
    let mut config = default_config();
    config.extensions = Arc::new(registry);
    config.model_context_window = window;
    config.compaction_threshold = threshold;
    // The threshold path in isolation: no keep-recent window, so the
    // candidate is the whole pre-turn range.
    config.compaction_keep_recent_tokens = 0;
    config
}

fn prompt() -> Prompt {
    Prompt {
        answers: Vec::new(),
        asked: Vec::new(),
    }
}

// Verifies: gh #36 phase 1 - the summarization budget derives from
// the reserve (`0.8 × reserve`, pi's shape): 16384 reserves a 13107
// budget, and an unset budget keeps the stopgap constant.
#[test]
fn the_summarization_budget_derives_from_the_reserve() {
    use lca_core::ext_provider::{SUMMARIZATION_MAX_TOKENS, summarization_max_tokens};
    assert_eq!(summarization_max_tokens(16_384), 13_107);
    assert_eq!(summarization_max_tokens(0), SUMMARIZATION_MAX_TOKENS);

    let fake = Arc::new(
        FakeProvider::builder()
            .turn(|t| t.text("hi").usage(fake_usage(10, 2, 0, 0)))
            .build(),
    );
    let backend = lca_core::ext_provider::ProviderBackend::new(fake.clone(), "m", "sess-1");
    let messages = vec![lca_protocol::ChatMessage::text(
        lca_protocol::MessageRole::User,
        "hello",
    )];
    backend.set_summarization_budget(summarization_max_tokens(16_384));
    backend.complete(&messages).expect("answers");
    assert_eq!(
        fake.last_request()
            .expect("request")
            .extras
            .get("max-tokens")
            .map(String::as_str),
        Some("13107"),
        "the derived budget rides the request"
    );
}

// Verifies: gh #36 phase 1 - an absolute reserve replaces the
// fraction: usage far below `threshold × window` still compacts when
// it clears `window − reserve`.
#[tokio::test]
async fn an_absolute_reserve_replaces_the_fraction() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(1500, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let double = SeeingDouble {
        calls: calls.clone(),
        saw_previous: Arc::new(Mutex::new(Vec::new())),
    };
    let registry = seeing_registry(double);
    let mut config = budget_config(registry, 10_000, 0.8);
    // 1500 never crosses 0.8 × 10000, but clears 10000 − 9000.
    config.compaction_reserve_tokens = 9000;
    let mut h = harness("absolute-reserve", provider, config);
    let mut sink = CollectingSink::default();
    let mut prompt = prompt();
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    turn(&mut h, "second", &mut sink, &mut prompt).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the absolute reserve fired where the fraction would not"
    );
}

// Verifies: gh #36 phase 1 - disabled means no compaction and no
// error: the turn runs clean over a crossing usage.
#[tokio::test]
async fn disabled_compaction_never_fires() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(9000, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(9000, 10, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let double = SeeingDouble {
        calls: calls.clone(),
        saw_previous: Arc::new(Mutex::new(Vec::new())),
    };
    let registry = seeing_registry(double);
    let mut config = budget_config(registry, 10_000, 0.5);
    config.compaction_enabled = false;
    let mut h = harness("disabled-compaction", provider, config);
    let mut sink = CollectingSink::default();
    let mut prompt = prompt();
    for index in 0..2 {
        let outcome = turn(&mut h, &format!("turn {index}"), &mut sink, &mut prompt).await;
        assert_eq!(outcome.status, lca_core::TurnStatus::Ok, "turn {index}");
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "never fired, never errored"
    );
}

// Verifies: gh #36 phase 2 - the two-compaction journey: the second
// compaction receives the first summary in-band (refine, not restart)
// while its replaced range starts past the first one.
#[tokio::test]
async fn the_second_compaction_iterates_the_first_summary() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(9000, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(9000, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let double = SeeingDouble {
        calls: calls.clone(),
        saw_previous: Arc::new(Mutex::new(Vec::new())),
    };
    let seen = double.saw_previous.clone();
    let registry = seeing_registry(double);
    let mut h = harness(
        "iterate-summary",
        provider,
        budget_config(registry, 10_000, 0.5),
    );
    let mut sink = CollectingSink::default();
    let mut prompt = prompt();
    for index in 0..4 {
        turn(&mut h, &format!("turn {index}"), &mut sink, &mut prompt).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2, "compacted twice");
    assert_eq!(
        *seen.lock().expect("seen"),
        vec![false, true],
        "the first had nothing to iterate; the second refined the first"
    );
    // The audit view: the second range nests the first, so Display
    // shows one coherent record while the log holds two.
    let records = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("records")
        .records;
    let ranges: Vec<(String, String, String)> = records
        .iter()
        .filter_map(|record| match record {
            Record::Compaction {
                replaced_from,
                replaced_to,
                first_kept_id,
                ..
            } => Some((
                replaced_from.clone(),
                replaced_to.clone(),
                first_kept_id.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(ranges.len(), 2, "two durable records");
    assert!(
        !ranges[1].2.is_empty(),
        "the second anchors its kept boundary"
    );
}

// Verifies: gh #36 phase 3 - file lists union across two manual
// compactions: the second record carries the first's files plus the
// new turn's, and the summary text names them.
#[test]
fn file_lists_union_across_two_compactions() {
    let provider = FakeProvider::builder().build();
    let registry = seeing_registry(SeeingDouble {
        calls: Arc::new(AtomicUsize::new(0)),
        saw_previous: Arc::new(Mutex::new(Vec::new())),
    });
    let h = harness(
        "files-union",
        provider,
        budget_config(registry, 10_000, 0.5),
    );
    let store = Arc::new(lca_session::SessionStore::new(h.root.join("data")));
    let session = store.session(&h.project, h.session.id()).expect("reattach");
    let tool_call = |id: &str, name: &str, path: &str| Record::ToolCall {
        v: lca_protocol::FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        call_id: format!("k-{id}"),
        name: name.to_string(),
        arguments: format!("{{\"path\": \"{path}\"}}"),
        source: lca_protocol::ToolSource::Builtin,
    };
    let user = |id: &str| Record::User {
        v: lca_protocol::FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        content: id.to_string(),
        attachments: Vec::new(),
        queue: None,
    };
    store.append(&session, user("u1")).expect("append");
    store
        .append(&session, tool_call("c1", "read", "old.md"))
        .expect("append");
    lca_core::compact_now(
        store.clone(),
        session.clone(),
        h.config.extensions.clone(),
        h.config.completion_backend.clone(),
        Some("prompt".to_string()),
    )
    .expect("first compaction runs");
    store.append(&session, user("u2")).expect("append");
    store
        .append(&session, tool_call("c2", "edit", "main.rs"))
        .expect("append");
    lca_core::compact_now(
        store.clone(),
        session.clone(),
        h.config.extensions.clone(),
        h.config.completion_backend.clone(),
        Some("prompt".to_string()),
    )
    .expect("second compaction runs");
    let records = store
        .read_with(&session, ViewMode::Audit)
        .expect("read")
        .records;
    let compactions: Vec<&Record> = records
        .iter()
        .filter(|record| matches!(record, Record::Compaction { .. }))
        .collect();
    assert_eq!(compactions.len(), 2, "two durable records");
    match compactions[1] {
        Record::Compaction {
            read_files,
            modified_files,
            summary,
            ..
        } => {
            assert_eq!(
                read_files,
                &vec!["old.md".to_string()],
                "the first's reads survive"
            );
            assert_eq!(
                modified_files,
                &vec!["main.rs".to_string()],
                "plus the new turn's edits"
            );
            assert!(
                summary.contains("<read-files>") && summary.contains("<modified-files>"),
                "the summary text names them: {summary}"
            );
        }
        other => panic!("a compaction record, not {other:?}"),
    }
}

// Verifies: gh #36 phase 3 - the checkpoint records the prompt and a
// later move records the detection, exactly once per move.
#[test]
fn checkpoint_records_and_detects_a_move() {
    let provider = FakeProvider::builder().build();
    let registry = seeing_registry(SeeingDouble {
        calls: Arc::new(AtomicUsize::new(0)),
        saw_previous: Arc::new(Mutex::new(Vec::new())),
    });
    let h = harness(
        "checkpoint-move",
        provider,
        budget_config(registry, 10_000, 0.5),
    );
    let store = Arc::new(lca_session::SessionStore::new(h.root.join("data")));
    let session = store.session(&h.project, h.session.id()).expect("reattach");
    // Each compaction needs fresh compactable records: the previous
    // range suppresses from the view once summarized.
    let compact_with = |id: &str, prompt: Option<String>| {
        store
            .append(
                &session,
                Record::User {
                    v: lca_protocol::FORMAT_VERSION,
                    ts: 1,
                    id: format!("{id}a"),
                    parent: None,
                    content: "hi".to_string(),
                    attachments: Vec::new(),
                    queue: None,
                },
            )
            .expect("append");
        store
            .append(
                &session,
                Record::User {
                    v: lca_protocol::FORMAT_VERSION,
                    ts: 1,
                    id: format!("{id}b"),
                    parent: None,
                    content: "hi again".to_string(),
                    attachments: Vec::new(),
                    queue: None,
                },
            )
            .expect("append");
        lca_core::compact_now(
            store.clone(),
            session.clone(),
            h.config.extensions.clone(),
            h.config.completion_backend.clone(),
            prompt,
        )
        .expect("compaction runs")
    };
    compact_with("u1", Some("prompt A".to_string()));
    compact_with("u2", Some("prompt B".to_string()));
    compact_with("u3", Some("prompt B".to_string()));
    let records = store
        .read_with(&session, ViewMode::Audit)
        .expect("read")
        .records;
    let checkpoints: Vec<Option<&str>> = records
        .iter()
        .filter_map(|record| match record {
            Record::Compaction { system_prompt, .. } => Some(system_prompt.as_deref()),
            _ => None,
        })
        .collect();
    assert_eq!(
        checkpoints,
        vec![Some("prompt A"), Some("prompt B"), Some("prompt B")],
        "every compaction checkpoints its prompt"
    );
    let detections = records
        .iter()
        .filter(|record| {
            matches!(
                record,
                Record::Custom { custom_type, .. }
                if custom_type == lca_core::SYSTEM_PROMPT_CHANGE_TYPE
            )
        })
        .count();
    assert_eq!(
        detections, 1,
        "the A-to-B move records exactly one detection"
    );
}

// Verifies: gh #36 phase 3 - manual compaction keeps nothing and
// anchors its own id (pi's retain-none shape), not an empty string.
#[test]
fn manual_compaction_anchors_its_own_id() {
    let provider = FakeProvider::builder().build();
    let registry = seeing_registry(SeeingDouble {
        calls: Arc::new(AtomicUsize::new(0)),
        saw_previous: Arc::new(Mutex::new(Vec::new())),
    });
    let h = harness(
        "retain-none",
        provider,
        budget_config(registry, 10_000, 0.5),
    );
    let store = Arc::new(lca_session::SessionStore::new(h.root.join("data")));
    let session = store.session(&h.project, h.session.id()).expect("reattach");
    store
        .append(
            &session,
            Record::User {
                v: lca_protocol::FORMAT_VERSION,
                ts: 1,
                id: "u1".to_string(),
                parent: None,
                content: "hi".to_string(),
                attachments: Vec::new(),
                queue: None,
            },
        )
        .expect("append");
    store
        .append(
            &session,
            Record::User {
                v: lca_protocol::FORMAT_VERSION,
                ts: 1,
                id: "u2".to_string(),
                parent: None,
                content: "hi again".to_string(),
                attachments: Vec::new(),
                queue: None,
            },
        )
        .expect("append");
    lca_core::compact_now(
        store.clone(),
        session.clone(),
        h.config.extensions.clone(),
        h.config.completion_backend.clone(),
        None,
    )
    .expect("compaction runs");
    let records = store
        .read_with(&session, ViewMode::Audit)
        .expect("read")
        .records;
    match records.iter().find_map(|record| match record {
        Record::Compaction {
            id, first_kept_id, ..
        } => Some((id, first_kept_id)),
        _ => None,
    }) {
        Some((id, kept)) => assert_eq!(kept, id, "retain-none anchors its own id"),
        None => panic!("a compaction record was written"),
    }
}

// Verifies: gh #36 phase 3 - a capped generation compacts and retries
// once (pi's recovery ordering): the aborted attempt stays visible to
// TurnEnded, the overflow compaction runs, the retry completes.
#[tokio::test]
async fn overflow_compacts_and_retries_once() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("first").usage(fake_usage(50, 10, 0, 0)))
        .turn(|t| {
            t.error(
                "generation hit the token cap (max_tokens 13107) before finishing",
                false,
            )
            .usage(fake_usage(9000, 10, 0, 0))
        })
        .turn(|t| t.text("recovered").usage(fake_usage(50, 10, 0, 0)))
        .build();
    let registry = seeing_registry(SeeingDouble {
        calls: Arc::new(AtomicUsize::new(0)),
        saw_previous: Arc::new(Mutex::new(Vec::new())),
    });
    let mut h = harness(
        "overflow-retry",
        provider,
        budget_config(registry, 10_000, 0.5),
    );
    let fake = h.provider.clone();
    h.config.retry_limit = 0;
    let mut sink = CollectingSink::default();
    let mut prompt = prompt();
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    let outcome = turn(&mut h, "second", &mut sink, &mut prompt).await;
    assert!(outcome.error.is_none(), "the retry completes: {outcome:?}");
    assert!(sink.texts().contains("recovered"), "the retry's text lands");
    assert_eq!(
        fake.requests().len(),
        3,
        "two turns, three calls: one retry"
    );
    assert_eq!(
        sink.count(|event| matches!(
            event,
            lca_core::TurnEvent::CompactionStarted { reason } if reason == "overflow"
        )),
        1,
        "exactly one overflow compaction"
    );
}

// Verifies: gh #36 phase 3 - a still-capped retry surfaces: one
// recovery per turn, never a loop.
#[tokio::test]
async fn a_second_overflow_surfaces_without_another_compaction() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("first").usage(fake_usage(50, 10, 0, 0)))
        .turn(|t| {
            t.error("prompt is too long", false)
                .usage(fake_usage(9000, 10, 0, 0))
        })
        .turn(|t| {
            t.error("prompt is too long", false)
                .usage(fake_usage(9000, 10, 0, 0))
        })
        .build();
    let registry = seeing_registry(SeeingDouble {
        calls: Arc::new(AtomicUsize::new(0)),
        saw_previous: Arc::new(Mutex::new(Vec::new())),
    });
    let mut h = harness(
        "overflow-once",
        provider,
        budget_config(registry, 10_000, 0.5),
    );
    let fake = h.provider.clone();
    h.config.retry_limit = 0;
    let mut sink = CollectingSink::default();
    let mut prompt = prompt();
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    let outcome = turn(&mut h, "second", &mut sink, &mut prompt).await;
    assert!(outcome.error.is_some(), "the second cap surfaces");
    assert_eq!(fake.requests().len(), 3, "no third attempt");
    assert_eq!(
        sink.count(|event| matches!(event, lca_core::TurnEvent::CompactionStarted { .. })),
        1,
        "compacted once, never looped"
    );
}

// Verifies: gh #36 phase 3 - an ordinary provider error never
// compacts: recovery is overflow-only.
#[tokio::test]
async fn a_plain_error_never_compacts() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("first").usage(fake_usage(50, 10, 0, 0)))
        .turn(|t| {
            t.error("model not found", false)
                .usage(fake_usage(50, 10, 0, 0))
        })
        .build();
    let registry = seeing_registry(SeeingDouble {
        calls: Arc::new(AtomicUsize::new(0)),
        saw_previous: Arc::new(Mutex::new(Vec::new())),
    });
    let mut h = harness(
        "plain-error",
        provider,
        budget_config(registry, 10_000, 0.5),
    );
    let fake = h.provider.clone();
    h.config.retry_limit = 0;
    let mut sink = CollectingSink::default();
    let mut prompt = prompt();
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    let outcome = turn(&mut h, "second", &mut sink, &mut prompt).await;
    assert!(outcome.error.is_some(), "the error surfaces");
    assert_eq!(fake.requests().len(), 2, "no retry");
    assert_eq!(
        sink.count(|event| matches!(event, lca_core::TurnEvent::CompactionStarted { .. })),
        0,
        "no compaction on a plain error"
    );
}

/// A compaction double that always vetoes (gh #45).
struct VetoAll;

impl ExtensionDispatch for VetoAll {
    fn name(&self) -> &str {
        "veto-all"
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::HooksCompaction]
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
    fn invoke_command(
        &self,
        _name: &str,
        _argument: &str,
    ) -> Result<lca_protocol::CommandEffect, DispatchError> {
        Ok(lca_protocol::CommandEffect::None)
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
    fn on_session_before_compact<'a>(
        &'a self,
        _reason: &'a str,
    ) -> DispatchFuture<'a, Result<lca_protocol::CompactVerdict, DispatchError>> {
        Box::pin(std::future::ready(Ok(lca_protocol::CompactVerdict::Deny(
            "vetoed for the journey".to_string(),
        ))))
    }
}

// Verifies: gh #45 - a `session_before_compact` veto cancels the
// compaction (no record, strategy uncalled) and names the refusal.
#[tokio::test]
async fn a_compact_veto_cancels_the_compaction() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(9000, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(50, 10, 0, 0)))
        .build();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(SeeingDouble {
        calls: calls.clone(),
        saw_previous: Arc::new(Mutex::new(Vec::new())),
    }));
    registry.register(Arc::new(VetoAll));
    let mut h = harness("vetoed", provider, budget_config(registry, 10_000, 0.5));
    let mut sink = CollectingSink::default();
    let mut prompt = prompt();
    // The first turn banks the usage the threshold fires on; the
    // second turn meets the veto instead of the strategy.
    turn(&mut h, "first", &mut sink, &mut prompt).await;
    let outcome = turn(&mut h, "second", &mut sink, &mut prompt).await;
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "the turn survives the veto"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "strategy never ran");
    let log = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    assert!(
        !log.records
            .iter()
            .any(|record| matches!(record, lca_protocol::Record::Compaction { .. })),
        "no compaction record"
    );
    assert!(
        sink.events.iter().any(|event| matches!(
            event,
            lca_core::TurnEvent::Error { message, .. } if message.contains("vetoed for the journey")
        )),
        "the veto names its reason"
    );
}
