//! Tool specifications, calls, and results.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// How the model reaches a tool (pi's `ToolExposure`, gh #77). Only
/// `direct` tools are declared to the model. A string on the wire, not
/// an enum: new values stay additive (the abi-versioning table's
/// string-vocabulary row); unknown values refuse loudly at
/// registration instead of guessing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolExposure {
    /// Declared to the model while active, and callable while active.
    #[default]
    Direct,
    /// Never declared and never callable: host-side tools the model
    /// neither sees nor calls (a narrower `hidden`).
    ModelOnly,
    /// Callable whenever registered, never declared; listed for
    /// codemode-style callers.
    Codemode,
    /// Like `codemode`, but found through discovery (`tool_search`)
    /// instead of listing; resolves on first use.
    Deferred,
    /// Registered but unreachable: not declared, not callable, not
    /// searchable. Re-register `hidden` to withdraw a tool.
    Hidden,
}

impl ToolExposure {
    /// Parse the wire string; unknown values refuse with the value
    /// itself, so a typo fails at registration, not at call time.
    pub fn parse(value: &str) -> Result<ToolExposure, String> {
        match value {
            "direct" => Ok(ToolExposure::Direct),
            "model-only" => Ok(ToolExposure::ModelOnly),
            "codemode" => Ok(ToolExposure::Codemode),
            "deferred" => Ok(ToolExposure::Deferred),
            "hidden" => Ok(ToolExposure::Hidden),
            unknown => Err(format!("unknown tool exposure `{unknown}`")),
        }
    }

    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            ToolExposure::Direct => "direct",
            ToolExposure::ModelOnly => "model-only",
            ToolExposure::Codemode => "codemode",
            ToolExposure::Deferred => "deferred",
            ToolExposure::Hidden => "hidden",
        }
    }
}

/// What namespace a tool belongs to (pi's `ToolNamespace`, gh #77):
/// related tools list under one heading with the description;
/// `instructions` holds longer usage guidance discovery surfaces,
/// never the declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolNamespace {
    /// The grouping name.
    pub name: String,
    /// Listed alongside the tools.
    pub description: String,
    /// Longer guidance; shown by discovery, never declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Hints about what a tool does (MCP tool annotations, gh #77): the
/// model sees them; the host's permission layer never decides on
/// them. A hint is not a bypass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolAnnotations {
    /// The tool only reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    /// The tool may destroy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    /// Repeated calls are safe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    /// The tool reaches the open world.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

/// A tool as advertised to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Tool name, unique within the session's namespace.
    pub name: String,
    /// Description the model reads when choosing a tool.
    pub description: String,
    /// JSON Schema for the arguments.
    pub parameters: serde_json::Value,
    /// How the model reaches the tool (gh #77); absent means
    /// `direct`, so specs written before exposure parse unchanged.
    #[serde(default)]
    pub exposure: ToolExposure,
    /// The grouping the tool belongs to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<ToolNamespace>,
    /// What the tool does; model-visible, never permission-deciding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    /// Reserved map for non-structural extensions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
}

/// One tool call the model requested.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-side call identifier; results reference it. A nested
    /// call (gh #77) carries `<parent id>/<n>` here.
    pub call_id: String,
    /// Tool name.
    pub name: String,
    /// Argument string (JSON object text) as accumulated by the host.
    pub arguments: String,
    /// The calling tool's id, when another tool made this call (pi's
    /// `parentToolCallId`). Absent for model-issued calls; skipped on
    /// the wire when absent, so older readers never see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_call_id: Option<String>,
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

/// An image a tool returned (R5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageContent {
    /// IANA media type, e.g. `image/png`.
    pub media_type: String,
    /// The raw bytes.
    pub bytes: Vec<u8>,
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
    /// Process exit code, when the tool ran a command (gh #40: the
    /// `shell` tool sets it; other tools leave it absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Full-output spill path, when the output was truncated and a
    /// session was attached to spill into (gh #40: names the attachment
    /// file the session record references by hash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    /// Bounded nested-call record (gh #77): filed under the calling
    /// tool, attached by the turn when the call finishes. Empty for
    /// calls that nested nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nested: Vec<crate::record::NestedCallRecord>,
    /// Images the tool returned (R5): a tool that read an image file, for
    /// example. Empty for text-only results.
    ///
    /// These carry the **bytes**, not an attachment hash. The turn event is
    /// in-process: the tool already holds the bytes it just read, so a hash
    /// would add a store round-trip for no isolation (ADR-0029's cycle-3
    /// annotation).
    /// ponytail: bytes are fine while producer and consumer share an
    /// address space; a process- or network-crossing host (the NFR-11 web
    /// host, an embedding SDK shipping events to a remote observer) wants
    /// the hash instead - content addressing already exists on disk
    /// (`SessionStore::attachment_path`), so the upgrade is a payload swap
    /// in the event, not a message-shape change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageContent>,
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
            images: Vec::new(),
            extras: BTreeMap::new(),
            exit_code: None,
            full_output_path: None,
            nested: Vec::new(),
        }
    }

    /// A failed result.
    pub fn error(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        ToolResult {
            call_id: call_id.into(),
            status: ToolResultStatus::Error,
            content: content.into(),
            truncated: false,
            images: Vec::new(),
            extras: BTreeMap::new(),
            exit_code: None,
            full_output_path: None,
            nested: Vec::new(),
        }
    }

    /// A refused result; `reason` is what the model sees.
    pub fn denied(call_id: impl Into<String>, reason: impl Into<String>) -> Self {
        ToolResult {
            call_id: call_id.into(),
            status: ToolResultStatus::Denied,
            content: reason.into(),
            truncated: false,
            images: Vec::new(),
            extras: BTreeMap::new(),
            exit_code: None,
            full_output_path: None,
            nested: Vec::new(),
        }
    }

    /// A timeout result (FR-TOOL-5).
    pub fn timeout(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        ToolResult {
            call_id: call_id.into(),
            status: ToolResultStatus::Timeout,
            content: content.into(),
            truncated: false,
            images: Vec::new(),
            extras: BTreeMap::new(),
            exit_code: None,
            full_output_path: None,
            nested: Vec::new(),
        }
    }
}
