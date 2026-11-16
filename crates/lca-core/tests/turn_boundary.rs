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

/// What the double observed, so a row can assert on the *guest's* facts rather
/// than on a same-thread read of another object (gh #604).
///
/// Gh #547's row used to read `cap.is_cancelled()` on the canceller thread one
/// line after `cancel()` returned. That is a load of an atomic **another**
/// object's listener wrote, with no happens-before edge between "cancel()
/// returned" and "the listener has run" - and hosted Windows (`43774970823`,
/// head `3f25c83e`) observed exactly that stale read.
///
/// Two observations replace it, and each is one the canceller cannot miss:
///
/// * [`Self::interrupts`] counts `interrupt()` calls, and the row reads it from
///   **its own** `on_cancel` callback - callbacks run in registration order on
///   the cancelling thread inside `cancel()`, so "the turn's listener has
///   already run when mine runs" is a real ordering fact rather than a guess;
/// * `guest_saw_cancel` records the guest's own look at its engine. It is the
///   issue's option-2 half, **measured not to fire in a turn row** (the host's
///   `biased` `select!` cuts the stream before the guest looks, so this stays
///   false - `interrupts=1 guest_saw_cancel=false error=None` in 3/3 runs). It
///   stays as the double's own record, and is what a row about a **blocked**
///   guest call would assert instead.
#[derive(Default)]
struct Probe {
    interrupts: std::sync::atomic::AtomicUsize,
    /// Set when the guest's own `stream_completion` arm saw a latched engine.
    /// Written by the double and read by the measurement that established the
    /// correction (`10/10 loaded repeats` section of gh #604's report); no row
    /// asserts it, which is the point of the correction.
    #[allow(
        dead_code,
        reason = "the double's own record; gh #604 measured that the host cuts the stream first"
    )]
    guest_saw_cancel: std::sync::atomic::AtomicBool,
    /// What the guest's own engine reported **at the instant it was
    /// interrupted**, taken inside `interrupt()` immediately after the flag was
    /// set - synchronous, so it cannot be missed and cannot be reordered.
    latched_on_interrupt: std::sync::atomic::AtomicBool,
}

