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
