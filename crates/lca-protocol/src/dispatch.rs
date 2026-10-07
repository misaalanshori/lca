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
    /// Show an image in the transcript as a placeholder and set a notice
    /// (FR-UI-13). Used by `/attach` so the staged image is visible at once.
    AttachImage {
        /// The media type.
        media_type: String,
        /// The image bytes (for dimensions and size).
        bytes: Vec<u8>,
        /// The notice to show alongside it.
        note: String,
    },
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

/// One `hooks-tool-call` handler's patch (gh #45, pi's `tool_call`):
/// `arguments` replaces the argument string (later handlers see the
/// replacement); `block` vetoes the call with the reason the model
/// sees. Both `None` is an observation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCallPatch {
    /// Replacement argument string.
    pub arguments: Option<String>,
    /// Veto reason.
    pub block: Option<String>,
}

/// One `hooks-tool-result` handler's patch (gh #45, pi's
/// `tool_result`): omitted fields stay as they are.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolResultPatch {
    /// Replacement result text.
    pub content: Option<String>,
    /// Flipped outcome.
    pub is_error: Option<bool>,
}

/// A settle handler's answer (gh #45, pi's `turn_end` /
/// `agent_before_settle`): append entries and continue one more
/// request, or settle. Both absent settles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettleDecision {
    /// Text injected as a context message the next request sees.
    pub append: Option<String>,
    /// Run exactly one more provider request before settling.
    pub continue_once: bool,
}

/// A `session_before_compact` handler's veto (gh #45): allow compacts
/// as planned, deny cancels this compaction with the reason.
#[derive(Debug, Clone, PartialEq)]
pub enum CompactVerdict {
    /// Compact as planned.
    Allow,
    /// Cancel this compaction.
    Deny(String),
}

/// A `project_trust` handler's vote (gh #45, pi's `project_trust`):
/// the first yes/no decides, undecided falls through to the next
/// handler and finally to the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustVote {
    /// Trust the project.
    Yes,
    /// Do not trust the project.
    No,
    /// No opinion; ask the next handler or the operator.
    Undecided,
}

impl TrustVote {
    /// Parse the wire string case-insensitively; anything else reads
    /// as `undecided` (a typo must not veto a trust flow).
    pub fn parse(value: &str) -> TrustVote {
        match value.to_ascii_lowercase().as_str() {
            "yes" => TrustVote::Yes,
            "no" => TrustVote::No,
            _ => TrustVote::Undecided,
        }
    }
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
