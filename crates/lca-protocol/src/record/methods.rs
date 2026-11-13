//! `Record`'s accessors and the convenience constructors (gh #559: the schema
//! half lives in `record/mod.rs`; this file is the codec half, so neither grows
//! into gate 11's ceiling).

use crate::tool::ToolCall;

use super::{FORMAT_VERSION, Record, ToolSource};

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
            | Record::ModelChange { v, .. }
            | Record::ThinkingLevelChange { v, .. }
            | Record::Usage { v, .. }
            | Record::Label { v, .. }
            | Record::SessionInfo { v, .. }
            | Record::Custom { v, .. }
            | Record::CustomMessage { v, .. }
            | Record::ContextEdit { v, .. }
            | Record::BranchPoint { v, .. }
            | Record::BranchSummary { v, .. }
            | Record::SessionEnd { v, .. } => *v,
        }
    }

    /// The ancestry link: the previous record's id, when the type
    /// carries one (gh #37, ADR-0046). `None` on records written
    /// before linkage and on junction types (`ForkPoint`,
    /// `BranchPoint`) the walk treats specially.
    pub fn parent(&self) -> Option<&str> {
        match self {
            Record::User { parent, .. }
            | Record::Assistant { parent, .. }
            | Record::ToolCall { parent, .. }
            | Record::ToolResult { parent, .. }
            | Record::Permission { parent, .. }
            | Record::ExtensionEvent { parent, .. }
            | Record::Compaction { parent, .. }
            | Record::ModelChange { parent, .. }
            | Record::ThinkingLevelChange { parent, .. }
            | Record::Usage { parent, .. }
            | Record::Label { parent, .. }
            | Record::SessionInfo { parent, .. }
            | Record::Custom { parent, .. }
            | Record::CustomMessage { parent, .. }
            | Record::ContextEdit { parent, .. }
            | Record::BranchSummary { parent, .. }
            | Record::SessionEnd { parent, .. } => parent.as_deref(),
            Record::SessionStart { .. } | Record::ForkPoint { .. } | Record::BranchPoint { .. } => {
                None
            }
        }
    }

    /// Stamp the ancestry link (gh #37): the store fills this on
    /// append when the writer left it empty. Junction types ignore
    /// the stamp (the walk never follows their parent).
    pub fn set_parent(&mut self, parent: &str) {
        let slot = match self {
            Record::User { parent, .. }
            | Record::Assistant { parent, .. }
            | Record::ToolCall { parent, .. }
            | Record::ToolResult { parent, .. }
            | Record::Permission { parent, .. }
            | Record::ExtensionEvent { parent, .. }
            | Record::Compaction { parent, .. }
            | Record::ModelChange { parent, .. }
            | Record::ThinkingLevelChange { parent, .. }
            | Record::Usage { parent, .. }
            | Record::Label { parent, .. }
            | Record::SessionInfo { parent, .. }
            | Record::Custom { parent, .. }
            | Record::CustomMessage { parent, .. }
            | Record::ContextEdit { parent, .. }
            | Record::BranchSummary { parent, .. }
            | Record::SessionEnd { parent, .. } => parent,
            Record::SessionStart { .. } | Record::ForkPoint { .. } | Record::BranchPoint { .. } => {
                return;
            }
        };
        *slot = Some(parent.to_string());
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
            | Record::ModelChange { id, .. }
            | Record::ThinkingLevelChange { id, .. }
            | Record::Usage { id, .. }
            | Record::Label { id, .. }
            | Record::SessionInfo { id, .. }
            | Record::Custom { id, .. }
            | Record::CustomMessage { id, .. }
            | Record::ContextEdit { id, .. }
            | Record::BranchPoint { id, .. }
            | Record::BranchSummary { id, .. }
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
            Record::ModelChange { .. } => "model-change",
            Record::ThinkingLevelChange { .. } => "thinking-level-change",
            Record::Usage { .. } => "usage",
            Record::Label { .. } => "label",
            Record::SessionInfo { .. } => "session-info",
            Record::Custom { .. } => "custom",
            Record::CustomMessage { .. } => "custom-message",
            Record::ContextEdit { .. } => "context-edit",
            Record::BranchPoint { .. } => "branch-point",
            Record::BranchSummary { .. } => "branch-summary",
            Record::SessionEnd { .. } => "session-end",
        }
    }
}

