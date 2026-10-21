//! GitHub #264: `HeadlessSink::on_event` had no arm for
//! `TurnEvent::ExtensionEvent`, so the row `docs/headless.md`'s event table
//! documents as `extension-event` fell into the catch-all `_ => {}` and was
//! dropped.
//!
//! The harm is a silent contract break, which is the worst kind: a script
//! integrating with `lca -p --mode json` sees a stream that is missing a
//! documented kind, with no error, no gap marker, and nothing to match on. A
//! consumer waiting for a `disabled` event to know an extension was evicted
//! waits forever; one counting lifecycle events undercounts. The docs promised
//! the row, the binary never produced it.
//!
//! Two guards, because the defect has two shapes:
//!
//! 1. **The end-to-end one** (acceptance 1): a real turn, a real agent, a real
//!    extension that fails mid-call, and the real `HeadlessSink` writing JSONL
//!    - asserting the emitted stream carries the documented row.
//! 2. **The vocabulary one** (the drift guard): every `type` the docs' table
//!    documents has a producer in `headless.rs`, so a future `on_event`
//!    refactor cannot quietly re-drop a kind. That one lives beside the sink
//!    it guards (`crates/lca-cli/src/headless.rs`,
//!    `every_documented_event_type_has_a_producer`), because it needs the
//!    module's own source text; this file carries the behavioural half.
//!
//! Also recorded, and deliberately **not** fixed here: `queue-queued` is
//! documented in the same table and is emitted nowhere in the tree at all - a
//! second, separate gap, outside #264's scope. The vocabulary guard names it
//! explicitly as a known gap rather than letting it pass by omission, which is
//! the difference between a tracked gap and an invisible one.
//!
//! Verifies: gh #264 acceptance 1-3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

use lca_cli::HeadlessSink;
use lca_core::{Agent, AgentConfig, TurnEvent, TurnSink};
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_protocol::{CommandEffect, CommandSpec, DispatchError, HookAction, ToolSpec};
use lca_session::SessionStore;
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

/// Writes into a shared buffer so the assertion can read the JSONL.
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An enabled extension whose tool dispatch fails, which is what makes the
/// agent record an `ExtensionEvent { event: "error" }` mid-turn.
///
/// Hand-rolled rather than a `lca-testkit` helper: `lca-testkit` carries the
/// fake provider and fixtures, and one 40-line trait impl here is smaller than
/// a new public helper in a shared crate for a single caller.
struct FailingExtension {
    name: String,
    reason: String,
}

impl ExtensionDispatch for FailingExtension {
    fn name(&self) -> &str {
        &self.name
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::Tool]
    }
    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Ok(vec![ToolSpec {
            name: "flaky".to_string(),
            description: "always fails".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        }])
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        let reason = self.reason.clone();
        Box::pin(std::future::ready(Err(DispatchError::Failed(reason))))
    }
    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Ok(CommandEffect::None)
    }
    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(HookAction::Allow)))
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

/// Denies nothing that matters here; the extension failure is the trigger, not
/// a permission prompt.
struct AllowPrompt;

impl PermissionPrompt for AllowPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        true
    }
}

