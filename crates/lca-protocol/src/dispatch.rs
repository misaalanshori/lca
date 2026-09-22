//! Dispatch-layer types shared by both delivery modes (ADR-0019).

use std::collections::BTreeMap;

use crate::tool::{ToolCall, ToolResult};

/// A slash command an extension provides, as registered in the table the
/// input editor completes against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// The full command name: `<extension>.<command>` (ADR-0012's
    /// namespacing), or a reserved built-in slot a first-party native
    /// extension fills.
    pub name: String,
    /// Argument hint shown in the input editor.
    pub hint: String,
    /// Completion behavior: `none`, `file`, or `list`.
    pub completion: String,
    /// Reserved map (ABI `extras`).
    pub extras: BTreeMap<String, String>,
}

/// What a command asks the input editor to do (the `command` world's
/// `effect` variant).
#[derive(Debug, Clone, PartialEq)]
pub enum CommandEffect {
    /// Insert text into the input editor.
    InsertText(String),
    /// Submit a prompt to the agent.
    SubmitPrompt(String),
    /// Show text in the notice area (the `show-widget` case, rendered by
    /// the host until the `ui` world lands in Phase 6).
    ShowWidget(String),
    /// Do nothing.
    None,
}

/// The pre-tool hook's verdict (the `hooks` world's `action` variant).
#[derive(Debug, Clone, PartialEq)]
pub enum HookAction {
    /// Continue to the permission layer.
    Allow,
    /// End the call with a reason the model sees, without prompting
    /// (FR-CORE-10).
    Deny(String),
    /// Replace the call; the replacement passes through the permission
    /// layer like any other call and is not re-hooked (SRDD hooks).
    Replace(ToolCall),
}

/// A hook that observes a completed tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct PostToolObservation {
    /// The call as it ran (post-replacement).
    pub call: ToolCall,
    /// Its result.
    pub result: ToolResult,
}

/// A dispatch-level failure: which answers the caller, which disables.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DispatchError {
    /// The extension is disabled for the session (FR-EXT-3/5).
    #[error("extension is disabled for this session")]
    Disabled,
    /// The extension does not implement the world this call needs.
    #[error("extension `{extension}` does not implement the {world} world")]
    MissingWorld {
        /// The extension's name.
        extension: String,
        /// The world asked for.
        world: &'static str,
    },
    /// The call failed; the host decides whether that disables.
    #[error("extension call failed: {0}")]
    Failed(String),
}
