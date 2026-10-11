//! The permission prompt's vocabulary: what an action is, what the
//! interface shows, and what approvals match against. Split from
//! `lib.rs` for the workspace file ceiling (gate 11). Behaviour
//! unchanged.

use super::*;

/// Something sensitive the model or an extension wants to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Run a shell command.
    Shell {
        /// The exact command string.
        command: String,
        /// Working directory.
        cwd: PathBuf,
    },
    /// Write a file outside the workspace (FR-TOOL-3).
    WritePath {
        /// The exact target path.
        path: PathBuf,
    },
    /// Read, list, or search a path outside the workspace (FR-TOOL-3).
    ReadPath {
        /// The exact target path.
        path: PathBuf,
    },
    /// Reach a named host outside every declared grant, the ad hoc attach
    /// FR-PERM-16 defines. Offered by the login/setup flow, never by an
    /// extension's own call, so the user attaches it deliberately.
    Net {
        /// The exact host being added.
        host: String,
    },
}

impl Action {
    /// Exactly what the interface shows while waiting (FR-UI-4).
    pub fn display(&self) -> String {
        match self {
            Action::Shell { command, cwd } => format!("{command} (in {})", cwd.display()),
            Action::WritePath { path } => format!("write {}", path.display()),
            Action::ReadPath { path } => format!("read {}", path.display()),
            Action::Net { host } => format!("connect to {host}"),
        }
    }

    /// The action with tool hints appended (#78): hints inform the
    /// human, never the decision - approval matching reads the
    /// action, never this text. No hints (or none set) renders
    /// exactly `display`.
    pub fn display_annotated(&self, annotations: Option<&lca_protocol::ToolAnnotations>) -> String {
        let base = self.display();
        let Some(hints) = annotations else {
            return base;
        };
        let mut words = Vec::new();
        if hints.read_only_hint == Some(true) {
            words.push("read-only");
        }
        if hints.destructive_hint == Some(true) {
            words.push("destructive");
        }
        if hints.idempotent_hint == Some(true) {
            words.push("idempotent");
        }
        if hints.open_world_hint == Some(true) {
            words.push("open-world");
        }
        if words.is_empty() {
            return base;
        }
        format!("{base} ({})", words.join(", "))
    }

    /// The value approvals match against.
    pub(super) fn match_value(&self) -> String {
        match self {
            Action::Shell { command, .. } => command.clone(),
            Action::WritePath { path } => path.display().to_string(),
            Action::ReadPath { path } => path.display().to_string(),
            Action::Net { host } => host.clone(),
        }
    }

    /// The pattern an always-approval records: the exact thing shown.
    pub fn suggested_pattern(&self) -> String {
        self.match_value()
    }
}
