//! #103 (QA-017): the per-extension instance cache that makes WASM
//! guests stateful. Split from `lib.rs` for the workspace file ceiling
//! (gate 11).
//!
//! Phase 2 instantiated a fresh guest per call (the trap-isolation
//! rule), so guest linear memory died with every call and stateful
//! extensions were impossible. The cache keeps one live guest per
//! world: checkout hands out the cached pair with its fuel refilled
//! and its epoch deadline re-armed, checkin parks it back on success.
//! Anything else evicts:
//!
//! * [`Inner::classify`] clears the whole cache on every Wasmtime
//!   failure. A trapped, fuel-exhausted, or epoch-interrupted guest is
//!   mid-execution in an unknown state; resuming it would run the next
//!   call on a poisoned guest, which is exactly what ADR-0014's fresh
//!   instances made impossible. Eviction restores that property: the
//!   next call builds fresh.
//! * [`Inner::disable`] clears it too, so a disabled extension never
//!   serves from a stale guest.
//!
//! What survives, by construction (ADR-0014's trap isolation):
//!
//! * Per-guest epoch accounting: each cached guest carries its own
//!   deadline, re-armed against the engine's current epoch at every
//!   checkout. A global bump only traps a guest that is executing
//!   right then; an idle cached guest is unaffected, and its next
//!   checkout re-arms past the bump.
//! * Cross-guest independence: every extension owns its stores, so a
//!   trap or eviction in one never touches another's.
//! * Concurrency: checkout takes the slot, so two calls racing on one
//!   handle each get a live guest (the loser builds fresh); checkin
//!   keeps the first parked guest and drops the spare. Forgetting a
//!   checkin only loses caching, never soundness.
//!
//! Host-internal only: no WIT, manifest-schema, or `ABI_VERSION` change
//! (the 0.6 freeze), and FR-EXT-3/4/5 plus FR-CONC-1 read unchanged -
//! only where the guest lives between calls moved.

use super::*;

/// One live guest per world: the store owning its linear memory plus
/// the instance bound to that store. `None` means uncached (never
/// checked out, or evicted); every world starts there.
#[derive(Default)]
pub(super) struct InstanceCache {
    tool: Option<(Store<HostState>, lca_ext_abi::host::tool::Tool)>,
    tool_catalog:
        Option<(Store<HostState>, lca_ext_abi::host::tool_catalog::ToolCatalog)>,
    command: Option<(Store<HostState>, lca_ext_abi::host::command::Command)>,
    hooks: Option<(Store<HostState>, lca_ext_abi::host::hooks::Hooks)>,
    provider: Option<(Store<HostState>, lca_ext_abi::host::provider::Provider)>,
    compaction:
        Option<(Store<HostState>, lca_ext_abi::host::compaction::Compaction)>,
    context_transform:
        Option<(Store<HostState>, lca_ext_abi::host::context_transform::ContextTransform)>,
    ui: Option<(Store<HostState>, lca_ext_abi::host::ui::Ui)>,
    hooks_message:
        Option<(Store<HostState>, lca_ext_abi::host::hooks_message::HooksMessage)>,
    hooks_tool_call:
        Option<(Store<HostState>, lca_ext_abi::host::hooks_tool_call::HooksToolCall)>,
    hooks_tool_result:
        Option<(Store<HostState>, lca_ext_abi::host::hooks_tool_result::HooksToolResult)>,
    hooks_stream:
        Option<(Store<HostState>, lca_ext_abi::host::hooks_stream::HooksStream)>,
    hooks_settle:
        Option<(Store<HostState>, lca_ext_abi::host::hooks_settle::HooksSettle)>,
    hooks_compaction:
        Option<(Store<HostState>, lca_ext_abi::host::hooks_compaction::HooksCompaction)>,
    hooks_cache:
        Option<(Store<HostState>, lca_ext_abi::host::hooks_cache::HooksCache)>,
    hooks_trust:
        Option<(Store<HostState>, lca_ext_abi::host::hooks_trust::HooksTrust)>,
}

impl InstanceCache {
    /// Drop every cached guest (eviction).
    pub(super) fn clear(&mut self) {
        *self = InstanceCache::default();
    }
}

