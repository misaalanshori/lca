//! The `/ext` manager host side (gh #3): list installed extensions
//! with states, toggle enable/disable (grant store + live registry),
//! pick the compaction strategy (config + live preference). Split
//! for the line ceiling; the hooks live in `hooks.rs`.

use std::sync::Arc;

use super::Ui;

/// The manager's rows plus the preferred strategy, assembled
/// host-side (the Hub #227 discovers; this manages).
pub fn assemble_rows(
    entries: Vec<(String, bool, Vec<lca_ext_abi::World>)>,
    preferred: Option<String>,
) -> lca_ui::ManagedData {
    let extensions = entries
        .into_iter()
        .map(|(name, enabled, worlds)| lca_ui::ManagedEntry {
            compaction: worlds.contains(&lca_ext_abi::World::Compaction),
            name,
            enabled,
        })
        .collect();
    lca_ui::ManagedData {
        extensions,
        strategy: preferred,
    }
}

impl Ui {
    /// List installed extensions with states plus the strategy.
    pub(super) fn managed_rows(&self) -> lca_ui::ManagedData {
        let registry = self.registry();
        assemble_rows(registry.managed_entries(), registry.preferred_compaction())
    }

    /// Toggle one extension: persist per project, apply live, report.
    pub(super) fn toggle_managed(&self, name: String, enabled: bool) -> String {
        match crate::ext::set_extension_enabled(&crate::data_dir(), &self.cwd, &name, enabled) {
            Ok(notice) => {
                self.registry().set_enabled(&name, enabled);
                notice
            }
            Err(err) => format!("could not toggle {name}: {err}"),
        }
    }

    /// Pick the compaction strategy: persist the key, apply live, report.
    pub(super) fn set_managed_strategy(&self, name: Option<String>) -> String {
        super::hooks::persist_ui_setting("compaction.strategy", name.clone());
        self.registry().set_preferred_compaction(name.clone());
        match name {
            Some(name) => format!("compaction strategy: {name}"),
            None => "compaction strategy: automatic".to_string(),
        }
    }

    /// The `/ext` manager hooks (gh #3): list, toggle, strategy.
    /// Built here beside the row assembly so `hooks.rs` only assigns.
    /// `Arc` self like the hub hooks: the closures share the live
    /// wiring instead of cloning it.
    pub(super) fn extman_hooks(
        self: &Arc<Self>,
    ) -> (
        lca_ui::ListManaged,
        lca_ui::ToggleManaged,
        lca_ui::SetManagedStrategy,
    ) {
        let list = self.clone();
        let toggle = self.clone();
        let strategy = self.clone();
        (
            Arc::new(move || list.managed_rows()) as lca_ui::ListManaged,
            Arc::new(move |name, enabled| toggle.toggle_managed(name, enabled))
                as lca_ui::ToggleManaged,
            Arc::new(move |name| strategy.set_managed_strategy(name)) as lca_ui::SetManagedStrategy,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::assemble_rows;
    use lca_ext_abi::World;

    // Verifies: gh #3 - the manager rows carry states and the strategy
    // the registry holds (the UI marks from these, no second source).
    #[test]
    fn managed_rows_carry_states_and_strategy() {
        let data = assemble_rows(
            vec![
                ("a".to_string(), true, vec![World::Tool]),
                ("b".to_string(), false, vec![World::Compaction]),
            ],
            Some("b".to_string()),
        );
        assert_eq!(data.extensions.len(), 2);
        assert!(data.extensions[0].enabled);
        assert!(!data.extensions[0].compaction);
        assert!(data.extensions[1].compaction);
        assert_eq!(data.strategy, Some("b".to_string()));
    }
}
