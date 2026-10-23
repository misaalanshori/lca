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
///
/// **The spelling (gh #454).** The field names on the wire are MCP's, which are
/// camelCase (`readOnlyHint`), and that is what every real sender uses - an MCP
/// server's `annotations` object, and a WIT guest's `extras["annotations"]` JSON.
/// This type used to derive `Deserialize` with no rename, so serde expected
/// `read_only_hint` and every real payload decoded to **all-`None`** without an
/// error: the hints were dropped, not rejected, which is why gh #303's
/// measurement saw "hints arrive" while nothing arrived.
///
/// So: camelCase **out** (the interop spelling), and camelCase **or** snake_case
/// **in** (the old shape this tree's own fixtures wrote, kept accepted so a
/// widening never breaks an existing reader). Tolerance is deliberate - an
/// unknown key is ignored rather than fatal, because a hint is display metadata
/// that the permission layer never decides on (ADR-0047), so a hint MCP adds
/// later must not break decoding for a peer that sends it. A typo is silently
/// ignored as the price; `annotation_spelling_tests` pins that choice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    /// The tool only reads.
    #[serde(
        default,
        alias = "read_only_hint",
        skip_serializing_if = "Option::is_none"
    )]
    pub read_only_hint: Option<bool>,
    /// The tool may destroy.
    #[serde(
        default,
        alias = "destructive_hint",
        skip_serializing_if = "Option::is_none"
    )]
    pub destructive_hint: Option<bool>,
    /// Repeated calls are safe.
    #[serde(
        default,
        alias = "idempotent_hint",
        skip_serializing_if = "Option::is_none"
    )]
    pub idempotent_hint: Option<bool>,
    /// The tool reaches the open world.
    #[serde(
        default,
        alias = "open_world_hint",
        skip_serializing_if = "Option::is_none"
    )]
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

impl ToolSpec {
    /// Fill `annotations` from the `annotations` extras key when the
    /// guest did not declare them structurally (#78): single-tool
    /// guests cross no annotations field in WIT, so the JSON rides
    /// extras instead. Explicit annotations always win; invalid JSON
    /// degrades to absent, never an error.
    pub fn with_annotations_from_extras(mut self) -> Self {
        if self.annotations.is_none()
            && let Some(json) = self.extras.get("annotations")
            && let Ok(hints) = serde_json::from_str::<ToolAnnotations>(json)
        {
            self.annotations = Some(hints);
        }
        self
    }
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
    /// Wall-clock active execution time in milliseconds (gh #214):
    /// strictly the worker's time, excluding permission-prompt waits.
    /// `None` for cached, instant, or pre-execution results (denied,
    /// refused, schema-invalid). Optional so older session files load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
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
            duration_ms: None,
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
            duration_ms: None,
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
            duration_ms: None,
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
            duration_ms: None,
        }
    }
}

#[cfg(test)]
mod annotation_spelling_tests {
    //! gh #454: the annotations everyone sends are camelCase, and this type
    //! decoded them to all-`None`.
    //!
    //! **Every row here starts from JSON text**, which is the whole point. The
    //! existing coverage built the struct in Rust (or wrote the snake_case
    //! spelling), so it passed while every real payload decoded to nothing. A
    //! guard that constructs the value it is checking cannot catch a decode bug.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

    use super::{ToolAnnotations, ToolSpec};

    // Verifies: gh #454 - **the defect.** This is the MCP spelling, and the
    // spelling a WIT guest puts in `extras["annotations"]`. Before the fix this
    // decoded to all-`None` **without an error** - the hints were dropped, not
    // rejected, which is why gh #303's measurement saw "hints arrive" while
    // nothing arrived.
    #[test]
    fn the_camel_case_payload_everyone_sends_decodes() {
        let hints: ToolAnnotations = serde_json::from_str(
            r#"{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}"#,
        )
        .expect("the MCP spelling decodes");
        assert_eq!(hints.read_only_hint, Some(true));
        // `false` is a claim, not an absence: the difference between
        // "not destructive" and "unknown" is exactly what a hint is for.
        assert_eq!(hints.destructive_hint, Some(false));
        assert_eq!(hints.idempotent_hint, Some(true));
        assert_eq!(hints.open_world_hint, Some(false));
    }

