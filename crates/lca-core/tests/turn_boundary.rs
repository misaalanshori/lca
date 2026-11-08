//! The turn-boundary latch guard (gh #97): a native extension that
//! implements only `interrupt` cannot leave the session pre-cancelled.
//! The host clears every noted engine at the boundary, so the next
//! turn runs clean.
//!
//! Gh #547's row lives here too, because it needs the same double: an
//! interrupt-only native is exactly the handle whose `interrupt` must run at
//! the instant of the cancel rather than on a later poll tick.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_core::{
    Agent, AgentConfig, ExtensionRegistry, TurnEvent, TurnOutcome, TurnSink, TurnStatus,
};
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::{
    CommandEffect, CommandSpec, CompletionRequest, DispatchError, EventSink, HookAction,
    PostToolObservation, StopReason, StreamEvent, ToolCall, ToolResult, ToolSpec, Usage,
};
use lca_testkit::{FakeProvider, fake_usage};

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

/// A sink that also flags the first text delta: the row below needs to know the
/// turn is genuinely *in* its stream before it cancels, because the cancellation
/// resets the extensions' engines at the turn boundary (gh #97's guard) and a
/// cancel that lands before that reset is deliberately erased.
struct StartedSink {
    inner: CollectingSink,
    started: Arc<std::sync::atomic::AtomicBool>,
}

impl TurnSink for StartedSink {
    fn on_event(&mut self, event: TurnEvent) {
        if matches!(event, TurnEvent::TextDelta(_)) {
            self.started
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.inner.on_event(event);
    }
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
    registry.note_engine("interrupt-only", &cap);

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

    // Turn one: pre-cancelled. The registration fires immediately (gh #547),
    // which latches the engine before any work runs - the premise this row
    // needs, and the reason the already-cancelled case has to work.
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

// Verifies: gh #547 - the turn's cancel reaches the extension *synchronously*:
// by the time `cancel()` returns, `interrupt()` has already run.
//
// The previous shape was a dedicated OS thread polling `is_cancelled()` every
// millisecond and calling `interrupt()` on one of its ticks, so the interrupt
// landed *after* `cancel()` returned - by up to a millisecond on Unix and up to
// a ~15.6 ms Windows timer quantum. The assertion is a coincidence, not a
// duration: the canceller reads the engine's flag on the line after
// `cancel()` and the flag must already be set. A poller cannot satisfy it on
// any platform, and no wall-clock threshold is involved.
#[tokio::test]
async fn a_cancel_interrupts_the_extension_before_cancel_returns() {
    let root = scratch("cancel-sync");
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
    registry.note_engine("interrupt-only", &cap);

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
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_base_delay: std::time::Duration::ZERO,
        extensions: Arc::new(registry),
        ..AgentConfig::default()
    };
    // A mid-stream pause: the cancel has to land *while* the turn is running,
    // which is the case a poller would have needed to exist for.
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("starting")
                .pause(400)
                .text("done")
                .usage(fake_usage(10, 5, 0, 0))
        })
        .build();

    let cancel = lca_tools::CancelFlag::new();
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let interrupted_by_cancel_return = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let canceller = {
        let flag = cancel.clone();
        let cap = cap.clone();
        let observed = interrupted_by_cancel_return.clone();
        let started = started.clone();
        std::thread::spawn(move || {
            // Wait for the turn's first delta, not for a duration: the boundary
            // reset clears any earlier interrupt by design, so the cancel has to
            // land after the turn is running for this row to mean anything. The
            // bound is a hang guard only.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !started.load(std::sync::atomic::Ordering::SeqCst) {
                if std::time::Instant::now() >= deadline {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            flag.cancel();
            // The line immediately after `cancel()`: this is the whole row.
            observed.store(cap.is_cancelled(), std::sync::atomic::Ordering::SeqCst);
        })
    };
    let mut sink = StartedSink {
        inner: CollectingSink::default(),
        started: started.clone(),
    };
    let mut prompt = Allow;
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
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        agent.run_turn("spin", &mut sink, &cancel),
    )
    .await
    .expect("the turn returns");
    canceller.join().expect("canceller");

    assert!(
        interrupted_by_cancel_return.load(std::sync::atomic::Ordering::SeqCst),
        "`interrupt()` had already run when `cancel()` returned (gh #547); \
         a 1 ms poll thread could only reach it on a later tick"
    );
    assert_eq!(
        outcome.stop_reason,
        StopReason::Cancelled,
        "and the turn still reports the cancellation"
    );
}

// Verifies: gh #547 - a turn that ended normally left nothing registered.
//
// The guard is turn-scoped: after `run_turn` returns, cancelling the same flag
// (twice, since several paths may) must not interrupt an extension whose turn
// is already over. The old watcher got this for free - it had been joined by
// then - and the callback shape gets it from the guard's `Drop`, so the row
// pins a property the new mechanism has to keep rather than inherit.
#[tokio::test]
async fn a_cancel_after_a_finished_turn_does_not_reach_the_extension() {
    let root = scratch("cancel-after");
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
    registry.note_engine("interrupt-only", &cap);

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
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_base_delay: std::time::Duration::ZERO,
        extensions: Arc::new(registry),
        ..AgentConfig::default()
    };
    let provider = FakeProvider::builder()
        .turn(|t| t.text("clean").usage(fake_usage(10, 5, 0, 0)))
        .build();

    let cancel = lca_tools::CancelFlag::new();
    let mut sink = CollectingSink::default();
    let mut prompt = Allow;
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
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        agent.run_turn("hello", &mut sink, &cancel),
    )
    .await
    .expect("the turn returns");
    assert_eq!(outcome.status, TurnStatus::Ok, "the turn ran clean");
    assert!(
        !cap.is_cancelled(),
        "an uncancelled turn never interrupted anything"
    );

    // Now cancel the turn's own flag, twice: the guard dropped with the turn,
    // so nothing of that turn is still listening.
    cancel.cancel();
    cancel.cancel();
    assert!(
        !cap.is_cancelled(),
        "a finished turn's registration is gone (gh #547); the cancel \
         reached no extension of that turn"
    );
    assert!(
        cancel.is_cancelled(),
        "the flag itself still records the cancel, so anything polling it sees it"
    );
}
