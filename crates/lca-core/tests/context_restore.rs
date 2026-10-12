//! Context-transform completeness (gh #46, pi's `context` /
//! `context_with_system`): transforms see the full transcript including
//! the system prompt, and a filter/slice cannot drop the prompt - the
//! host restores it, so every handler and the provider keep the
//! declarations.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_core::ExtensionRegistry;
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::{
    ChatMessage, CommandEffect, CommandSpec, ContentBlock, DispatchError, HookAction, MessageRole,
    PostToolObservation, Record, SettleDecision, ToolCall, ToolResult, ToolSpec,
};
use lca_session::ViewMode;
use lca_testkit::{FakeProvider, fake_usage};
use std::sync::{Arc, Mutex};

/// One configurable transform double: observes the roles it saw, then
/// either passes the list through or slices the leading system message
/// off (the hostile filter gh #46 guards against).
#[derive(Clone, Copy)]
enum ProbeMode {
    Observe,
    SliceSystem,
}

struct ProbeTransform {
    name: String,
    mode: ProbeMode,
    seen: Arc<Mutex<Vec<MessageRole>>>,
}

impl ExtensionDispatch for ProbeTransform {
    fn name(&self) -> &str {
        &self.name
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::ContextTransform]
    }
    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name.clone(),
            world: "tool",
        })
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.name.clone(),
            world: "tool",
        })))
    }
    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name.clone(),
            world: "command",
        })
    }
    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name.clone(),
            world: "command",
        })
    }
    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(HookAction::Allow)))
    }
    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a PostToolObservation,
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
    fn transform_messages(
        &self,
        messages: Vec<ChatMessage>,
    ) -> DispatchFuture<'static, Result<Result<Vec<ChatMessage>, String>, DispatchError>> {
        *self.seen.lock().expect("seen") = messages.iter().map(|message| message.role).collect();
        let out = match self.mode {
            ProbeMode::Observe => messages,
            ProbeMode::SliceSystem => messages
                .into_iter()
                .skip_while(|message| message.role == MessageRole::System)
                .collect(),
        };
        // The future is `'static`: the seen roles went into the shared
        // slot above, so nothing borrows `self` here.
        Box::pin(std::future::ready(Ok(Ok(out))))
    }
}

fn system_user() -> Vec<ChatMessage> {
    vec![
        ChatMessage::text(MessageRole::System, "be brief"),
        ChatMessage::text(MessageRole::User, "hi"),
    ]
}

fn first_text(message: &ChatMessage) -> String {
    message
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

// Verifies: gh #46 - a transform sees the full transcript including the
// system prompt (pi's `context_with_system` visibility).
#[tokio::test]
async fn transforms_see_the_full_transcript_including_system() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(ProbeTransform {
        name: "observe".to_string(),
        mode: ProbeMode::Observe,
        seen: seen.clone(),
    }));
    let output = registry
        .transform(system_user())
        .await
        .expect("pass-through");
    assert_eq!(output.len(), 2);
    assert_eq!(
        *seen.lock().expect("seen"),
        vec![MessageRole::System, MessageRole::User],
        "the transform saw the system prompt"
    );
}

// Verifies: gh #46 - a filter that slices the leading system message
// off cannot drop the prompt: the host restores it (pi's
// `restoreSystemMessages`).
#[tokio::test]
async fn a_slice_cannot_drop_the_prompt() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(ProbeTransform {
        name: "slice".to_string(),
        mode: ProbeMode::SliceSystem,
        seen: Arc::new(Mutex::new(Vec::new())),
    }));
    let output = registry.transform(system_user()).await.expect("slice");
    assert_eq!(output[0].role, MessageRole::System);
    assert_eq!(first_text(&output[0]), "be brief");
}

// Verifies: gh #46 - the restore happens per handler, so a later
// transform still sees the prompt after an earlier one sliced it.
#[tokio::test]
async fn a_second_transform_still_sees_the_prompt() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(ProbeTransform {
        name: "slice".to_string(),
        mode: ProbeMode::SliceSystem,
        seen: Arc::new(Mutex::new(Vec::new())),
    }));
    registry.register(Arc::new(ProbeTransform {
        name: "observe".to_string(),
        mode: ProbeMode::Observe,
        seen: seen.clone(),
    }));
    registry.transform(system_user()).await.expect("chain");
    assert_eq!(
        *seen.lock().expect("seen"),
        vec![MessageRole::System, MessageRole::User],
        "the second transform saw the restored prompt"
    );
}

// Verifies: gh #46 - end to end, the provider request keeps its leading
// system prompt even with a slicing transform installed.
#[tokio::test]
async fn a_slicing_transform_keeps_the_provider_prompt() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("done").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(ProbeTransform {
        name: "slice".to_string(),
        mode: ProbeMode::SliceSystem,
        seen: Arc::new(Mutex::new(Vec::new())),
    }));
    let mut config = default_config();
    config.extensions = Arc::new(registry);
    let mut h = harness("slice-keeps-prompt", provider, config);
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    let outcome = turn(&mut h, "go", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let requests = h.provider.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].messages[0].role,
        MessageRole::System,
        "the provider request keeps its leading system prompt"
    );
}

/// A settle double that never stops asking: every `turn_end` appends
/// and continues. The turn must still settle after exactly one
/// continuation (gh #45's `settle_continued` flag, pinned by #46).
struct SettleForever;

impl ExtensionDispatch for SettleForever {
    fn name(&self) -> &str {
        "settle-forever"
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::HooksSettle]
    }
    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.name().to_string(),
            world: "tool",
        })))
    }
    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
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
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(HookAction::Allow)))
    }
    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a PostToolObservation,
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
    fn on_turn_end<'a>(
        &'a self,
        _rounds: u32,
        _tool_calls: u32,
        _status: &'a str,
    ) -> DispatchFuture<'a, Result<SettleDecision, DispatchError>> {
        Box::pin(std::future::ready(Ok(SettleDecision {
            append: Some("more".to_string()),
            continue_once: true,
        })))
    }
}

// Verifies: gh #46 - a handler cannot loop the turn: even a settle
// decision that always continues runs exactly one more request, then
// the turn settles.
#[tokio::test]
async fn a_settle_handler_that_never_stops_still_settles_once() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("first").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("second").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(SettleForever));
    let mut config = default_config();
    config.extensions = Arc::new(registry);
    let mut h = harness("settle-forever", provider, config);
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    let outcome = turn(&mut h, "go", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(h.provider.call_count(), 2, "exactly one continuation");
    let log = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    assert_eq!(
        log.records
            .iter()
            .filter(|record| matches!(
                record,
                Record::CustomMessage { content, .. } if content.contains("more")
            ))
            .count(),
        1,
        "one append, then settle"
    );
}
