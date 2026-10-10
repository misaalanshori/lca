//! #102 (QA-016): one background identity/discovery step for the
//! login/identity commands. Split from `login.rs` for the workspace file
//! ceiling (gate 11). Behaviour unchanged.

use std::sync::Arc;

use lca_ui::LoginNext;

/// #102 (QA-016): one background identity/discovery step. The interface
/// thread never joins blocking work: it issues a generation, hands the
/// work off, and keeps painting. The result lands in the poll slot for
/// `poll_login` iff no newer step or cancel intervened - the same shape
/// `/compact` and the namespaced `.login` path already use, factored so
/// the slow `/login` discovery and the generic identity commands share
/// it instead of each joining `drive_blocking` on the interface thread.
///
/// The slot and counter are caller-owned Arcs (the interface passes its
/// own `login_pending` generation pair), so this stays testable without
/// a whole `Ui`.
pub(super) struct PendingStep {
    slot: Arc<std::sync::Mutex<Option<LoginNext>>>,
    generation: Arc<std::sync::atomic::AtomicU64>,
}

impl PendingStep {
    /// Wrap a poll slot with its generation counter.
    pub(super) fn wrap(
        slot: Arc<std::sync::Mutex<Option<LoginNext>>>,
        generation: Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        PendingStep { slot, generation }
    }

