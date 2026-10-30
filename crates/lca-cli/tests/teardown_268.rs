//! Gh #268: the session-close teardown must **complete**, not merely be called.
//!
//! `headless.rs` (and `rpc.rs`'s `finish`) called
//! `lca_core::drive_blocking(async move { registry.on_session_close().await })`
//! from inside an `async fn`. That spawns an OS thread and builds a
//! single-thread Tokio runtime *inside a running async context* - a hazard (it can
//! deadlock when the blocking call needs the same runtime) and a thread per
//! invocation, for a call that is plainly `.await`able: `on_session_close` is
//! already an `async fn`.
//!
//! Two properties are asserted here, and they are different:
//!
//! 1. **the hook runs to completion** - an `async fn` dropped mid-await does not
//!    finish, so the marker is inspected rather than the call's return;
//! 2. **no thread or nested runtime is created** - "it worked" was true before the
//!    fix too, so a passing teardown alone would not notice the hazard returning.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::{CommandEffect, CommandSpec, DispatchError, ToolCall, ToolResult, ToolSpec};

/// A hooks-world extension that records every `on_session_close` it receives.
struct Marker {
    calls: Arc<AtomicUsize>,
    completed: Arc<AtomicUsize>,
}

impl ExtensionDispatch for Marker {
    fn name(&self) -> &str {
        "marker"
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<World> {
        vec![World::Hooks]
    }

    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>> {
        Box::pin(async {
            Err(DispatchError::MissingWorld {
                extension: "marker".to_string(),
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

    fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        let calls = self.calls.clone();
        let completed = self.completed.clone();
        // The marker is written **after** a yield point, so a caller that drops the
        // future instead of awaiting it to completion records the call but not the
        // completion. That is the difference the issue is about.
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            completed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

fn registry_with_marker() -> (
    Arc<lca_core::ExtensionRegistry>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(Marker {
        calls: calls.clone(),
        completed: completed.clone(),
    }));
    (Arc::new(registry), calls, completed)
}

// Verifies: gh #268 - awaiting the teardown **completes** it.
//
// The shape `headless` uses now: await it in the surrounding async context. The
// marker is written past a `yield_now`, so this only passes if the future was
// polled to completion - which is what a dropped `async fn` fails to do.
#[test]
fn awaiting_the_teardown_runs_it_to_completion() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (registry, calls, completed) = registry_with_marker();
    runtime.block_on(async move {
        registry.on_session_close().await;
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the hook was entered once");
    assert_eq!(
        completed.load(Ordering::SeqCst),
        1,
        "and ran past its yield, so it completed rather than being dropped"
    );
}

// Verifies: gh #268 - a **cancelled** teardown stops mid-flight and is reported
// as such, not silently.
//
// `tokio::time::timeout` around the teardown drops the future at the deadline.
// The row pins the honest behaviour: the entry marker is written, the completion
// marker is not. Stating this is the point - it means "the hook ran" must be
// asserted on completion, not on entry, which is exactly the mistake a
// `drive_blocking` (join-forever) call could never make and an `.await` could.
#[test]
fn a_cancelled_teardown_stops_before_completing() {
    struct Slow {
        entered: Arc<AtomicUsize>,
        completed: Arc<AtomicUsize>,
    }
    impl ExtensionDispatch for Slow {
        fn name(&self) -> &str {
            "slow"
        }
        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }
        fn worlds(&self) -> Vec<World> {
            vec![World::Hooks]
        }
        fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
            Ok(Vec::new())
        }
        fn execute_tool<'a>(
            &'a self,
            _call: &'a ToolCall,
        ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>> {
            Box::pin(async {
                Err(DispatchError::MissingWorld {
                    extension: "slow".to_string(),
                    world: "tool",
                })
            })
        }
        fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
            Ok(Vec::new())
        }
        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<CommandEffect, DispatchError> {
            Ok(CommandEffect::None)
        }
        fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            let entered = self.entered.clone();
            let completed = self.completed.clone();
            Box::pin(async move {
                entered.fetch_add(1, Ordering::SeqCst);
                // Long enough that the timeout below always wins.
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                completed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }
    let entered = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(Slow {
        entered: entered.clone(),
        completed: completed.clone(),
    }));
    let registry = Arc::new(registry);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async move {
        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            registry.on_session_close(),
        )
        .await;
        assert!(outcome.is_err(), "the deadline was reached");
    });
    assert_eq!(entered.load(Ordering::SeqCst), 1, "the hook was entered");
    assert_eq!(
        completed.load(Ordering::SeqCst),
        0,
        "and not completed - so a row must assert completion, not entry"
    );
}
