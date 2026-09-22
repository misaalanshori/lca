//! Typed provider stream events.
//!
//! One case per WIT variant case of the `provider` world's stream
//! (ADR-0004, `wit/world-provider.wit` is normative for the wire shape).
//! `VendorEvent` is the reserved case that keeps new vendor concepts from
//! forcing an ABI break.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::usage::Usage;

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
    /// Reserved vendor-specific event: kind string plus JSON payload.
    VendorEvent {
        /// Vendor-defined kind.
        kind: String,
        /// Vendor-defined JSON payload.
        payload: serde_json::Value,
    },
}

impl StreamEvent {
    /// JSON `type` tag used in the headless `--json` envelopes where this
    /// event maps one-to-one (`docs/headless.md`).
    pub fn event_type(&self) -> &'static str {
        match self {
            StreamEvent::TextDelta { .. } => "text",
            StreamEvent::ReasoningDelta { .. } => "text",
            StreamEvent::ToolCallStart { .. } | StreamEvent::ToolCallArgDelta { .. } => "tool-call",
            StreamEvent::ToolCallEnd { .. } => "tool-call",
            StreamEvent::Usage { .. } => "usage",
            StreamEvent::Error { .. } => "error",
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