impl Probe {
    fn interrupts(&self) -> usize {
        self.interrupts.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn latched_on_interrupt(&self) -> bool {
        self.latched_on_interrupt
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The footgun shape (gh #97): a native extension whose only dispatch
/// behavior is `interrupt`. `turn_started` keeps its no-op default, so
/// only a host-side reset can keep the next turn clean.
struct InterruptOnly {
    cap: Arc<lca_tools::Capabilities>,
    /// Set by the cancel-synchrony row (gh #604); the other rows ignore it, so
    /// one double serves all three rather than three near-copies drifting.
    probe: Probe,
}

impl InterruptOnly {
    fn new(cap: Arc<lca_tools::Capabilities>) -> InterruptOnly {
        InterruptOnly {
            cap,
            probe: Probe::default(),
        }
    }
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
        //
        // Counted before the flag so a reader that sees the flag set also sees
        // the count (gh #604's edge is about ordering, not about the flag).
        self.probe
            .interrupts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.cap.cancel();
        // The guest's own observation, taken on the fan-out's thread inside
        // `cancel()` - the half gh #604 adds so the row is not only about the
        // counter: "the guest saw the cancellation" is a fact about the guest.
        self.probe
            .latched_on_interrupt
            .store(self.cap.is_cancelled(), std::sync::atomic::Ordering::SeqCst);
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
            // The guest's own look at its engine (gh #604): this arm is the
            // behavioural half of what #547 bought - a host whose fan-out is
            // deferred leaves it unset when the call runs.
            self.probe
                .guest_saw_cancel
                .store(true, std::sync::atomic::Ordering::SeqCst);
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
    let double: Arc<InterruptOnly> = Arc::new(InterruptOnly::new(cap.clone()));
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

// Verifies: gh #547 (the ordering) and gh #604 (how the row proves it) - a
// cancel reaches the extension **synchronously**: by the time `cancel()`
// returned, `interrupt()` had already run on the cancelling thread.
//
// **The observation moved, and why** (gh #604). This row used to read
// `cap.is_cancelled()` on the canceller thread one line after `cancel()`
// returned. That is a load of an atomic a **listener** wrote, on a **different
// object**, with no happens-before edge between "cancel() returned" and "the
// listener has run": the listener runs on the cancelling thread by construction,
// so under a stretch there is simply no guarantee the load sees the store - and
// hosted Windows, CI `43774970823` (head `3f25c83e`), observed exactly that
// stale read, on the *assertion* rather than on the hang guard:
//
// ```text
// `interrupt()` had already run when `cancel()` returned (gh #547);
// a 1 ms poll thread could only reach it on a later tick
// ```
//
// (The message named a poll thread gh #547 had already removed, which is part of
// why a reader could not tell whether that firing was a regression.)
//
// The edge is now observed where it is actually made. The row registers **its
// own** callback on the same flag, *after* the turn's listener is in place
// (see the comment on the spawn), and callbacks run in **registration order** on
// the cancelling thread inside `cancel()` - so "the turn's listener has already
// run by the time mine runs" is an ordering fact about one thread, not a race
// between two objects. What the callback records is the double's own counters,
// so the assertion is the *guest's* effect:
//
// * the guest's `interrupt()` count (a poller leaves it 0 at that instant);
// * the guest's own engine state, read inside `interrupt()` itself - the fact a
//   deferred fan-out cannot produce, since it only runs after the row read.
//
// No wait, no yield, no sleep: the claim is ordering **at the moment of
// return**, and a wait would prove something else.
//
// **A measured correction to the issue's option 2** (this cycle, gh #604). The
// issue's "cheap behavioural half" proposed asserting that the guest's own
// `stream_completion` cancelled arm fired. It was measured not to fire in a turn
// row, and not by accident: `stream_once`'s `select!` is `biased` with
// `cancel.wait_cancelled()` as its **first** arm, so the moment the flag is set
// the host returns `CallFail::Cancelled` and the guest's dispatch future is
// dropped before it can observe anything (measured this cycle: the probe read
// `interrupts=1` and `guest_saw_cancel=false` in all 3 runs, with `TurnOutcome
// .error == None` and `stop_reason == Cancelled` - the host cut the stream, the
// guest never got to look). A guest observes a cancellation from inside a
// **blocked host call** (the `net`/`complete` waits poll the same engine), and
// the turn reports `Cancelled` with no error - so the guest's error text is
// deliberately never the turn's. Asserting "that arm fired" here would pin behaviour the tree does
// not have; what *is* asserted instead is the same fact at the same instant,
// taken inside `interrupt()` on the fan-out's own thread.
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
    let double: Arc<InterruptOnly> = Arc::new(InterruptOnly::new(cap.clone()));
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
    // What the row's **own** callback saw, on the thread that ran `cancel()`.
    let seen_interrupts = Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX));
    let seen_engine_latched = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let canceller = {
        let flag = cancel.clone();
        let double = Arc::clone(&double);
        let observed = Arc::clone(&seen_interrupts);
        let latched = Arc::clone(&seen_engine_latched);
        let started = started.clone();
        std::thread::spawn(move || {
            // Wait for the turn's first delta, not for a duration: the boundary
            // reset clears any earlier interrupt by design, so the cancel has to
            // land after the turn is running for this row to mean anything. The
            // bound is a hang guard only, and it is a 1 ms sleep - it is
            // *this thread's* wait for the turn, never the row's proof.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !started.load(std::sync::atomic::Ordering::SeqCst) {
                if std::time::Instant::now() >= deadline {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            // Register **after** the turn is running, which is what puts this
            // callback after the turn's own in registration order (the turn
            // registers its fan-out when `run_turn` starts, before any delta).
            // Registering earlier would race the turn's registration and could
            // run *first*, which would make the counter observation read 0 for a
            // healthy tree - the shape a row must not have.
            let _mine = flag.on_cancel(move || {
                observed.store(
                    double.probe.interrupts(),
                    std::sync::atomic::Ordering::SeqCst,
                );
                latched.store(
                    double.probe.latched_on_interrupt(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            });
            flag.cancel();
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

    let interrupts = seen_interrupts.load(std::sync::atomic::Ordering::SeqCst);
    assert_ne!(
        interrupts,
        usize::MAX,
        "the row's own callback never ran: `cancel()` returned without draining the \
         registrations (gh #547) - the fan-out is deferred, not synchronous"
    );
    assert!(
        interrupts >= 1,
        "the turn's listener had already run when the row's own callback ran: it saw \
         {interrupts} `interrupt()` calls. Callbacks run in registration order on the \
         cancelling thread, so a deferred fan-out (the pre-#547 poller shape) leaves \
         this at 0 - which is exactly what CI `43774970823` (hosted Windows, head \
         `3f25c83e`) could no longer guarantee when the row read another object's flag \
         one line after `cancel()`"
    );
    assert!(
        seen_engine_latched.load(std::sync::atomic::Ordering::SeqCst),
        "and the guest's own engine was already latched at the instant `interrupt()` ran, \
         observed inside `interrupt()` itself - not read afterwards from another thread"
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
    let double: Arc<InterruptOnly> = Arc::new(InterruptOnly::new(cap.clone()));
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
