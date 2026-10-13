//! Session debug traces (gh #258 §3B): append shapes and lineage
//! walks. Split from `store.rs` under the workspace's 1,200-line file
//! ceiling; every value crossing in is content-free by construction.

/// Trace one append: type, id, and bytes — never content.
pub(crate) fn trace_append(record: &lca_protocol::Record, bytes: usize) {
    lca_debug::session::event(
        "record.append",
        &format!(
            "type={} id={} bytes={bytes}",
            record_type_name(record),
            record.id().unwrap_or("?")
        ),
    );
}

/// The record's type name: content-free, so appends stay inspectable
/// without leaking.
fn record_type_name(record: &lca_protocol::Record) -> &'static str {
    use lca_protocol::Record::*;
    match record {
        SessionStart { .. } => "session-start",
        User { .. } => "user",
        Assistant { .. } => "assistant",
        ToolCall { .. } => "tool-call",
        ToolResult { .. } => "tool-result",
        Permission { .. } => "permission",
        ExtensionEvent { .. } => "extension-event",
        Compaction { .. } => "compaction",
        ForkPoint { .. } => "fork-point",
        ModelChange { .. } => "model-change",
        ThinkingLevelChange { .. } => "thinking-level-change",
        Usage { .. } => "usage",
        Label { .. } => "label",
        SessionInfo { .. } => "session-info",
        Custom { .. } => "custom",
        CustomMessage { .. } => "custom-message",
        ContextEdit { .. } => "context-edit",
        BranchPoint { .. } => "branch-point",
        BranchSummary { .. } => "branch-summary",
        SessionEnd { .. } => "session-end",
    }
}
