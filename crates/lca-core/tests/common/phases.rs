//! The compaction/transform doubles the `loop_*` phase rows drive: the scripted
//! strategy, its registry builders, and the two observations the rows count.
//!
//! Gh #548 phase 4 moved them here when `loop_phases.rs` split by subject: the
//! two sibling targets both need them, and a sibling target cannot import
//! another target's items, so they are shared rather than copied.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
#![allow(dead_code)] // a shared helper module: each test crate uses a subset.

use lca_core::{AgentConfig, ExtensionRegistry};
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::{ChatMessage, CommandEffect, DispatchError, Record, ToolCall};
use lca_session::ViewMode;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::{Harness, default_config, scratch};

/// What the transform double does per call.
#[derive(Clone, Copy)]
pub enum TransformKind {
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
pub struct PhaseDouble {
    label: &'static str,
    worlds: Vec<World>,
    compact_calls: Arc<AtomicUsize>,
    transform_calls: Arc<AtomicUsize>,
    kind: TransformKind,
}

impl PhaseDouble {
    pub fn strategy(label: &'static str, calls: Arc<AtomicUsize>) -> PhaseDouble {
        PhaseDouble {
            label,
            worlds: vec![World::Compaction],
            compact_calls: calls,
            transform_calls: Arc::new(AtomicUsize::new(0)),
            kind: TransformKind::Pass,
        }
    }

    pub fn transform(
        label: &'static str,
        kind: TransformKind,
        calls: Arc<AtomicUsize>,
    ) -> PhaseDouble {
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

pub fn phase_config(registry: ExtensionRegistry, window: u32, threshold: f64) -> AgentConfig {
    AgentConfig {
        extensions: Arc::new(registry),
        model_context_window: window,
        compaction_threshold: threshold,
        // The threshold path in isolation: no keep-recent window, so
        // the candidate is the whole pre-turn range (gh #36 phase 1
        // derives the reserve from the threshold here, as in production
        // when no absolute reserve is configured).
        compaction_keep_recent_tokens: 0,
        ..default_config()
    }
}

pub fn registry_with(handle: PhaseDouble) -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(handle));
    registry
}

pub fn registry_two(first: PhaseDouble, second: PhaseDouble) -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(first));
    registry.register(Arc::new(second));
    registry
}

pub fn count_extension_events(h: &Harness, event: &str) -> usize {
    h.store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records
        .iter()
        .filter(|record| matches!(record, Record::ExtensionEvent { event: found, .. } if found == event))
        .count()
}

/// Roots for a throwaway Capabilities engine (pattern of the other
/// fixtures in this file).
pub fn scratch_roots(name: &str) -> lca_permissions::ScopeRoots {
    let root = scratch(name);
    lca_permissions::ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    }
}