    // Verifies: gh #454 design point 1 - **snake_case still decodes.** This
    // tree's own fixtures and guests written against the old shape must keep
    // working; acceptance is widened, not moved.
    #[test]
    fn the_snake_case_spelling_still_decodes() {
        let hints: ToolAnnotations = serde_json::from_str(
            r#"{"read_only_hint":true,"destructive_hint":false,"idempotent_hint":true,"open_world_hint":false}"#,
        )
        .expect("the old spelling still decodes");
        assert_eq!(hints.read_only_hint, Some(true));
        assert_eq!(hints.destructive_hint, Some(false));
        assert_eq!(hints.idempotent_hint, Some(true));
        assert_eq!(hints.open_world_hint, Some(false));
    }

    // Verifies: gh #454 design point 1 - **serialization emits one spelling,
    // and it is camelCase** (the interop spelling). Pinned as text rather than
    // through a round-trip, so a change to either direction is visible.
    #[test]
    fn serialization_emits_camel_case() {
        let hints = ToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(true),
            idempotent_hint: Some(true),
            open_world_hint: Some(true),
        };
        let json = serde_json::to_string(&hints).expect("serializes");
        assert_eq!(
            json,
            r#"{"readOnlyHint":true,"destructiveHint":true,"idempotentHint":true,"openWorldHint":true}"#
        );
        // Absent hints stay absent: an "unknown" claim is not a `false` one.
        let bare = serde_json::to_string(&ToolAnnotations::default()).expect("serializes");
        assert_eq!(bare, "{}", "an empty hint set is an empty object");
    }

    // Verifies: gh #454 - a round-trip through this tree is stable, whichever
    // spelling the input used. This is the row that would catch the two
    // directions drifting apart.
    #[test]
    fn a_round_trip_is_stable_from_either_spelling() {
        for input in [
            r#"{"readOnlyHint":true,"openWorldHint":false}"#,
            r#"{"read_only_hint":true,"open_world_hint":false}"#,
        ] {
            let hints: ToolAnnotations = serde_json::from_str(input).expect("decodes");
            let text = serde_json::to_string(&hints).expect("serializes");
            assert_eq!(
                text, r#"{"readOnlyHint":true,"openWorldHint":false}"#,
                "input {input} normalises to the camelCase shape"
            );
            let again: ToolAnnotations = serde_json::from_str(&text).expect("re-decodes");
            assert_eq!(again, hints, "and re-decoding changes nothing");
        }
    }

    // Verifies: gh #454 design point 2 - **an unknown key is ignored**, and
    // that is deliberate.
    //
    // The house bias is a loud failure at a *trust* boundary and tolerance
    // elsewhere. A hint is display metadata the permission layer never decides
    // on (ADR-0047), so a future MCP hint must not break decoding for a peer
    // that sends it - and `deny_unknown_fields` would do exactly that. The cost
    // is that a typo is silently ignored; it is paid knowingly, and this row is
    // where a change of mind becomes visible.
    #[test]
    fn an_unknown_hint_key_is_ignored_not_fatal() {
        let hints: ToolAnnotations =
            serde_json::from_str(r#"{"readOnlyHint":true,"someFutureHint":true,"title":"A tool"}"#)
                .expect("an unknown key does not break decoding");
        assert_eq!(hints.read_only_hint, Some(true));
    }

    // Verifies: gh #454 - **the extras path**, which is what gh #303 depends on:
    // a guest's `extras["annotations"]` JSON reaches the real field.
    #[test]
    fn a_guest_camel_case_extras_payload_reaches_the_struct() {
        let spec = ToolSpec {
            name: "guest_tool".to_string(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: super::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: [(
                "annotations".to_string(),
                r#"{"readOnlyHint":true,"openWorldHint":true}"#.to_string(),
            )]
            .into_iter()
            .collect(),
        }
        .with_annotations_from_extras();
        let hints = spec.annotations.expect("the guest's hints were lifted");
        assert_eq!(
            hints.read_only_hint,
            Some(true),
            "a guest's camelCase hint survives the extras decode"
        );
        assert_eq!(hints.open_world_hint, Some(true));
    }
}
