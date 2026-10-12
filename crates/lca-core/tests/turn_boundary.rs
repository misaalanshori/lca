//! The turn-boundary latch guard (gh #97): a native extension that
//! implements only `interrupt` cannot leave the session pre-cancelled.
//! The host clears every noted engine at the boundary, so the next
//! turn runs clean.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_core::{Agent, AgentConfig, ExtensionRegistry, TurnEvent, TurnOutcome, TurnSink, TurnStatus};
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::{
    CommandEffect, CommandSpec, CompletionRequest, DispatchError, EventSink, HookAction,
    PostToolObservation, StreamEvent, ToolCall, ToolResult, ToolSpec, Usage,
};

fn scratch(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(name);
    for part in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    dir
}

struct Allow;
impl PermissionPrompt for Allow {
    fn ask(&mut self, _: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _: &ProposalDiff) -> bool {
        false
    }
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

/// The footgun shape (gh #97): a native extension whose only dispatch
/// behavior is `interrupt`. `turn_started` keeps its no-op default, so
/// only a host-side reset can keep the next turn clean.
struct InterruptOnly {
    cap: Arc<lca_tools::Capabilities>,
}

impl ExtensionDispatch for InterruptOnly {
    fn name(&self) -> &str {
        "interrupt-only"
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::Provider]
    }
    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: "interrupt-only".to_string(),
            world: "tool",
        })
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: "interrupt-only".to_string(),
            world: "tool",
        })))
    }
    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: "interrupt-only".to_string(),
            world: "command",
        })
    }
    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: "interrupt-only".to_string(),
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
    fn interrupt(&self) {
        // The half every cancellable native implements: flag the engine.
        self.cap.cancel();
    }
    // No `turn_started`: the latch the host must clear itself.
    fn stream_completion<'a>(
        &'a self,
        _request: CompletionRequest,
        sink: &'a dyn EventSink,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        // The provider shape: a latched engine fails the call the way a
        // real provider's blocked `net` wait does ("request cancelled by
        // the user"), so a stuck flag fails the turn instead of hiding.
        if self.cap.is_cancelled() {
            return Box::pin(std::future::ready(Err(DispatchError::Failed(
                "interrupt-only: request cancelled by the user".to_string(),
            ))));
        }
        sink.push(StreamEvent::TextDelta {
            delta: "clean".to_string(),
        });
        sink.push(StreamEvent::Usage {
            usage: Usage::default(),
        });
        Box::pin(std::future::ready(Ok(())))
    }
}

// Verifies: gh #97 (an interrupt-only native cannot latch the session).
// Turn one cancels mid-flight, which fires `interrupt` and latches the
// engine; turn two must still run clean because the host resets the
// engine at the boundary, not the extension.
#[tokio::test]
async fn an_interrupt_only_extension_cannot_latch_cancellation() {
    let root = scratch("latch-guard");
    let project = root.join("project");
    let cap = Arc::new(lca_tools::Capabilities::new(
        "interrupt-only",
        lca_tools::CapabilityGrants::default(),
        ScopeRoots {
            workspace: project.clone(),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        project.clone(),
        None,
    ));
    let double: Arc<InterruptOnly> = Arc::new(InterruptOnly { cap: cap.clone() });
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::clone(&double) as Arc<dyn ExtensionDispatch>);
    // The host knows this handle's engine (gh #97's host-side reset):
    // noting it is registration bookkeeping, not implementor behavior.
    // RED: the noting API lands with the fix; until then the latch
    // below is unopposed.
    // registry.note_engine("interrupt-only", &cap);

    let store = lca_session::SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let mut tools = lca_tools::ToolExecutor::new(
        Arc::new(lca_tools::NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Some(std::time::Duration::from_secs(30)),
    );
    let config = AgentConfig {
        provider: "interrupt-only".to_string(),
        model: "faux-1".to_string(),
        retry_base_delay: std::time::Duration::ZERO,
        extensions: Arc::new(registry),
        ..AgentConfig::default()
    };
    let backend = lca_core::ExtensionProvider::new(double.clone() as Arc<dyn ExtensionDispatch>);
    let mut prompt = Allow;
    let mut agent = Agent::new(
        &store,
        &session,
        &backend,
        &mut tools,
        grants,
        &mut prompt,
        None,
        config,
    );

    // Turn one: pre-cancelled, so the watcher's `interrupt` fires and
    // latches the engine before any work runs.
    let cancelled = lca_tools::CancelFlag::new();
    cancelled.cancel();
    let mut sink = CollectingSink::default();
    let first: TurnOutcome = agent.run_turn("spin", &mut sink, &cancelled).await;
    assert!(
        double.cap.is_cancelled(),
        "turn one's interrupt latched the engine (the guard's premise)"
    );
    let _ = first;

    // Turn two with a fresh flag: the boundary reset the engine, so the
    // provider answers instead of failing pre-cancelled.
    let fresh = lca_tools::CancelFlag::new();
    let mut sink2 = CollectingSink::default();
    let second: TurnOutcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        agent.run_turn("again", &mut sink2, &fresh),
    )
    .await
    .expect("the next turn returns");
    assert_eq!(second.status, TurnStatus::Ok, "the next turn runs clean");
}
