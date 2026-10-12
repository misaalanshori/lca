//! The `/ext` extension manager (gh #3): installed extensions with
//! states to toggle, plus the compaction-strategy picker. The Hub
//! (#227) discovers; this manages. The host supplies rows and applies
//! changes; this file owns the picker state. Synchronous throughout:
//! every hook reads local state (registry, grant store, config), so
//! nothing needs the hub's worker thread.

use std::sync::Arc;

use super::chat::Chat;
use crate::state::Action;

/// One installed extension as the host reports it.
#[derive(Debug, Clone)]
pub struct ManagedEntry {
    /// Extension name.
    pub name: String,
    /// Session-enabled (grant store + live registry agree).
    pub enabled: bool,
    /// Implements the compaction world (strategy-pickable).
    pub compaction: bool,
}

/// One manager fetch: installed rows plus the active strategy.
#[derive(Debug, Clone, Default)]
pub struct ManagedData {
    /// Installed extensions, registration order.
    pub extensions: Vec<ManagedEntry>,
    /// The preferred compaction strategy (`None` is automatic).
    pub strategy: Option<String>,
}

/// The host's synchronous manager fetch: registry rows plus the
/// preferred strategy.
pub type ListManaged = Arc<dyn Fn() -> ManagedData + Send + Sync>;

/// Toggle one extension (persist per project + apply live): returns
/// the notice to show.
pub type ToggleManaged = Arc<dyn Fn(String, bool) -> String + Send + Sync>;

/// Pick the compaction strategy (`None` is automatic): persists and
/// applies; returns the notice to show.
pub type SetManagedStrategy = Arc<dyn Fn(Option<String>) -> String + Send + Sync>;

/// One display row: the strategy section leads (Automatic first,
/// then each compaction extension), then every extension.
#[derive(Debug, Clone)]
pub enum ManagedRow {
    /// A strategy pick (`None` is automatic).
    Strategy {
        /// The strategy name, or `None` for automatic.
        name: Option<String>,
        /// Whether this pick is active.
        active: bool,
    },
    /// An installed extension with its state.
    Extension {
        /// Extension name.
        name: String,
        /// Session-enabled.
        enabled: bool,
        /// Implements the compaction world.
        compaction: bool,
    },
}

/// The open manager: one flat row list, selection by index.
pub struct ExtensionManager {
    /// Every row, strategy section first.
    pub rows: Vec<ManagedRow>,
    /// The highlighted row.
    pub selected: usize,
}

impl ExtensionManager {
    /// Build rows from a host fetch: strategy picks lead, then the
    /// extensions in the host's order.
    pub fn ready(data: ManagedData) -> ExtensionManager {
        let mut rows = vec![ManagedRow::Strategy {
            name: None,
            active: data.strategy.is_none(),
        }];
        for entry in &data.extensions {
            if entry.compaction {
                rows.push(ManagedRow::Strategy {
                    name: Some(entry.name.clone()),
                    active: data.strategy.as_deref() == Some(entry.name.as_str()),
                });
            }
        }
        for entry in &data.extensions {
            rows.push(ManagedRow::Extension {
                name: entry.name.clone(),
                enabled: entry.enabled,
                compaction: entry.compaction,
            });
        }
        ExtensionManager { rows, selected: 0 }
    }
}

impl Chat {
    /// Open the manager over a synchronous host fetch.
    pub(crate) fn open_extension_manager(&mut self) {
        let Some(list) = self.world.options.hooks.list_managed.clone() else {
            self.world.notice = Some("extensions are not available in this host".to_string());
            return;
        };
        self.extension_manager = Some(ExtensionManager::ready(list()));
    }

    /// Whether the manager owns the keyboard.
    pub fn ext_manager_open(&self) -> bool {
        self.extension_manager.is_some()
    }

    /// The `/ext` entry.
    pub(crate) fn dispatch_ext(&mut self) -> Action {
        self.open_extension_manager();
        Action::Continue
    }

    /// Handle a key while the manager owns the keyboard: navigate,
    /// toggle extensions, pick the strategy, close.
    pub(crate) fn handle_extension_manager_key(
        &mut self,
        _data: &str,
        key: Option<&str>,
    ) -> Option<Action> {
        let mut manager = self.extension_manager.take()?;
        match key {
            Some("escape") => None,
            Some("up") => {
                manager.selected = manager.selected.saturating_sub(1);
                self.extension_manager = Some(manager);
                None
            }
            Some("down") => {
                if !manager.rows.is_empty() {
                    manager.selected = (manager.selected + 1).min(manager.rows.len() - 1);
                }
                self.extension_manager = Some(manager);
                None
            }
            Some("enter") => {
                let row = manager.rows.get(manager.selected).cloned();
                self.extension_manager = Some(manager);
                match row {
                    Some(ManagedRow::Strategy { name, .. }) => {
                        self.pick_managed_strategy(name);
                    }
                    Some(ManagedRow::Extension { name, enabled, .. }) => {
                        self.toggle_managed_extension(&name, !enabled);
                    }
                    None => {}
                }
                None
            }
            _ => {
                self.extension_manager = Some(manager);
                None
            }
        }
    }

    /// Toggle one extension through the host hook, then refresh the
    /// row from a re-fetch so the list shows what stuck.
    fn toggle_managed_extension(&mut self, name: &str, enabled: bool) {
        let Some(toggle) = self.world.options.hooks.toggle_managed.clone() else {
            return;
        };
        self.world.notice = Some(toggle(name.to_string(), enabled));
        self.refresh_extension_manager();
    }

    /// Pick the compaction strategy through the host hook, then
    /// refresh so the active marker moves.
    fn pick_managed_strategy(&mut self, name: Option<String>) {
        let Some(set) = self.world.options.hooks.set_managed_strategy.clone() else {
            return;
        };
        self.world.notice = Some(set(name));
        self.refresh_extension_manager();
    }

    /// Re-fetch rows keeping the selection when it still lands.
    fn refresh_extension_manager(&mut self) {
        let Some(list) = self.world.options.hooks.list_managed.clone() else {
            return;
        };
        let selected = self
            .extension_manager
            .as_ref()
            .map(|manager| manager.selected)
            .unwrap_or(0);
        let mut manager = ExtensionManager::ready(list());
        if !manager.rows.is_empty() {
            manager.selected = selected.min(manager.rows.len() - 1);
        }
        self.extension_manager = Some(manager);
    }
}
