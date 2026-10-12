//! Manager support (gh #3): the compaction preference and the row
//! listing. Split from `registry.rs` for the line ceiling; the table
//! stays the single source, these are its manager-facing methods.

use super::ExtensionRegistry;
use std::sync::Arc;

use lca_ext_abi::{ExtensionDispatch, World};

impl ExtensionRegistry {
    /// The first enabled `compaction` strategy (FR-SESS-5): compaction
    /// only ever happens through this world, and only one extension
    /// owns a given turn's compact. Gh #3: a preferred name wins when
    /// it names an enabled compaction handle; anything else (unknown
    /// or disabled) falls back to registration order.
    pub fn compaction_strategy(&self) -> Option<&Arc<dyn ExtensionDispatch>> {
        let preferred = self
            .preferred_compaction
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(preferred) = preferred
            && let Some(handle) = self.entries.iter().find(|entry| {
                entry.is_enabled()
                    && entry.handle.name() == preferred
                    && entry.handle.worlds().contains(&World::Compaction)
            })
        {
            return Some(&handle.handle);
        }
        self.entries
            .iter()
            .find(|entry| entry.is_enabled() && entry.handle.worlds().contains(&World::Compaction))
            .map(|entry| &entry.handle)
    }
    /// Prefer one compaction strategy by name (gh #3, the manager's
    /// picker): `None` is automatic. Unknown names are kept and fall
    /// back at selection time, so a typo degrades instead of hiding.
    pub fn set_preferred_compaction(&self, name: Option<String>) {
        *self
            .preferred_compaction
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = name;
    }
    /// The preferred compaction strategy name, if one is picked (gh
    /// #3, the manager's radio state).
    pub fn preferred_compaction(&self) -> Option<String> {
        self.preferred_compaction
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
    /// Every registered extension with its session state (gh #3, the
    /// manager's rows): name, enabled, and worlds, in registration
    /// order.
    pub fn managed_entries(&self) -> Vec<(String, bool, Vec<lca_ext_abi::World>)> {
        self.entries
            .iter()
            .map(|entry| {
                (
                    entry.handle.name().to_string(),
                    entry.enabled.load(std::sync::atomic::Ordering::SeqCst),
                    entry.handle.worlds(),
                )
            })
            .collect()
    }
}
