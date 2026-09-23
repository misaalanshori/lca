//! Provider-world dispatch types: the data both sides of the `provider`
//! world speak. They live with the rest of the protocol types so the
//! contract crate (and a guest depending on it) never pulls in provider
//! machinery or a channel runtime (ADR-0019).

use std::collections::BTreeMap;

use crate::message::ChatMessage;
use crate::stream::StreamEvent;
use crate::tool::ToolSpec;

/// One model a provider offers (FR-PROV-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Provider-specific model identifier.
    pub id: String,
    /// Display name for the picker.
    pub name: String,
    /// Context window in tokens; `0` when the provider does not publish one.
    pub context_window: u32,
    /// Maximum output tokens; `0` when the provider does not publish one.
    pub max_tokens: u32,
}

/// One completion request as the core assembles it. Moved here from
/// `lca-provider` when the dispatch trait needed it; `lca-provider`
/// re-exports it so existing call sites do not change.
#[derive(Debug, Clone, Default)]
pub struct CompletionRequest {
    /// The resolved message list (post-compaction, post-transform).
    pub messages: Vec<ChatMessage>,
    /// Tool specs advertised to the model.
    pub tools: Vec<ToolSpec>,
    /// Model identifier.
    pub model: String,
    /// Count of leading messages the host considers the stable, cacheable
    /// prefix (FR-CACHE-5). Advisory: providers with no cache marker ignore
    /// it safely.
    pub stable_prefix: usize,
    /// Reserved map for non-structural additions.
    pub extras: BTreeMap<String, String>,
}

/// Where a provider's events go while a completion streams: channel-agnostic
/// so neither the contract crate nor a guest needs a channel runtime.
/// `push` returning `false` means the receiver is gone and the provider
/// should stop streaming (FR-CONC-3: dropping the receiver cancels).
pub trait EventSink: Send + Sync {
    /// Deliver one event; `false` = receiver gone, stop.
    fn push(&self, event: StreamEvent) -> bool;
}

/// What an identity operation did (ADR-0012). Every provider exports all
/// three functions and returns `NotSupported` where it has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityOutcome {
    /// The operation ran (login stored tokens, logout cleared them).
    Ok,
    /// This provider has no such operation (ADR-0012's optional-export rule).
    NotSupported,
    /// The operation ran and failed; the string is shown to the user.
    Failed(String),
}
