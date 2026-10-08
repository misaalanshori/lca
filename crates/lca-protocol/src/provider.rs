//! Provider-world dispatch types: the data both sides of the `provider`
//! world speak. They live with the rest of the protocol types so the
//! contract crate (and a guest depending on it) never pulls in provider
//! machinery or a channel runtime (ADR-0019).

use std::collections::BTreeMap;

use crate::message::ChatMessage;
use crate::stream::StreamEvent;
use crate::tool::ToolSpec;

/// `ModelInfo` extras keys carrying image behavior (#39). Provider
/// extensions write them from their limits table; the host reads them
/// through the tool executor's image policy — the non-structural
/// channel, so no ABI change. They live here (not in `lca-tools`, which
/// does not build for the guest target) so both delivery modes share
/// them.
pub const IMAGE_VISION_EXTRA: &str = "image.vision";
/// `WIDTHxHEIGHT:BYTES`, present only with a vendor-sourced profile.
pub const IMAGE_RESIZE_EXTRA: &str = "image.resize";
/// Cache lifetimes (`short=SEC,long=SEC`, tiers present only), carried
/// for the warming epic (gh #64); no consumer reads it yet.
pub const PROMPT_CACHE_EXTRA: &str = "prompt_cache";

/// A per-model image profile: pi's `ModelImageResizeOptions` shape.
/// `max_bytes` bounds the encoded payload. Vendor-sourced when present;
/// pi's conservative defaults fill the gaps downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageResize {
    /// Long-edge cap in pixels.
    pub max_width: u32,
    /// Short-edge cap in pixels.
    pub max_height: u32,
    /// Encoded-payload cap in bytes.
    pub max_bytes: usize,
}

impl Default for ImageResize {
    /// Pi's conservative defaults for omitted fields (2000 by 2000
    /// pixels, 4.5 MiB encoded; JPEG quality 80 is applied at encode).
    fn default() -> Self {
        ImageResize {
            max_width: 2000,
            max_height: 2000,
            max_bytes: 4_500_000,
        }
    }
}

/// One model a provider offers (FR-PROV-2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelInfo {
    /// Provider-specific model identifier.
    pub id: String,
    /// Display name for the picker.
    pub name: String,
    /// Context window in tokens; `0` when the provider does not publish one.
    pub context_window: u32,
    /// Maximum output tokens; `0` when the provider does not publish one.
    pub max_tokens: u32,
    /// Non-structural per-model data: the ABI record carries
    /// `extras: list<extra-pair>` already (`wit/world-provider.wit`), so
    /// this is the host-side half of that field, not a new one (gh #31 -
    /// the picker's per-model provenance rides here).
    pub extras: std::collections::BTreeMap<String, String>,
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

/// The capability surface every provider extension's transport runs on:
/// exactly what the `provider` world's `net` and `credentials` imports
/// expose, so neither delivery mode can reach a socket or a credential
/// file directly (Phase 3 exit test). The native handle implements it
/// with the shared capability engine, the WASM guest with the host's
/// imports; the trait lives here so both sides and the contract crate
/// speak one type (ADR-0019).
pub trait ProviderCap: Send + Sync {
    /// Start one HTTP request; the response body is read through
    /// `net_read_body` until it returns `None`.
    fn net_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<u32, crate::CapabilityError>;
    /// The response's status line.
    fn net_response_status(&self, handle: u32) -> Result<u16, crate::CapabilityError>;
    /// The next body chunk; `None` at end of stream.
    fn net_read_body(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, crate::CapabilityError>;
    /// Release the response.
    fn net_close_response(&self, handle: u32) -> Result<(), crate::CapabilityError>;
    /// Read one credential; a denial reads as absence, exactly as the
    /// capability catalog specifies.
    fn credentials_get(&self, key: &str) -> Option<String>;
    /// Store one credential in this extension's own namespace.
    fn credentials_set(&self, key: &str, value: &str) -> Result<(), crate::CapabilityError>;
    /// Delete one credential.
    fn credentials_delete(&self, key: &str) -> Result<(), crate::CapabilityError>;
    /// Read one of the extension's own resources (ADR-0030). Default:
    /// absent, so a provider with no resource bag compiles unchanged.
    fn resource_read(&self, path: &str) -> Result<Vec<u8>, crate::CapabilityError> {
        Err(crate::CapabilityError::NotFound(format!(
            "no resource `{path}`"
        )))
    }
}

/// The loopback authorization flow, the other half of the `provider`
/// world's imports (capability catalog `oauth`): the extension builds
/// the authorization URL and PKCE challenge itself, the host owns the
/// listener (FR-PROV-3, FR-PROV-4).
pub trait OauthCap: Send + Sync {
    /// Pick a port, bind the loopback listener, return the redirect URL
    /// plus a flow handle.
    fn oauth_begin(&self, redirect_path: &str) -> Result<(String, u32), crate::CapabilityError>;
    /// Open a URL in the user's browser (best effort).
    fn oauth_open(&self, url: &str) -> Result<(), crate::CapabilityError>;
    /// Block until the callback arrives; its parsed query parameters.
    fn oauth_await(&self, handle: u32) -> Result<Vec<(String, String)>, crate::CapabilityError>;
    /// Abandon a flow and stop its listener.
    fn oauth_end(&self, handle: u32) -> Result<(), crate::CapabilityError>;
}

/// Whether a provider failure message names transient capacity (gh #202):
/// an overloaded model or status, not a refusal. The turn loop retries
/// these even when the provider marked the failure non-retryable, and
/// the wire kits mark matching mid-stream error payloads retryable at
/// the source. Spec-pinned patterns (pi `3874b3e98` narrows to the
/// capacity case); keep this the single definition - the kits cannot
/// depend on `lca-core`, so it lives here, beside the request types.
pub fn is_capacity_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("at capacity")
        || lower.contains("is at capacity")
        || lower.contains("overloaded")
        || lower.contains("529")
}

#[cfg(test)]
mod tests {
    use super::is_capacity_error;

    // Verifies: gh #202 - the spec's capacity patterns match, and
    // refusals, auth failures, and rate limits do not (capacity-only).
    #[test]
    fn capacity_patterns_match_and_only_they_do() {
        for message in [
            "Selected model is at capacity",
            "the model is at capacity, try again",
            "The engine is currently overloaded, please try again later.",
            "Overloaded",
            "provider returned HTTP 529: overloaded",
        ] {
            assert!(is_capacity_error(message), "capacity: {message}");
        }
        for message in [
            "invalid api key",
            "context length exceeded",
            "rate limit exceeded, slow down",
            "the response was not a server-sent event stream",
            "",
        ] {
            assert!(!is_capacity_error(message), "not capacity: {message}");
        }
    }
}
