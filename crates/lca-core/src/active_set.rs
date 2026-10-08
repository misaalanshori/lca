//! The session's active tool set (gh #77's pi-active-tools,
//! gh #67's run-scoped selection): which registered tools declare,
//! which executor-table built-ins show, and whether `tool_search` is
//! pinned. Split from `registry.rs` for the file ceiling; the state
//! lives on `ExtensionRegistry`, so this is one `impl` block, not a
//! second owner.

use super::registry::ExtensionRegistry;
use lca_protocol::ToolExposure;

/// The session's dynamic tool set (pi's active tools, gh #77): `None`
/// is the default (every registered tool is active); `Some` is a
/// `set-active-tools` replacement. Only registered names take
/// effect; unknown names are ignored at set time, never here.
#[derive(Debug, Default)]
pub(crate) struct ActiveSet {
    pub(crate) names: Option<std::collections::HashSet<String>>,
    /// Bumped on every set; the turn records the transcript entry.
    pub(crate) revision: u64,
    /// Executor-table (built-in) tools selected for the session (gh
    /// #67): `None` is every built-in (no flags); the request builder
    /// filters its static table through this.
    pub(crate) builtin: Option<std::collections::HashSet<String>>,
    /// Whether the `tool_search` offer shows (gh #67): `None` is the
    /// standing rule (while undisclosed tools exist); a tool-selection
    /// flag pins it (only an explicit `--tools` naming shows it).
    pub(crate) tool_search: Option<bool>,
}

impl ExtensionRegistry {
    /// Whether a tool name is active (gh #77): the default set holds
    /// every registered non-hidden tool; a `set-active-tools`
    /// replacement holds exactly its applied names.
    pub(crate) fn is_active(&self, name: &str, exposure: ToolExposure) -> bool {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        match &active.names {
            None => exposure != ToolExposure::Hidden,
            Some(names) => names.contains(name),
        }
    }

    /// Replace the session's active tool set (pi's `setActiveTools`,
    /// gh #77): only registered names take effect; unknown names are
    /// ignored and reported, never applied. Returns `(applied,
    /// ignored)`, both sorted. The turn records the transcript entry
    /// before the next model request.
    pub fn set_active_tools(&self, names: &[String]) -> (Vec<String>, Vec<String>) {
        let mut applied = Vec::new();
        let mut ignored = Vec::new();
        for name in names {
            if self.tool_schemas.contains_key(name) {
                applied.push(name.clone());
            } else {
                ignored.push(name.clone());
            }
        }
        applied.sort();
        ignored.sort();
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        active.names = Some(applied.iter().cloned().collect());
        active.revision += 1;
        (applied, ignored)
    }

    /// The active tool names, sorted (gh #77's `getActiveTools`).
    pub fn active_tools(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tool_schemas
            .iter()
            .filter(|(name, spec)| self.is_active(name, spec.exposure))
            .map(|(name, _)| name.clone())
            .collect();
        names.sort();
        names
    }

    /// Select executor-table (built-in) tools for the session (gh
    /// #67): `None` keeps every built-in (no flags); the request
    /// builder filters its static table through this. Never bumps the
    /// revision (run-start state, not a mid-turn change).
    pub fn set_builtin_active(&self, names: Option<std::collections::HashSet<String>>) {
        self.active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .builtin = names;
    }

    /// Whether a built-in tool shows (gh #67).
    pub fn is_builtin_active(&self, name: &str) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .builtin
            .as_ref()
            .is_none_or(|names| names.contains(name))
    }

    /// Pin the `tool_search` offer (gh #67): `None` restores the
    /// standing rule. A tool-selection flag pins it off unless the
    /// flag names `tool_search` explicitly.
    pub fn set_tool_search(&self, show: Option<bool>) {
        self.active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .tool_search = show;
    }

    /// Whether the `tool_search` offer shows (gh #67).
    pub fn tool_search_pinned(&self) -> Option<bool> {
        self.active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .tool_search
    }

    /// The active set's revision: the turn compares it across requests
    /// to record transcript entries for mid-turn changes (gh #77).
    pub fn active_revision(&self) -> u64 {
        self.active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .revision
    }
}
