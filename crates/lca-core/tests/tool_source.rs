//! Tool-call source records (gh #128): the log names the real
//! source - built-in calls `Builtin`, extension-handled calls
//! `Extension` - read back from the log, not from events.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_core::AgentConfig;
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_permissions::Decision;
use lca_protocol::{
    CommandEffect, CommandSpec, DispatchError, Record, ToolCall, ToolExposure, ToolSpec,
};
use lca_testkit::{FakeProvider, fake_usage};
use std::sync::Arc;
// ---------------------------------------------------------------------------
// Gh #128: the tool-call record names the real source.
// ---------------------------------------------------------------------------use lca_protocol::{CommandEffect, CommandSpec, ToolCall, ToolExposure, ToolSpec};

/// A one-tool native double: declares `test-tool`, answers `ok`.
struct SourceDouble;

impl ExtensionDispatch for SourceDouble {
    fn name(&self) -> &str {
        "source-double"
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::Tool]
    }
    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Ok(vec![lca_protocol::ToolSpec {
            name: "test-tool".to_string(),
            description: "test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        }])
    }
    fn execute_tool<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Ok(lca_protocol::ToolResult::ok(
            call.call_id.clone(),
            "ok",
        ))))
    }
    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Ok(CommandEffect::None)
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
    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
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

// Verifies: gh #128 - the log records the real source: a built-in call
// is `Builtin`, an extension-handled call is `Extension` (read back
// from the log, not from events).
#[tokio::test]
async fn tool_call_records_name_the_real_source() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"true"}"#)
                .tool_call("test-tool", "{}")
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(SourceDouble));
    let mut h = harness(
        "tool-source",
        provider,
        AgentConfig {
            extensions: Arc::new(registry),
            ..default_config()
        },
    );
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![Decision::Once, Decision::Once],
        asked: vec![],
    };

    let outcome = turn(&mut h, "run both", &mut sink, &mut prompt).await;
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "error: {:?}",
        outcome.error
    );

    let log = h.store.read(&h.session).expect("read");
    let mut sources = std::collections::BTreeMap::new();
    for record in &log.records {
        if let Record::ToolCall { name, source, .. } = record {
            sources.insert(name.clone(), *source);
        }
    }
    assert_eq!(
        sources.get("shell"),
        Some(&lca_protocol::ToolSource::Builtin),
        "built-in call: {sources:?}"
    );
    assert_eq!(
        sources.get("test-tool"),
        Some(&lca_protocol::ToolSource::Extension),
        "extension call: {sources:?}"
    );
}