/// Convenience constructor for a `thinking-level-change` record.
pub fn thinking_level_change_record(ts: u64, id: impl Into<String>, level: &str) -> Record {
    Record::ThinkingLevelChange {
        v: FORMAT_VERSION,
        ts,
        id: id.into(),
        parent: None,
        level: level.to_string(),
    }
}

/// Convenience constructor for a `usage` record: model-attributed usage
/// outside any assistant message.
pub fn usage_record(
    ts: u64,
    id: impl Into<String>,
    kind: &str,
    provider: Option<&str>,
    model: Option<&str>,
    usage: crate::usage::Usage,
) -> Record {
    Record::Usage {
        v: FORMAT_VERSION,
        ts,
        id: id.into(),
        parent: None,
        kind: kind.to_string(),
        provider: provider.map(str::to_string),
        model: model.map(str::to_string),
        usage,
    }
}

/// Convenience constructor for a `label` record (`None` clears).
pub fn label_record(
    ts: u64,
    id: impl Into<String>,
    target_id: &str,
    label: Option<&str>,
) -> Record {
    Record::Label {
        v: FORMAT_VERSION,
        ts,
        id: id.into(),
        parent: None,
        target_id: target_id.to_string(),
        label: label.map(str::to_string),
    }
}

/// Convenience constructor for a `session-info` record.
pub fn session_info_record(ts: u64, id: impl Into<String>, name: &str) -> Record {
    Record::SessionInfo {
        v: FORMAT_VERSION,
        ts,
        id: id.into(),
        parent: None,
        name: name.to_string(),
    }
}

/// The marker kind the host uses for the in-band previous summary
/// (gh #36 phase 2): the latest summary rides as one `custom` record
/// at the candidate's head so the strategy refines instead of
/// restarting, with no WIT change. Single source: the host builds it,
/// the compaction strategy reads it.
pub const PREVIOUS_SUMMARY_TYPE: &str = "previous-summary";

/// Whether a record is the host's in-band previous-summary marker.
pub fn is_previous_summary(record: &Record) -> bool {
    matches!(record, Record::Custom { custom_type, .. } if custom_type == PREVIOUS_SUMMARY_TYPE)
}

/// Convenience constructor for a `custom` record: extension state the
/// host appends on the extension's behalf. Extensions never touch the
/// log file; the host calls this under the capability model.
pub fn custom_record(
    ts: u64,
    id: impl Into<String>,
    custom_type: &str,
    data: serde_json::Value,
) -> Record {
    Record::Custom {
        v: FORMAT_VERSION,
        ts,
        id: id.into(),
        parent: None,
        custom_type: custom_type.to_string(),
        data,
    }
}

/// Convenience constructor for a `custom-message` record: extension
/// context injection, host-appended like `custom`.
pub fn custom_message_record(
    ts: u64,
    id: impl Into<String>,
    custom_type: &str,
    content: &str,
    display: bool,
) -> Record {
    Record::CustomMessage {
        v: FORMAT_VERSION,
        ts,
        id: id.into(),
        parent: None,
        custom_type: custom_type.to_string(),
        content: content.to_string(),
        display,
        details: None,
    }
}

/// Convenience constructor for a `context-edit` record (`None` omits
/// the target from future model context).
pub fn context_edit_record(
    ts: u64,
    id: impl Into<String>,
    target_id: &str,
    replacement: Option<&str>,
) -> Record {
    Record::ContextEdit {
        v: FORMAT_VERSION,
        ts,
        id: id.into(),
        parent: None,
        target_id: target_id.to_string(),
        replacement: replacement.map(str::to_string),
    }
}

/// Convenience constructor for a `branch-point` record: the walk
/// jumps to `target_id`, so no parent is stamped (gh #37).
pub fn branch_point_record(ts: u64, id: impl Into<String>, target_id: &str) -> Record {
    Record::BranchPoint {
        v: FORMAT_VERSION,
        ts,
        id: id.into(),
        target_id: target_id.to_string(),
    }
}

/// Convenience constructor for a `branch-summary` record: the parent
/// names the navigation point explicitly (gh #37).
pub fn branch_summary_record(
    ts: u64,
    id: impl Into<String>,
    parent: &str,
    from_id: Option<&str>,
    summary: &str,
) -> Record {
    Record::BranchSummary {
        v: FORMAT_VERSION,
        ts,
        id: id.into(),
        parent: Some(parent.to_string()),
        from_id: from_id.map(str::to_string),
        summary: summary.to_string(),
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
        parent: None,
        call_id: call.call_id.clone(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
        source,
    }
}
