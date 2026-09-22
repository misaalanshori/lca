//! Session-log records: the JSON Lines record schema from
//! `docs/session-log-format.md`.
//!
//! Every record carries `v` (schema version), `t` (record type), and `ts`
//! (epoch milliseconds, UTC) before anything type-specific. The `v` field is
//! per record, so a file written across an upgrade holds two versions and a
//! reader handles each by its own version.

use serde::{Deserialize, Serialize};

use crate::tool::{ToolCall, ToolResultStatus};
use crate::usage::Usage;

/// Record schema version written by this build.
pub const FORMAT_VERSION: u32 = 1;

/// One line of a session log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum Record {
    /// First record of every log.
    SessionStart {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Agent version that opened the session.
        agent_version: String,
        /// Extension ABI version at open time.
        abi_version: String,
        /// Canonical working directory.
        working_dir: String,
    },
    /// One user message.
    User {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier, sortable by creation order.
        id: String,
        /// Message text.
        content: String,
        /// Attachment hashes, when present.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
    },
    /// One model message.
    Assistant {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,
        /// Message content blocks.
        content: Vec<crate::message::ContentBlock>,
        /// Reasoning text, when the model produced one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        /// Model identifier used.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Provider extension name used.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// Recorded usage, including cache fields (FR-CORE-8).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    /// A tool call the model requested.
    ToolCall {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,
        /// Provider-side call identifier.
        call_id: String,
        /// Tool name.
        name: String,
        /// Argument string (JSON object text).
        arguments: String,
        /// Whether the tool is built in or comes from an extension.
        source: ToolSource,
    },
    /// The outcome of a tool call.
    ToolResult {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,
        /// Call this answers.
        call_id: String,
        /// Outcome.
        status: ToolResultStatus,
        /// Text shown to the model, unless an attachment hash replaces it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        /// Attachment hash for large content.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachment: Option<String>,
        /// Whether the content was truncated (FR-TOOL-7).
        #[serde(default)]
        truncated: bool,
    },
    /// A grant decision made during the session.
    Permission {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,
        /// What was attempted, shown verbatim.
        action: String,
        /// What the user chose.
        decision: PermissionDecision,
        /// The pattern, when the decision was `always`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
    },
    /// A load, disable, trap, capability denial, or cache divergence.
    ExtensionEvent {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,
        /// Extension identity.
        extension: String,
        /// Event kind.
        event: String,
        /// Detail text.
        detail: String,
    },
    /// Marks a compacted range; the summary replaces it on read.
    Compaction {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,
        /// First replaced record id (inclusive).
        replaced_from: String,
        /// Last replaced record id (inclusive).
        replaced_to: String,
        /// Replacement content.
        summary: String,
        /// Name of the `compaction` extension that ran (FR-SESS-5).
        strategy: String,
        /// Usage of the summarization call, when the strategy made one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    /// Names the fork origin in a forked session.
    ForkPoint {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,
        /// Parent session id.
        parent_session: String,
        /// Record id in the parent the fork was taken at.
        record_id: String,
    },
    /// Written on a clean exit; absence is normal after a crash.
    SessionEnd {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,
    },
}

/// Which delivery path a tool call came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolSource {
    /// One of the built-in tools.
    Builtin,
    /// An extension-registered tool.
    Extension,
}

/// What the user chose at a permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDecision {
    /// Allow this call only.
    Once,
    /// Allow this pattern, written to the user grant store (FR-PERM-8).
    Always,
    /// Refuse the call.
    Denied,
}

impl Record {
    /// The record's schema version.
    pub fn version(&self) -> u32 {
        match self {
            Record::SessionStart { v, .. }
            | Record::User { v, .. }
            | Record::Assistant { v, .. }
            | Record::ToolCall { v, .. }
            | Record::ToolResult { v, .. }
            | Record::Permission { v, .. }
            | Record::ExtensionEvent { v, .. }
            | Record::Compaction { v, .. }
            | Record::ForkPoint { v, .. }
            | Record::SessionEnd { v, .. } => *v,
        }
    }

    /// Record identifier, when the type has one.
    pub fn id(&self) -> Option<&str> {
        match self {
            Record::User { id, .. }
            | Record::Assistant { id, .. }
            | Record::ToolCall { id, .. }
            | Record::ToolResult { id, .. }
            | Record::Permission { id, .. }
            | Record::ExtensionEvent { id, .. }
            | Record::Compaction { id, .. }
            | Record::ForkPoint { id, .. }
            | Record::SessionEnd { id, .. } => Some(id),
            Record::SessionStart { .. } => None,
        }
    }

    /// The record's `t` tag as written to the log.
    pub fn type_tag(&self) -> &'static str {
        match self {
            Record::SessionStart { .. } => "session-start",
            Record::User { .. } => "user",
            Record::Assistant { .. } => "assistant",
            Record::ToolCall { .. } => "tool-call",
            Record::ToolResult { .. } => "tool-result",
            Record::Permission { .. } => "permission",
            Record::ExtensionEvent { .. } => "extension-event",
            Record::Compaction { .. } => "compaction",
            Record::ForkPoint { .. } => "fork-point",
            Record::SessionEnd { .. } => "session-end",
        }
    }
}

/// Convenience constructor for a `tool-call` record (tests and the core loop).
pub fn tool_call_record(
    ts: u64,
    id: impl Into<String>,
    call: &ToolCall,
    source: ToolSource,
) -> Record {
    Record::ToolCall {
        v: FORMAT_VERSION,
        ts,
        id: id.into(),
        call_id: call.call_id.clone(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
        source,
    }
}
