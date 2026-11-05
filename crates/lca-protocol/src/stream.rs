//! Typed provider stream events.
//!
//! One case per WIT variant case of the `provider` world's stream
//! (ADR-0004, `wit/world-provider.wit` is normative for the wire shape).
//! `VendorEvent` is the reserved case that keeps new vendor concepts from
//! forcing an ABI break.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::usage::Usage;

/// A thinking signature preserved for multi-turn continuity (gh #41):
/// the Anthropic kit emits it as a [`StreamEvent::VendorEvent`] when a
/// thinking block closes, and the host resends it on replay — vendors
/// that require signatures reject transcripts that drop them. The kind
/// lives here (not in the kit) so the host matches it without
/// depending on any wire crate.
pub const THINKING_SIGNATURE_KIND: &str = "thinking-signature";

/// The vendor-event kind that carries an output-token truncation across the WASM
/// boundary (gh #390 gap 1).
///
/// [`StreamEvent::Truncated`] has no WIT case, so a guest that only forwards events
/// re-encodes it as a vendor event - the ABI's documented escape hatch - and the host
/// maps it back here. Without this the cut would be lost at the boundary for every
/// provider delivered through the subscription kit, which is most of them.
pub const TRUNCATED_KIND: &str = "truncated";

/// One event from a streaming completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum StreamEvent {
    /// A chunk of response text.
    TextDelta {
        /// The chunk.
        delta: String,
    },
    /// A chunk of reasoning text.
    ReasoningDelta {
        /// The chunk.
        delta: String,
    },
    /// Opens a tool call. Must precede every argument delta for that id
    /// (FR-PROV-7).
    ToolCallStart {
        /// Provider-side call identifier.
        call_id: String,
        /// Tool name.
        name: String,
    },
    /// A fragment of the call's argument string.
    ToolCallArgDelta {
        /// Call this fragment belongs to.
        call_id: String,
        /// The fragment.
        delta: String,
    },
    /// Closes a call; the accumulated argument string is complete.
    ToolCallEnd {
        /// The call identifier.
        call_id: String,
    },
    /// Token usage and cost for the call.
    Usage {
        /// The usage record.
        usage: Usage,
    },
    /// Ends the stream with an error.
    Error {
        /// Human-readable message.
        message: String,
        /// Whether the host should retry (FR-CORE-6).
        retryable: bool,
    },
    /// The provider stopped because the response hit its output-token limit
    /// (gh #390 gap 1): the wire's `finish_reason: "length"` /
    /// `stop_reason: "max_tokens"`.
    ///
    /// Deliberately **not** an `Error`: a truncation is not a failure, so a
    /// retry would produce the same cut and an error envelope would make a
    /// script treat a usable partial answer as a crashed turn. It is a fact
    /// about *why* the stream ended, and the turn reports it as
    /// `StopReason::Length`.
    ///
    /// This is why the signal needed a variant at all: before it, a `length`
    /// finish left no trace the host could see for an ordinary chat turn, so a
    /// response cut mid-tool-call looked like a normal end and the half-streamed
    /// call was re-attempted in a loop.
    Truncated,
    /// Reserved vendor-specific event: kind string plus JSON payload.
    VendorEvent {
        /// Vendor-defined kind.
        kind: String,
        /// Vendor-defined JSON payload.
        payload: serde_json::Value,
    },
}

impl StreamEvent {
    /// The **family** tag for this event: one word per kind of content, not the
    /// per-variant serde tag.
    ///
    /// Gh #339 finding 4: `ReasoningDelta` used to answer `"text"`, the same as
    /// `TextDelta`. That conflation is the one an LLM-stream consumer cannot
    /// tolerate - answer text and reasoning text are exactly what
    /// `docs/headless.md` keeps in separate envelopes (`text-delta` versus
    /// `thinking-delta`), because a reader that adds thinking to the answer shows a
    /// conversation the model never had. Reasoning now answers `"reasoning"`, which is
    /// the variant's own name and a prefix of its wire tag `reasoning-delta`.
    ///
    /// The three tool-call variants really are one family (`"tool-call"`), which is why
    /// this is a family tag and not a rename of the serde tags: `type_tag`-style
    /// one-to-one is what `Record::type_tag` does, and this is deliberately coarser.
    ///
    /// Measured before changing it (the brief asked for the caller survey, not a
    /// guess): **zero** call sites in the workspace, and the headless envelopes are
    /// not produced here at all - `lca-cli`'s `HeadlessSink` emits them from
    /// `TurnEvent` (`docs/headless.md`'s `text-delta` / `thinking-delta` /
    /// `tool-call`). So the old mapping broke no caller, and the row
    /// `stream_event_family_tags_keep_reasoning_apart` keeps a future one from
    /// inheriting it.
    pub fn event_type(&self) -> &'static str {
        match self {
            StreamEvent::TextDelta { .. } => "text",
            StreamEvent::ReasoningDelta { .. } => "reasoning",
            StreamEvent::ToolCallStart { .. } | StreamEvent::ToolCallArgDelta { .. } => "tool-call",
            StreamEvent::ToolCallEnd { .. } => "tool-call",
            StreamEvent::Usage { .. } => "usage",
            StreamEvent::Error { .. } => "error",
            // The headless envelope's own kind (gh #390 gap 1). A script that
            // only knows the pre-existing kinds can ignore it; one that wants to
            // detect a cut answer now can.
            StreamEvent::Truncated => "truncated",
            StreamEvent::VendorEvent { .. } => "vendor-event",
        }
    }

    /// Vendor events with an unrecognized kind are recorded and ignored
    /// (`docs/flows.md`, streaming pipeline).
    pub fn is_vendor(&self) -> bool {
        matches!(self, StreamEvent::VendorEvent { .. })
    }
}

/// Reserved per-event extension map kept alongside a stream for hosts that
/// need to attach non-structural data without changing the variant.
pub type EventExtras = BTreeMap<String, String>;
