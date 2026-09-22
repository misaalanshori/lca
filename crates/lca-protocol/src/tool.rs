//! Tool specifications, calls, and results.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A tool as advertised to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Tool name, unique within the session's namespace.
    pub name: String,
    /// Description the model reads when choosing a tool.
    pub description: String,
    /// JSON Schema for the arguments.
    pub parameters: serde_json::Value,
    /// Reserved map for non-structural extensions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
}

/// One tool call the model requested.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-side call identifier; results reference it.
    pub call_id: String,
    /// Tool name.
    pub name: String,
    /// Argument string (JSON object text) as accumulated by the host.
    pub arguments: String,
}

/// Outcome status of a tool call, as recorded in the session log and the
/// headless `tool-result` envelope (`docs/headless.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolResultStatus {
    /// The tool ran and reported success.
    Ok,
    /// The tool ran and reported failure, or arguments were invalid.
    Error,
    /// A hook or the permission layer refused the call.
    Denied,
    /// The command exceeded its timeout.
    Timeout,
}

/// The result of one tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Call this answers.
    pub call_id: String,
    /// Outcome.
    pub status: ToolResultStatus,
    /// Text handed back to the model.
    pub content: String,
    /// Whether `content` was cut down by the configured size limit
    /// (FR-TOOL-7).
    #[serde(default)]
    pub truncated: bool,
    /// Reserved map for non-structural extensions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
}

impl ToolResult {
    /// A successful result.
    pub fn ok(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        ToolResult {
            call_id: call_id.into(),
            status: ToolResultStatus::Ok,
            content: content.into(),
            truncated: false,
            extras: BTreeMap::new(),
        }
    }

    /// A failed result.
    pub fn error(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        ToolResult {
            call_id: call_id.into(),
            status: ToolResultStatus::Error,
            content: content.into(),
            truncated: false,
            extras: BTreeMap::new(),
        }
    }

    /// A refused result; `reason` is what the model sees.
    pub fn denied(call_id: impl Into<String>, reason: impl Into<String>) -> Self {
        ToolResult {
            call_id: call_id.into(),
            status: ToolResultStatus::Denied,
            content: reason.into(),
            truncated: false,
            extras: BTreeMap::new(),
        }
    }

    /// A timeout result (FR-TOOL-5).
    pub fn timeout(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        ToolResult {
            call_id: call_id.into(),
            status: ToolResultStatus::Timeout,
            content: content.into(),
            truncated: false,
            extras: BTreeMap::new(),
        }
    }
}