    /// Claim the next generation for a step about to start.
    pub(super) fn issue(&self) -> u64 {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    /// Run `work` for a generation ticket, delivering its answer unless a
    /// newer step or a cancel superseded it meanwhile.
    pub(super) fn run(&self, ticket: u64, work: impl FnOnce() -> LoginNext + Send + 'static) {
        let step = PendingStep {
            slot: self.slot.clone(),
            generation: self.generation.clone(),
        };
        // Its own thread: the caller never joins it (QA-016 - a slow
        // provider call must not freeze the interface thread).
        std::thread::spawn(move || {
            let next = work();
            step.deliver(ticket, next);
        });
    }

    /// Deliver a finished step unless it is stale.
    pub(super) fn deliver(&self, ticket: u64, next: LoginNext) {
        if self.generation.load(std::sync::atomic::Ordering::SeqCst) != ticket {
            return;
        }
        *self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(next);
    }

    /// Cancel: any in-flight step's delivery is dropped on arrival, so
    /// Escape during a slow discovery reports the cancel, never a
    /// stale picker.
    pub(super) fn cancel(&self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Take a landed step (tests use this; the interface polls the slot
    /// itself through `poll_login`).
    #[cfg(test)]
    pub(super) fn take(&self) -> Option<LoginNext> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use lca_ext_abi::ExtensionDispatch as _;

    use super::PendingStep;

    /// #102's slow fake provider: `login_options` answers after `delay`,
    /// like a model discovery stalled on the network.
    struct SlowFake {
        delay: Duration,
    }

    impl lca_ext_abi::ExtensionDispatch for SlowFake {
        fn name(&self) -> &str {
            "slow-fake"
        }

        fn delivery(&self) -> lca_ext_abi::DeliveryMode {
            lca_ext_abi::DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<lca_ext_abi::World> {
            vec![lca_ext_abi::World::Provider]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, lca_protocol::DispatchError> {
            Ok(Vec::new())
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> lca_ext_abi::DispatchFuture<
            'a,
            Result<lca_protocol::ToolResult, lca_protocol::DispatchError>,
        > {
            Box::pin(std::future::ready(Err(
                lca_protocol::DispatchError::Failed("no tools".to_string()),
            )))
        }

        fn command_specs(
            &self,
        ) -> Result<Vec<lca_protocol::CommandSpec>, lca_protocol::DispatchError> {
            Ok(Vec::new())
        }

        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, lca_protocol::DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }

        fn on_pre_turn(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> lca_ext_abi::DispatchFuture<
            'a,
            Result<lca_protocol::HookAction, lca_protocol::DispatchError>,
        > {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }

        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_session_close(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn login_options(
            &self,
        ) -> lca_ext_abi::DispatchFuture<
            'static,
            Result<Vec<lca_protocol::LoginOption>, lca_protocol::DispatchError>,
        > {
            let delay = self.delay;
            Box::pin(async move {
                std::thread::sleep(delay);
                Ok(vec![lca_protocol::LoginOption {
                    id: "key".to_string(),
                    name: "Slow".to_string(),
                    kind: "api-key".to_string(),
                    host: "slow.example.com".to_string(),
                    fields: vec!["api-key".to_string()],
                    extras: Default::default(),
                }])
            })
        }
    }

    fn step() -> PendingStep {
        PendingStep::wrap(Arc::new(Mutex::new(None)), Arc::new(AtomicU64::new(0)))
    }

    /// The discovery work the `/login` step runs: gather the fake's
    /// options off-thread (a `drive_blocking` bridge like [`Ui::gather`]
    /// uses) and open the picker over them.
    fn discovery(fake: Arc<SlowFake>) -> impl FnOnce() -> lca_ui::LoginNext + Send + 'static {
        move || {
            let options = lca_core::drive_blocking(async move { fake.login_options().await })
                .unwrap_or_default();
            if options.is_empty() {
                return lca_ui::LoginNext::Message("nothing to sign in to".to_string());
            }
            lca_ui::LoginNext::Picker {
                options: options
                    .into_iter()
                    .map(|option| lca_ui::PickerOption {
                        provider: "slow-fake".to_string(),
                        id: option.id,
                        label: option.name,
                        hint: option.host,
                    })
                    .collect(),
            }
        }
    }

    // Verifies: #102 (QA-016) - a slow discovery never blocks the
    // caller: `run` returns within 500 ms, and a cancel in between
    // drops the late arrival instead of popping a stale picker.
    #[test]
    fn a_slow_login_does_not_freeze_input() {
        let step = step();
        let fake = Arc::new(SlowFake {
            delay: Duration::from_millis(1500),
        });
        let ticket = step.issue();
        let start = Instant::now();
        step.run(ticket, discovery(fake));
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "the interface thread must not join a 1.5 s discovery: took {:?}",
            start.elapsed()
        );
        step.cancel();
        std::thread::sleep(Duration::from_millis(2000));
        assert!(step.take().is_none(), "a cancelled step delivers nothing");
    }

    // Verifies: #102 (QA-016) - the finished discovery opens the picker
    // carrying the discovered options (progress shown, then the rows).
    #[test]
    fn a_finished_discovery_opens_the_picker() {
        let step = step();
        let fake = Arc::new(SlowFake {
            delay: Duration::from_millis(100),
        });
        let ticket = step.issue();
        step.run(ticket, discovery(fake));
        let deadline = Instant::now() + Duration::from_secs(5);
        let landed = loop {
            if let Some(next) = step.take() {
                break next;
            }
            assert!(Instant::now() < deadline, "the picker never landed");
            std::thread::sleep(Duration::from_millis(10));
        };
        match landed {
            lca_ui::LoginNext::Picker { options } => {
                assert_eq!(options.len(), 1, "one discovered option");
                assert_eq!(options[0].id, "key");
                assert_eq!(options[0].provider, "slow-fake");
            }
            other => panic!("discovery must open the picker, got {other:?}"),
        }
    }

    // Verifies: #102 - a cancel drops even an already-finished step:
    // Escape during the wait reports the cancel, never the stale row.
    #[test]
    fn cancel_drops_a_landed_step() {
        let step = step();
        let ticket = step.issue();
        step.cancel();
        step.deliver(ticket, lca_ui::LoginNext::Message("stale".to_string()));
        assert!(step.take().is_none(), "stale delivery is dropped");
    }

    // Verifies: #102 - generations order overlapping steps: only the
    // latest lands, and a superseded delivery never overwrites it.
    #[test]
    fn only_the_latest_generation_lands() {
        let step = step();
        let first = step.issue();
        let second = step.issue();
        step.deliver(first, lca_ui::LoginNext::Message("first".to_string()));
        assert!(step.take().is_none(), "the older step is dropped");
        step.deliver(second, lca_ui::LoginNext::Message("second".to_string()));
        match step.take() {
            Some(lca_ui::LoginNext::Message(text)) => assert_eq!(text, "second"),
            other => panic!("the latest step must land, got {other:?}"),
        }
        step.deliver(first, lca_ui::LoginNext::Message("first again".to_string()));
        assert!(step.take().is_none(), "a replay never overwrites");
    }
}
