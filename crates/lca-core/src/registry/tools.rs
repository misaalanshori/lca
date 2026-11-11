//! Tool exposure and the tools-import surface (gh #548): which registered
//! tools declare, what `tool_search` discovers, the namespace grouping, and
//! the `ToolsRegistryView` the `tools` import reaches through the host.
//!
//! Split from `registry/mod.rs` for the 1,200-line ceiling. The state stays on
//! [`ExtensionRegistry`]: one table, one active set, no second owner - these are
//! readers and planners, not a second registry.

use std::collections::HashMap;
use std::sync::Arc;

use lca_ext_abi::ExtensionDispatch;
use lca_protocol::{ToolAnnotations, ToolExposure, ToolSpec};

use super::{ExtensionRegistry, NamespaceView};

impl ExtensionRegistry {
    /// The extension that registered a bare tool name, if any.
    ///
    /// Re-plans first (gh #584), so a bridged tool whose server landed after
    /// registration resolves here - which is the half of the defect that
    /// answered `unknown tool`.
    pub fn tool_owner(&self, tool: &str) -> Option<&Arc<dyn ExtensionDispatch>> {
        self.refresh();
        let table = self.lock_table();
        let index = *table.tools.get(tool)?;
        // Borrowed out of the guard deliberately: the handle outlives the table,
        // and the entry index it names is the same one `refresh` just wrote.
        drop(table);
        self.entries.get(index).map(|entry| &entry.handle)
    }

    /// Whether a tool name is registered right now (gh #584's active set reads
    /// this instead of the table). Re-plans first, like every spec reader.
    pub(crate) fn has_tool(&self, name: &str) -> bool {
        self.refresh();
        self.lock_table().tool_schemas.contains_key(name)
    }

    /// The schema of a registered tool, for validating its arguments before
    /// dispatch (extension authoring guide: the host validates, then calls).
    pub fn tool_schema(&self, name: &str) -> Option<ToolSpec> {
        self.refresh();
        self.lock_table().tool_schemas.get(name).cloned()
    }

    /// The annotations a registered tool declares (#78): structural
    /// hints first (normalized from extras at registration), else
    /// `None`. The permission prompt may show them; nothing decides
    /// on them.
    pub fn tool_annotations(&self, name: &str) -> Option<ToolAnnotations> {
        self.refresh();
        self.lock_table()
            .tool_schemas
            .get(name)
            .and_then(|spec| spec.annotations.clone())
    }

    /// Whether the host may run a tool by name right now (gh #77's
    /// callable set): direct tools while active, codemode and deferred
    /// tools whenever registered; model-only and hidden tools never.
    pub fn is_callable(&self, name: &str) -> bool {
        self.refresh();
        let enabled = {
            let table = self.lock_table();
            table
                .tools
                .get(name)
                .map(|index| {
                    self.entries[*index]
                        .enabled
                        .load(std::sync::atomic::Ordering::SeqCst)
                })
                .unwrap_or(false)
        };
        if !enabled {
            return false;
        }
        let exposure = {
            let table = self.lock_table();
            table.tool_schemas.get(name).map(|spec| spec.exposure)
        };
        match exposure {
            Some(exposure) => match exposure {
                ToolExposure::Direct => self.is_active(name, exposure),
                ToolExposure::Codemode | ToolExposure::Deferred => true,
                ToolExposure::ModelOnly | ToolExposure::Hidden => false,
            },
            None => false,
        }
    }

    /// The callable tools for orchestrators (the `tools` import's
    /// `list-tools`, gh #77): name plus description, sorted.
    pub fn callable_names(&self) -> Vec<(String, String)> {
        self.refresh();
        // The names are collected under the lock and the predicate is asked
        // afterwards: `is_callable` takes the same lock, and `std::sync::Mutex`
        // is not re-entrant (gh #584).
        let specs: Vec<(String, String)> = self
            .lock_table()
            .tool_schemas
            .values()
            .map(|spec| (spec.name.clone(), spec.description.clone()))
            .collect();
        let mut names: Vec<(String, String)> = specs
            .into_iter()
            .filter(|(name, _)| self.is_callable(name))
            .collect();
        names.sort();
        names
    }

    /// Deferred discovery (gh #77's `tool_search` shape, pi's
    /// `deferred`): codemode and deferred tools whose name,
    /// description, or namespace matches the query, sorted. Hidden
    /// and model-only tools never match; direct tools are declared,
    /// not discovered. Matching is case-insensitive substring; an
    /// empty query lists every discoverable tool.
    pub fn tool_search(&self, query: &str) -> Vec<ToolSpec> {
        let needle = query.to_lowercase();
        let mut matches: Vec<ToolSpec> = self
            .tool_specs()
            .into_iter()
            .filter(|spec| {
                matches!(
                    spec.exposure,
                    ToolExposure::Codemode | ToolExposure::Deferred
                )
            })
            .filter(|spec| {
                if needle.is_empty() {
                    return true;
                }
                let haystack = match &spec.namespace {
                    Some(namespace) => format!(
                        "{} {} {} {}",
                        spec.name, spec.description, namespace.name, namespace.description
                    ),
                    None => format!("{} {}", spec.name, spec.description),
                };
                haystack.to_lowercase().contains(&needle)
            })
            .collect();
        matches.sort_by(|a, b| a.name.cmp(&b.name));
        matches
    }

    /// Namespace grouping rows (gh #77): one row per namespace with
    /// its enabled tools, both sorted. Tools without a namespace
    /// group nowhere.
    pub fn namespaces(&self) -> Vec<NamespaceView> {
        let mut groups: HashMap<String, NamespaceView> = HashMap::new();
        for spec in self.tool_specs() {
            if let Some(namespace) = &spec.namespace {
                groups
                    .entry(namespace.name.clone())
                    .or_insert_with(|| NamespaceView {
                        name: namespace.name.clone(),
                        description: namespace.description.clone(),
                        tools: Vec::new(),
                    })
                    .tools
                    .push(spec.name.clone());
            }
        }
        let mut views: Vec<NamespaceView> = groups.into_values().collect();
        for view in &mut views {
            view.tools.sort();
        }
        views.sort_by(|a, b| a.name.cmp(&b.name));
        views
    }

    /// What the next declaration request carries (gh #77): enabled,
    /// active, `direct` tools, plus `deferred` tools the model already
    /// discovered through `tool_search` (#55). Everything else reaches
    /// the model through orchestration, or not at all.
    pub fn declared_tool_specs(&self) -> Vec<ToolSpec> {
        let discovered = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .discovered
            .clone();
        let mut specs: Vec<ToolSpec> = self
            .tool_specs()
            .into_iter()
            .filter(|spec| {
                (spec.exposure == ToolExposure::Direct && self.is_active(&spec.name, spec.exposure))
                    || (spec.exposure == ToolExposure::Deferred && discovered.contains(&spec.name))
            })
            .collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }

    /// Every extension tool spec, for the provider's tool list. Sorted by
    /// name: a provider's prompt cache and the request-assembly snapshot key
    /// on stable bytes, and the registry's maps are unordered.
    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.refresh();
        let table = self.lock_table();
        let mut specs: Vec<ToolSpec> = table
            .tool_schemas
            .iter()
            .filter(|(name, _)| {
                table
                    .tools
                    .get(*name)
                    .map(|index| {
                        self.entries[*index]
                            .enabled
                            .load(std::sync::atomic::Ordering::SeqCst)
                    })
                    .unwrap_or(false)
            })
            .map(|(_, spec)| spec.clone())
            .collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }
}
