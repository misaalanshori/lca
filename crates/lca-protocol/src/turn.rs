//! Turn-scoped types: what a turn produced, why it stopped, and the
//! events it emits. They live at the bottom layer because both the
//! interface (`lca-tui`) and the embedding SDK render them without
//! depending on the agent loop itself (SRDD crate decomposition).

use crate::{ToolCall, ToolResult, Usage};

/// Whether a turn ended cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStatus {
    /// The turn completed.
    Ok,
    /// The turn ended with an error.
    Error,
}

/// Why a turn stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The model stopped without requesting a tool.
    Stop,
    /// The tool-call iteration limit was hit (FR-CORE-9).
    IterationLimit,
    /// The user cancelled (FR-CORE-5).
    Cancelled,
    /// A provider error after the retry limit (FR-CORE-7) or a rejection.
    Error,
}

/// What one turn produced.
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    /// Clean or error.
    pub status: TurnStatus,
    /// Why it stopped.
    pub stop_reason: StopReason,
    /// The turn's usage, summed across provider calls (FR-CORE-8).
    pub usage: Usage,
    /// The surfaced error, when the turn ended in one.
    pub error: Option<String>,
}

/// Events the interface and headless mode render. Owned copies, so sinks
/// store them freely.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// A chunk of response text (FR-CORE-4).
    TextDelta(String),
    /// A chunk of reasoning text.
    ReasoningDelta(String),
    /// A completed response's text (headless `text` envelope).
    AssistantText(String),
    /// A response's usage (headless `usage` envelope, FR-CORE-8).
    Usage(Usage),
    /// A tool call is about to run (after permission).
    ToolStarted(ToolCall),
    /// A tool call finished (headless `tool-result` envelope).
    ToolFinished(ToolResult),
    /// A chunk of live shell output while a command runs (FR-TOOL-4).
    ToolOutputChunk {
        /// The running call.
        call_id: String,
        /// The chunk as text.
        chunk: String,
    },
    /// A retry was scheduled (FR-CORE-6).
    RetryScheduled {
        ///1-based attempt number about to run.
        attempt: u32,
        /// The configured limit.
        max: u32,
        /// Delay before it runs.
        delay_ms: u64,
        /// The error that caused the retry.
        error: String,
    },
    /// An error surfaced to the interface (headless `error` envelope).
    Error {
        /// What went wrong.
        message: String,
        /// Class from `lca_provider::ProviderError` or `internal`.
        class: String,
        /// Whether a retry could have helped.
        retryable: bool,
    },
    /// An extension lifecycle event: load, disable, trap, capability
    /// denial, or cache divergence (the headless `extension-event`
    /// envelope, `docs/headless.md`).
    ExtensionEvent {
        /// The extension's identity.
        extension: String,
        /// The event kind: `disabled`, `error`, `collision`, ...
        event: String,
        /// Detail text.
        detail: String,
    },
    /// The turn ended.
    TurnEnded {
        /// Clean or error.
        status: TurnStatus,
        /// Why.
        stop_reason: StopReason,
    },
}