/// The lines a sink emitted, parsed.
fn lines(buffer: &Arc<Mutex<Vec<u8>>>) -> Vec<serde_json::Value> {
    let bytes = buffer
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    String::from_utf8(bytes)
        .expect("utf-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect()
}

/// A sink over a buffer, at the given mode.
fn sink_for(json: bool) -> (HeadlessSink, Arc<Mutex<Vec<u8>>>) {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let sink = HeadlessSink::with_writer(json, false, Box::new(Buffer(buffer.clone())));
    (sink, buffer)
}

// Verifies: gh #264 acceptance 1 and 2 - the documented row, in the real
// stream, with exactly the fields `docs/headless.md` names.
//
// Driven through a real `Agent`, not by calling `on_event` by hand: the defect
// was that the engine's event never reached the sink's JSONL, and a test that
// skips the engine would have passed against the broken binary.
#[tokio::test]
async fn a_real_turn_emits_the_documented_extension_event_to_json_stdout() {
    let root = lca_testkit::scratch_path("regression-264-turn");
    let _ = std::fs::remove_dir_all(&root);
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
        Some(std::time::Duration::from_secs(30)),
    );

    // The model asks for a tool an *enabled but failing* extension owns. The
    // agent records `ExtensionEvent { event: "error", .. }` when the dispatch
    // fails, which is exactly the kind of row scripts consume.
    let provider = FakeProvider::builder()
        .turn(|t| t.tool_call("flaky", "{}").usage(fake_usage(10, 10, 0, 10)))
        .turn(|t| t.text("done").usage(fake_usage(10, 5, 0, 0)))
        .build();

    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(FailingExtension {
        name: "flaky-ext".to_string(),
        reason: "the guest trapped at call 3".to_string(),
    }));

    let config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 1,
        retry_base_delay: std::time::Duration::ZERO,
        max_iterations: 10,
        extensions: Arc::new(registry),
        ..AgentConfig::default()
    };

    let (mut sink, buffer) = sink_for(true);
    let mut prompt = AllowPrompt;
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
    let _ = agent
        .run_turn("trigger it", &mut sink, &CancelFlag::new())
        .await;

    let rows = lines(&buffer);
    let extension_events: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|row| row["type"] == "extension-event")
        .collect();
    assert!(
        !extension_events.is_empty(),
        "the turn's extension failure reached the JSON stream; rows were:\n{}",
        rows.iter()
            .map(|row| row["type"].as_str().unwrap_or("?"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let row = extension_events[0];
    assert_eq!(row["extension"], "flaky-ext", "{row}");
    assert_eq!(row["event"], "error", "{row}");
    assert!(
        row["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("trapped")),
        "the detail carries the extension's own words: {row}"
    );
    // Acceptance 2: exactly the documented field set, no extras. A script
    // that validates against the table must not trip over a surprise key.
    let mut keys: Vec<&String> = row.as_object().expect("object").keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["detail", "event", "extension", "type"],
        "the documented fields, and only those: {row}"
    );
}

// Verifies: gh #264 acceptance 1 - the same event reaches `--mode rpc`, which
// shares this sink. One arm, both modes; this row is what keeps a future
// second emitter from appearing.
#[tokio::test]
async fn rpc_mode_shares_the_sink_and_so_sees_the_same_event() {
    // `--mode rpc` drives `HeadlessSink` (`crates/lca-cli/src/rpc.rs` builds
    // one via `HeadlessSink::with_writer`), so the sink contract is the RPC
    // contract. This pins the fact rather than the plumbing: if RPC ever grows
    // its own emitter, this assertion's premise dies and the row should be
    // replaced with a real rpc-loop drive.
    let source = include_str!("../../crates/lca-cli/src/rpc.rs");
    assert!(
        source.contains("HeadlessSink"),
        "rpc mode still drives HeadlessSink; a second emitter would be a \
         contract split this guard must catch"
    );
    let (mut sink, buffer) = sink_for(true);
    sink.on_event(TurnEvent::ExtensionEvent {
        extension: "mcp".to_string(),
        event: "connecting".to_string(),
        detail: "2 servers still connecting".to_string(),
    });
    let rows = lines(&buffer);
    assert_eq!(rows[0]["type"], "extension-event", "{rows:?}");
    assert_eq!(rows[0]["event"], "connecting");
}

// Verifies: gh #264 acceptance 3 - text mode stays silent. The event is a
// host diagnostic; printing it to stdout would corrupt the answer a script
// reads in plain `-p` mode.
#[test]
fn text_mode_prints_no_extension_event() {
    let (mut sink, buffer) = sink_for(false);
    sink.on_event(TurnEvent::ExtensionEvent {
        extension: "host".to_string(),
        event: "cache-boundary-narrowed".to_string(),
        detail: "transform changed the stable prefix".to_string(),
    });
    assert!(
        buffer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_empty(),
        "plain -p writes nothing for an extension event"
    );
}
