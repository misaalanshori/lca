//! Shared doubles for the gh #275 / #277 regression files.
//!
//! `ExtensionDispatch` has no defaults for the hook and tool methods, so a
//! double that only wants `compact` or `transform_messages` still has to
//! declare the rest. Keeping one implementation here means the two regression
//! files stay about their defect instead of about trait plumbing.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test support: a panic here is a failed assertion.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::{
    ChatMessage, CommandEffect, CommandSpec, DispatchError, PostToolObservation, Record, ToolCall,
    ToolResult, ToolSpec,
};

/// What the double does when asked to transform.
#[derive(Clone, Copy)]
pub enum TransformKind {
    /// Append a marker message after everything (the skills-injection shape).
    AppendTail(&'static str),
    /// Rewrite the system prompt once, on the second call.
    RewriteSystemOnSecondCall,
}

/// A double covering the compaction and context-transform worlds.
pub struct Double {
    label: &'static str,
    worlds: Vec<World>,
    pub transform_calls: Arc<AtomicUsize>,
    pub compact_calls: Arc<AtomicUsize>,
    kind: Option<TransformKind>,
}

impl Double {
    /// A compaction strategy that always summarizes.
    pub fn summarizer() -> Double {
        Double {
            label: "test-summary",
            worlds: vec![World::Compaction],
            transform_calls: Arc::new(AtomicUsize::new(0)),
            compact_calls: Arc::new(AtomicUsize::new(0)),
            kind: None,
        }
    }

    /// A context transform that behaves per `kind`.
    pub fn transformer(label: &'static str, kind: TransformKind) -> Double {
        Double {
            label,
            worlds: vec![World::ContextTransform],
            transform_calls: Arc::new(AtomicUsize::new(0)),
            compact_calls: Arc::new(AtomicUsize::new(0)),
            kind: Some(kind),
        }
    }
}

impl ExtensionDispatch for Double {
    fn name(&self) -> &str {
        self.label
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<World> {
        self.worlds.clone()
    }

    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>> {
        let label = self.label.to_string();
        Box::pin(async move {
            Err(DispatchError::MissingWorld {
                extension: label,
                world: "tool",
            })
        })
    }

    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Ok(CommandEffect::None)
    }

    fn compact(
        &self,
        _records: &[Record],
    ) -> DispatchFuture<'static, Result<String, DispatchError>> {
        self.compact_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok("SUMMARY".to_string()) })
    }

    fn transform_messages(
        &self,
        mut messages: Vec<ChatMessage>,
    ) -> DispatchFuture<'static, Result<Result<Vec<ChatMessage>, String>, DispatchError>> {
        let call = self.transform_calls.fetch_add(1, Ordering::SeqCst);
        match self.kind {
            Some(TransformKind::AppendTail(marker)) => {
                messages.push(ChatMessage::text(lca_protocol::MessageRole::User, marker));
            }
            Some(TransformKind::RewriteSystemOnSecondCall) => {
                if call == 1
                    && let Some(first) = messages.first_mut()
                {
                    first.content = vec![lca_protocol::ContentBlock::Text {
                        text: "REWRITTEN".to_string(),
                    }];
                }
            }
            None => {}
        }
        Box::pin(async move { Ok(Ok(messages)) })
    }

    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(async { Ok(()) })
    }

    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
        Box::pin(async { Ok(lca_protocol::HookAction::Allow) })
    }

    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a PostToolObservation,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async { Ok(()) })
    }

    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async { Ok(()) })
    }

    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async { Ok(()) })
    }

    fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(async { Ok(()) })
    }
}