/// One checkout/checkin pair per world, identical by construction: a
/// cached guest is re-armed and served, otherwise the miss path builds
/// exactly what the old per-call code built (missing-world error
/// first, then the enabled check inside `build_store`, then
/// instantiate).
macro_rules! cached_world {
    ($slot:ident, $checkout:ident, $checkin:ident, $pre:ident, $instance:ty, $missing:expr) => {
        pub(super) fn $checkout(&self) -> Result<(Store<HostState>, $instance), CallError> {
            if let Some((mut store, instance)) = lock(&self.cache).$slot.take() {
                if !self.enabled.load(Ordering::SeqCst) {
                    return Err(CallError::Disabled);
                }
                self.arm(&mut store)?;
                return Ok((store, instance));
            }
            let pre = self.$pre.as_ref().ok_or_else(|| $missing)?;
            let mut store = self.build_store()?;
            let instance = pre
                .instantiate(&mut store)
                .map_err(|err| self.classify(err))?;
            Ok((store, instance))
        }

        pub(super) fn $checkin(&self, store: Store<HostState>, instance: $instance) {
            lock(&self.cache).$slot.replace((store, instance));
        }
    };
}

impl Inner {
    cached_world!(
        tool,
        checkout_tool,
        checkin_tool,
        tool,
        lca_ext_abi::host::tool::Tool,
        CallError::InvalidArguments("no tool world".into())
    );
    cached_world!(
        tool_catalog,
        checkout_tool_catalog,
        checkin_tool_catalog,
        tool_catalog,
        lca_ext_abi::host::tool_catalog::ToolCatalog,
        CallError::InvalidArguments("no tool-catalog world".into())
    );
    cached_world!(
        command,
        checkout_command,
        checkin_command,
        command,
        lca_ext_abi::host::command::Command,
        CallError::InvalidArguments("no command world".into())
    );
    cached_world!(
        hooks,
        checkout_hooks,
        checkin_hooks,
        hooks,
        lca_ext_abi::host::hooks::Hooks,
        CallError::InvalidArguments("no hooks world".into())
    );
    cached_world!(
        provider,
        checkout_provider,
        checkin_provider,
        provider,
        lca_ext_abi::host::provider::Provider,
        CallError::InvalidArguments("no provider world".into())
    );
    cached_world!(
        compaction,
        checkout_compaction,
        checkin_compaction,
        compaction,
        lca_ext_abi::host::compaction::Compaction,
        CallError::InvalidArguments("no compaction world".into())
    );
    cached_world!(
        context_transform,
        checkout_transform,
        checkin_transform,
        context_transform,
        lca_ext_abi::host::context_transform::ContextTransform,
        CallError::InvalidArguments("no context-transform world".into())
    );
    cached_world!(
        ui,
        checkout_ui,
        checkin_ui,
        ui,
        lca_ext_abi::host::ui::Ui,
        CallError::InvalidArguments("no ui world".into())
    );
    cached_world!(
        hooks_message,
        checkout_hooks_message,
        checkin_hooks_message,
        hooks_message,
        lca_ext_abi::host::hooks_message::HooksMessage,
        CallError::InvalidArguments("no hooks-message world".into())
    );
    cached_world!(
        hooks_tool_call,
        checkout_hooks_tool_call,
        checkin_hooks_tool_call,
        hooks_tool_call,
        lca_ext_abi::host::hooks_tool_call::HooksToolCall,
        CallError::InvalidArguments("no hooks-tool-call world".into())
    );
    cached_world!(
        hooks_tool_result,
        checkout_hooks_tool_result,
        checkin_hooks_tool_result,
        hooks_tool_result,
        lca_ext_abi::host::hooks_tool_result::HooksToolResult,
        CallError::InvalidArguments("no hooks-tool-result world".into())
    );
    cached_world!(
        hooks_stream,
        checkout_hooks_stream,
        checkin_hooks_stream,
        hooks_stream,
        lca_ext_abi::host::hooks_stream::HooksStream,
        CallError::InvalidArguments("no hooks-stream world".into())
    );
    cached_world!(
        hooks_settle,
        checkout_hooks_settle,
        checkin_hooks_settle,
        hooks_settle,
        lca_ext_abi::host::hooks_settle::HooksSettle,
        CallError::InvalidArguments("no hooks-settle world".into())
    );
    cached_world!(
        hooks_compaction,
        checkout_hooks_compaction,
        checkin_hooks_compaction,
        hooks_compaction,
        lca_ext_abi::host::hooks_compaction::HooksCompaction,
        CallError::InvalidArguments("no hooks-compaction world".into())
    );
    cached_world!(
        hooks_cache,
        checkout_hooks_cache,
        checkin_hooks_cache,
        hooks_cache,
        lca_ext_abi::host::hooks_cache::HooksCache,
        CallError::InvalidArguments("no hooks-cache world".into())
    );
    cached_world!(
        hooks_trust,
        checkout_hooks_trust,
        checkin_hooks_trust,
        hooks_trust,
        lca_ext_abi::host::hooks_trust::HooksTrust,
        CallError::InvalidArguments("no hooks-trust world".into())
    );
}
