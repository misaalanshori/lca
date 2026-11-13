//! Session-log records: the JSON Lines record schema from
//! `docs/session-log-format.md`.
//!
//! Every record carries `v` (schema version), `t` (record type), and `ts`
//! (epoch milliseconds, UTC) before anything type-specific. The `v` field is
//! per record, so a file written across an upgrade holds two versions and a
//! reader handles each by its own version.

use serde::{Deserialize, Serialize};

use crate::tool::ToolResultStatus;
use crate::usage::Usage;

/// Record schema version written by this build.
pub const FORMAT_VERSION: u32 = 1;

/// One nested call's bounded record (gh #77): the name, whether it
/// worked, and the content's head. The full nested transcript never
/// persists; the first `MAX_NESTED_RECORDS` entries win.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NestedCallRecord {
    /// The nested tool's name.
    pub name: String,
    /// Whether it worked.
    pub status: ToolResultStatus,
    /// The content's head (at most `NESTED_CONTENT_HEAD` chars).
    pub content_head: String,
}

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

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// Message text.
        content: String,
        /// Attachment hashes, when present.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
        /// The submit-mode marker when this message was queued while a
        /// turn ran (`steer` / `follow-up`, ADR-0038). Its position in the
        /// log is the injection point. Absent for an ordinary message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queue: Option<String>,
    },
    /// One model message.
    Assistant {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// Message content blocks.
        content: Vec<crate::message::ContentBlock>,
        /// Reasoning text, when the model produced one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        /// The thinking signature the model issued with its reasoning
        /// (gh #41, pi's `thinkingSignature`): resent verbatim on
        /// replay, preserved across compaction. Absent for vendors
        /// without signatures and on old records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_signature: Option<String>,
        /// The thinking level the provider ran at (gh #41, pi's
        /// `providerThinkingLevel`). Absent when unset and on old
        /// records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_thinking_level: Option<String>,
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

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
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

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
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
        /// Active execution time in milliseconds (gh #214): what the
        /// worker spent, excluding permission-prompt waits. Absent on
        /// old lines and non-executed results; readers skip it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        /// Process exit code, when the tool ran a command (gh #40).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        /// Full-output spill path, when the output spilled (gh #40).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        full_output_path: Option<String>,
        /// Bounded nested-call record (gh #77, pi's `nestedCalls`):
        /// nested calls never appear as their own records, so the
        /// calling tool's result keeps the first entries (each a
        /// content head, never the full text). Absent before gh #77.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        nested: Vec<NestedCallRecord>,
        /// Image attachment hashes the tool returned (gh #113, pi's
        /// `ImageContent` in tool results): stored content-addressed
        /// like user attachments, resolved to image blocks at
        /// assembly. Old lines without it load with none (no format
        /// bump: the field defaults, old readers skip it).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
    },
    /// A grant decision made during the session.
    Permission {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
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

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
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

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// First replaced record id (inclusive).
        replaced_from: String,
        /// Last replaced record id (inclusive).
        replaced_to: String,
        /// First kept record id after the cut (gh #36 phase 1): the
        /// next compaction starts here instead of the session start.
        /// Empty on records written before the kept boundary existed.
        #[serde(default)]
        first_kept_id: String,
        /// Replacement content.
        summary: String,
        /// Name of the `compaction` extension that ran (FR-SESS-5).
        strategy: String,
        /// Usage of the summarization call, when the strategy made one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// Files read (not modified) across the summarized range and
        /// earlier compactions, cumulative and bounded (gh #36 phase 3).
        /// Empty on records written before file tracking existed.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        read_files: Vec<String>,
        /// Files written or edited, same accumulation (gh #36 phase 3).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        modified_files: Vec<String>,
        /// The system prompt at compaction time (gh #36 phase 3): a
        /// later compaction whose prompt differs records the change
        /// instead of migrating anything. Absent when the compactor
        /// did not know the prompt (manual `/compact` before plumbing).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
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
    /// A model switch during the session (gh #8, EFG-013): which model
    /// left, which arrived, and the provider/profile that will answer for
    /// it - the log's witness of a switch, including one that spans
    /// profiles (gh #31: routing follows the model). Additive: a reader
    /// that predates it skips the line by type (`docs/session-log-format.md`).
    ModelChange {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// The model the session ran on before this change, when it had
        /// one. Absent when the session's first model was just picked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        /// The model now in use.
        to: String,
        /// Provider extension name that serves `to`.
        provider: String,
        /// The owning profile, when the model belongs to a named one
        /// (absent for the default profile - the same rule the picker's
        /// `profile` extra uses, gh #31).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
    },
    /// A thinking-level switch during the session (pi's
    /// `thinking_level_change` semantics under this log's framing): the
    /// level the next request runs at. Assembly ignores it - the request
    /// itself carries the level.
    ThinkingLevelChange {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// The level now in use (`off`, `minimal`, `low`, `medium`,
        /// `high`, `max`, or a provider extension's own level).
        level: String,
    },
    /// Model-attributed usage that is not an assistant message and does
    /// not participate in model context (pi's `usage` semantics): cache
    /// warms, compaction calls, nested model work an extension reports.
    /// Unknown `kind` values are normal usage, never rejected. Assembly
    /// ignores it content-wise; turn totals stay live-measured.
    Usage {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// What produced the usage (`cache_warm`, ...).
        kind: String,
        /// Provider extension that did the work, when one did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// Model that did the work, when one did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// The counted usage.
        usage: Usage,
    },
    /// A user bookmark on an entry (pi's `label` semantics). `None`
    /// clears the label. Assembly ignores it; the resume and title
    /// surfaces read it later.
    Label {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// The labeled record's identifier.
        target_id: String,
        /// The bookmark text; `None` clears it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Session display name (pi's `session_info` semantics): the name
    /// the session selector shows instead of the first message. Kept
    /// minimal (name + set); no interface reads it yet.
    SessionInfo {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// The display name.
        name: String,
    },
    /// Extension state persistence (pi's `custom` semantics): an
    /// extension's own data, written through the host under the
    /// capability model, never by touching the log file. Does not
    /// participate in model context.
    Custom {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// Which extension owns the entry; readers use it to find
        /// their own entries on reload.
        custom_type: String,
        /// The extension's data.
        data: serde_json::Value,
    },
    /// An extension-injected context message (pi's `custom_message`
    /// semantics): written through the host like `custom`, but this one
    /// DOES participate in model context - assembly injects it as a user
    /// message. `display` controls terminal rendering only, never
    /// whether the model sees it.
    CustomMessage {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// Which extension injected the message.
        custom_type: String,
        /// The injected text (a string; content blocks ride a later
        /// record version if an extension needs them).
        content: String,
        /// Whether the interface shows it with distinct styling.
        display: bool,
        /// Extension metadata, never sent to the model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },
    /// An append-only edit of one earlier context-producing entry (pi's
    /// `context_edit` semantics): the target and its metadata stay
    /// unchanged in raw history, display, exports, and accounting - only
    /// future model context changes. `None` omits the target; a string
    /// replaces its text, keeping role and tool linkage.
    ContextEdit {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// The edited record's identifier (a `user`, `assistant`,
        /// `tool-result`, or `custom-message` record).
        target_id: String,
        /// The replacement text, or `None` to omit the target.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replacement: Option<String>,
    },
    /// An in-file branch jump (gh #37, ADR-0046): navigating to an
    /// earlier record appends this naming it, and later records chain
    /// through it. Transparent to the ancestry walk (the walk jumps to
    /// `target_id`, never follows a parent of its own - it has none);
    /// skipped in display views, carried in audit and exports.
    BranchPoint {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,
        /// The record the branch continues from.
        target_id: String,
    },
    /// A summary of an abandoned branch path (pi's `branch_summary`
    /// semantics on this log's vocabulary): appended at a navigation
    /// point so the new branch inherits what the old one learned.
    /// Assembly injects the summary as context, like a compaction.
    BranchSummary {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,
        /// The previous record's id: the navigation point, named
        /// explicitly (an auto-stamp would point at the branch jump).
        /// `None` only on hand-written records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// The abandoned leaf the summary covers; `None` when the old
        /// path was empty (pi's `"root"` sentinel, typed).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_id: Option<String>,
        /// The summary text.
        summary: String,
    },
    /// Written on a clean exit; absence is normal after a crash.
    SessionEnd {
        /// Schema version.
        v: u32,
        /// Epoch milliseconds.
        ts: u64,
        /// Record identifier.
        id: String,

        /// The previous record's id (gh #37, ADR-0046): the ancestry
        /// link the branch walk follows. `None` on records written
        /// before linkage; the walk falls back to log order there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
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
mod methods;

pub use methods::{
    PREVIOUS_SUMMARY_TYPE, branch_point_record, branch_summary_record, context_edit_record,
    custom_message_record, custom_record, is_previous_summary, label_record, session_info_record,
    thinking_level_change_record, tool_call_record, usage_record,
};
