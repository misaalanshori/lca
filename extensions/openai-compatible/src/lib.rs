//! Any OpenAI-compatible chat completions endpoint: the default provider,
//! native-linked by default per ADR-0013, dual-mode like every extension
//! under `extensions/` (Phase 3's port from the Phase 1 compiled-in
//! placeholder, `docs/providers/openai-compatible.md`).
//!
//! Both modes share this file. The transport never touches a socket: it
//! speaks the `net` and `credentials` capability surface through
//! [`ProviderCap`], which the native handle implements with the shared
//! [`lca_tools::Capabilities`] engine and the WASM guest implements with
//! the host's imports. The cache-boundary hint is advisory and
//! intentionally unused for marker placement: OpenAI-shaped endpoints
//! cache automatically. `cache_read` and `cache_write` still come back
//! from `cached_tokens`, which is what the cache-waste measurement
//! consumes.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims, the
//! only `unsafe` in this crate; the module holds the allowance.

#![deny(unsafe_code)]

use std::collections::BTreeMap;

use lca_protocol::CompletionRequest;
use lca_protocol::{
    ChatMessage, ContentBlock, IdentityOutcome, MessageRole, StreamEvent, ToolSpec, Usage,
};

// The capability traits live with the protocol types now that more than
// one provider shares them; re-exported so this crate's public surface
// (`run_provider_stream(&dyn ProviderCap, ...)`) does not change.
pub use lca_protocol::{OauthCap, ProviderCap};

/// The manifest this form ships with (single source for the grants
/// [`manifest_grants`] builds; a test keeps them in step with
/// `extension.toml`).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: `net` for the default endpoint and
/// the credential namespace (FR-PERM-6), everything else denied.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net: vec![
            lca_permissions::parse_net_pattern("api.openai.com")
                .expect("the manifest's own host pattern parses"),
        ],
        credentials: true,
        ..Default::default()
    }
}

/// Configuration. Endpoints and keys come from the environment for the
/// native form (the WASM form reads only `credentials`, since a
/// sandboxed extension must not see the host's environment).
#[derive(Debug, Clone)]
pub struct Settings {
    /// Base URL including the version prefix, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    /// Bearer token from the environment, when present.
    pub api_key: Option<String>,
    /// Default model identifier.
    pub model: String,
    /// The model's context window when the endpoint publishes none; the
    /// FR-SESS-4 threshold needs it, `0` means unknown (never compacts).
    pub context_window: u32,
    /// Whether to send OpenAI's `prompt_cache_key` cache-affinity pin
    /// (V1, ADR-0031). Default on; a strict proxy that rejects unknown
    /// body fields can turn it off with `OPENAI_PROMPT_CACHE_KEY=0`.
    pub prompt_cache_key: bool,
    /// Whether the endpoint honors a `reasoning_effort` body field (R1).
    /// Default on; a strict proxy that rejects unknown body fields can
    /// turn it off with `OPENAI_SUPPORTS_REASONING=0`.
    pub supports_reasoning: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            // OpenCode Go is an ordinary OpenAI-shaped endpoint with its
            // own key name (LCA-PROMPT's real-provider notes); either
            // variable is a plain bearer key, no OAuth (ADR-0012's
            // login shape for this provider).
            api_key: std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("OPENCODE_API_KEY"))
                .ok()
                .filter(|key| !key.is_empty()),
            model: std::env::var("OPENAI_MODEL")
                .or_else(|_| std::env::var("LCA_MODEL"))
                .unwrap_or_default(),
            context_window: std::env::var("OPENAI_CONTEXT_WINDOW")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            prompt_cache_key: !matches!(
                std::env::var("OPENAI_PROMPT_CACHE_KEY").as_deref(),
                Ok("0") | Ok("false") | Ok("off")
            ),
            supports_reasoning: !matches!(
                std::env::var("OPENAI_SUPPORTS_REASONING").as_deref(),
                Ok("0") | Ok("false") | Ok("off")
            ),
        }
    }
}

/// Provider profiles: per-profile credentials, routing, and the labels
/// the picker rows show (gh #31).
pub mod profiles;

/// OpenAI's `prompt_cache_key` is capped at 64 characters (V1, ADR-0031).
fn clamp_cache_key(key: &str) -> String {
    key.chars().take(64).collect()
}

/// Whether an HTTP status is worth retrying (FR-CORE-6).
pub fn classify_status(status: u16) -> bool {
    status == 429 || status == 408 || (500..=599).contains(&status)
}

fn class_for_status(status: u16) -> &'static str {
    match status {
        401 | 403 => "auth",
        400..=499 => "invalid",
        _ => "transport",
    }
}

/// How a shared stream run failed, in the vocabulary the core's
/// `ProviderError` speaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFailure {
    /// Human-readable message.
    pub message: String,
    /// Class for the headless envelope (`docs/headless.md`).
    pub class: &'static str,
    /// Whether a retry could help (FR-CORE-6).
    pub retryable: bool,
}

impl From<lca_protocol::CapabilityError> for StreamFailure {
    fn from(err: lca_protocol::CapabilityError) -> Self {
        use lca_protocol::CapabilityError as E;
        // A refusal is a configuration problem, not a flaky network:
        // never retried, and its class keeps it out of "transport".
        let (class, retryable) = match &err {
            E::Permission(_) | E::NotGranted(_) | E::NotFound(_) | E::Invalid(_) => {
                ("invalid", false)
            }
            E::Io(_) | E::Timeout(_) => ("transport", true),
        };
        StreamFailure {
            message: err.to_string(),
            class,
            retryable,
        }
    }
}

/// Map the resolved message list onto the OpenAI chat shape. Reasoning
/// blocks are model-internal and are not resent. Text and image blocks both
/// map: text becomes the string content, and a message with an image becomes
/// the content-part array OpenAI uses for vision (the "cannot carry images"
/// case is the text stub the attach path already put in the message).
fn to_wire(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|message| {
            let role = match message.role {
                MessageRole::System => "system",
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::Tool => "tool",
            };
            let text: String = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    ContentBlock::Reasoning { .. } => None,
                    ContentBlock::ToolCall { .. } => None,
                    ContentBlock::Image { .. } => None,
                })
                .collect();
            let mut wire = serde_json::json!({ "role": role, "content": text });
            if message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. }))
            {
                let mut parts = Vec::new();
                if !text.is_empty() {
                    parts.push(serde_json::json!({ "type": "text", "text": text }));
                }
                for block in &message.content {
                    if let ContentBlock::Image { media_type, bytes } = block {
                        parts.push(serde_json::json!({
                            "type": "image_url",
                            "image_url": {
                                "url": format!(
                                    "data:{media_type};base64,{}",
                                    lca_protocol::base64_encode(bytes)
                                ),
                            },
                        }));
                    }
                }
                wire["content"] = serde_json::Value::Array(parts);
            }
            if !message.tool_calls.is_empty() {
                let calls: Vec<serde_json::Value> = message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        serde_json::json!({
                            "id": call.call_id,
                            "type": "function",
                            "function": { "name": call.name, "arguments": call.arguments },
                        })
                    })
                    .collect();
                wire["tool_calls"] = serde_json::Value::Array(calls);
            }
            if let Some(call_id) = &message.tool_call_id {
                wire["tool_call_id"] = serde_json::Value::String(call_id.clone());
            }
            wire
        })
        .collect()
}

fn tools_wire(tools: &[ToolSpec]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            })
        })
        .collect()
}

mod sse;
pub use sse::{SseDecoder, parse_sse};

/// A pull-based driver over one streaming completion: [`next_event`] returns
/// one typed event at a time, reading more of the response body only when its
/// buffer is empty. The native form drains it in a loop; the WASM form drives
/// it from the `completion-stream` resource's `next`, so a sandboxed provider
/// streams chunk by chunk instead of buffering the whole response
/// (`docs/deferred_workplan.md` C1).
///
/// [`next_event`]: StreamDriver::next_event
pub struct StreamDriver<'a, C: ProviderCap + ?Sized> {
    cap: &'a C,
    handle: u32,
    decoder: SseDecoder,
    pending: std::collections::VecDeque<StreamEvent>,
    finished: bool,
}

impl<'a, C: ProviderCap + ?Sized> StreamDriver<'a, C> {
    /// Build the request, send it, and check the status. A non-2xx response
    /// is read (bounded) and reported as a [`StreamFailure`].
    pub fn open(
        cap: &'a C,
        settings: &Settings,
        request: &CompletionRequest,
    ) -> Result<StreamDriver<'a, C>, StreamFailure> {
        let model = if request.model.is_empty() {
            settings.model.clone()
        } else {
            request.model.clone()
        };
        // Routing follows the model (gh #31): the entry the `models`
        // setting carries for this id names the profile whose credentials
        // the request is built from; an id the list does not know is a
        // default-profile request.
        let entries = profiles::model_entries(&cap.credentials_get("models").unwrap_or_default());
        let profile = profiles::profile_of_model(&entries, &model);
        let base = profiles::base_url_for(cap, settings, &profile);
        let url = format!("{}/chat/completions", base.trim_end_matches('/'));
        // Issue #3: no implicit default model. A call with nothing selected is
        // a legible error pointing at /model, never a silent "gpt-4o-mini".
        if model.is_empty() {
            return Err(StreamFailure {
                message: "this endpoint has no model selected; run /model to pick one".to_string(),
                class: "invalid",
                retryable: false,
            });
        }
        let mut body = serde_json::json!({
            "model": model,
            "messages": to_wire(&request.messages),
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        // gh #169: the completion capability's requests carry an explicit
        // generation budget (`extras["max-tokens"]`); without it the
        // endpoint's default decides when a long summarization is cut and
        // no failure can name the number. Ordinary turns send no budget
        // and their body is unchanged.
        let max_tokens = request
            .extras
            .get("max-tokens")
            .and_then(|value| value.parse::<u64>().ok());
        if let Some(budget) = max_tokens {
            body["max_tokens"] = serde_json::json!(budget);
        }
        let tools = tools_wire(&request.tools);
        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools);
        }
        // V1 (ADR-0031): OpenAI's cache-affinity pin, the general "keep our
        // cache" field for OpenAI-shaped endpoints. Clamped to 64 chars;
        // off only when a preset/opt-out says so. Added before the body is
        // serialized.
        if let Some(session) = request.extras.get("session-id")
            && settings.prompt_cache_key
        {
            body["prompt_cache_key"] = serde_json::Value::String(clamp_cache_key(session));
        }
        // R1: the session's thinking level rides the request extras as
        // `reasoning-effort`; map it to the endpoint's reasoning parameter
        // when the endpoint has one, and ignore it otherwise (a hint,
        // honored where meaningful, never an error).
        if let Some(effort) = request.extras.get("reasoning-effort")
            && settings.supports_reasoning
        {
            body["reasoning_effort"] = serde_json::Value::String(effort.clone());
        }
        let body_bytes = serde_json::to_vec(&body).map_err(|err| StreamFailure {
            message: format!("cannot build request: {err}"),
            class: "invalid",
            retryable: false,
        })?;
        let mut headers: Vec<(&str, &str)> = vec![("content-type", "application/json")];
        // ADR-0023: OpenCode Go refuses requests without a per-conversation
        // routing header (verified against the live endpoint); unknown
        // headers are ignored by every other OpenAI-shaped server.
        if let Some(session) = request.extras.get("session-id") {
            headers.push(("x-opencode-session", session.as_str()));
        }
        let key = profiles::api_key_for(cap, settings, &profile);
        let bearer;
        if let Some(key) = key.as_deref() {
            bearer = format!("Bearer {key}");
            headers.push(("authorization", bearer.as_str()));
        }
        let handle = cap.net_request("POST", &url, &headers, Some(&body_bytes))?;
        let status = cap.net_response_status(handle)?;
        if !(200..300).contains(&status) {
            let mut detail = Vec::new();
            while let Some(chunk) = cap.net_read_body(handle, 64 * 1024)? {
                detail.extend_from_slice(&chunk);
                if detail.len() > 1024 * 1024 {
                    break;
                }
            }
            let _ = cap.net_close_response(handle);
            let text = String::from_utf8_lossy(&detail);
            let message = error_message(&text, status);
            // Issue #3: a model the endpoint does not offer reads as a legible
            // "pick another with /model", not a bare HTTP 400.
            let lower = message.to_lowercase();
            let model_problem = (status == 400 || status == 404)
                && lower.contains("model")
                && (lower.contains("unavailable")
                    || lower.contains("not found")
                    || lower.contains("does not exist")
                    || lower.contains("invalid")
                    || lower.contains("no such"));
            let message = if model_problem {
                format!(
                    "this model isn't available on this endpoint; run /model to pick another ({message})"
                )
            } else {
                format!("provider returned HTTP {status}: {message}")
            };
            return Err(StreamFailure {
                message,
                class: class_for_status(status),
                retryable: classify_status(status),
            });
        }
        Ok(StreamDriver {
            cap,
            handle,
            decoder: SseDecoder::default().with_max_tokens(max_tokens),
            pending: std::collections::VecDeque::new(),
            finished: false,
        })
    }

    /// The next typed event, reading more of the body when the buffer is
    /// empty; `None` at end of stream.
    pub fn next_event(&mut self) -> Option<Result<StreamEvent, StreamFailure>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            if self.finished {
                return None;
            }
            match self.cap.net_read_body(self.handle, 64 * 1024) {
                Ok(Some(chunk)) => {
                    let mut events = Vec::new();
                    self.decoder.feed(&chunk, &mut |event| events.push(event));
                    self.pending.extend(events);
                }
                Ok(None) => {
                    self.finished = true;
                    let mut events = Vec::new();
                    self.decoder.finish(&mut |event| events.push(event));
                    self.pending.extend(events);
                }
                Err(err) => {
                    self.finished = true;
                    return Some(Err(StreamFailure::from(err)));
                }
            }
        }
    }
}

impl<C: ProviderCap + ?Sized> Drop for StreamDriver<'_, C> {
    fn drop(&mut self) {
        let _ = self.cap.net_close_response(self.handle);
    }
}

/// The whole completion call, over capabilities only. `emit` returning
/// `false` stops the read (the receiver went away, FR-CONC-3).
pub fn run_provider_stream(
    cap: &dyn ProviderCap,
    settings: &Settings,
    request: &CompletionRequest,
    emit: &mut dyn FnMut(StreamEvent) -> bool,
) -> Result<(), StreamFailure> {
    let mut driver = StreamDriver::open(cap, settings, request)?;
    while let Some(event) = driver.next_event() {
        if !emit(event?) {
            break;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Identity (ADR-0012): login promotes the environment's key into the
// credential store, logout clears it, usage is genuinely unsupported.
// ---------------------------------------------------------------------------

/// `login`: a stored key already counts as logged in; otherwise the
/// environment's key is promoted into this provider's namespace, with a
/// non-default base URL saved alongside it. The interactive modal that
/// asks for both arrives with the `ui` world (Phase 6); until then a
/// user with no environment key gets a message that says exactly that.
pub fn run_login(cap: &dyn ProviderCap, settings: &Settings) -> IdentityOutcome {
    if cap
        .credentials_get("api_key")
        .is_some_and(|key| !key.is_empty())
    {
        return IdentityOutcome::Ok;
    }
    let Some(key) = settings.api_key.clone() else {
        return IdentityOutcome::Failed(
            "no API key is configured. Set OPENAI_API_KEY (or OPENCODE_API_KEY) \
             and run /login again."
                .to_string(),
        );
    };
    if let Err(err) = cap.credentials_set("api_key", &key) {
        return IdentityOutcome::Failed(format!("cannot store the key: {err}"));
    }
    if settings.base_url != "https://api.openai.com/v1"
        && let Err(err) = cap.credentials_set("base_url", &settings.base_url)
    {
        return IdentityOutcome::Failed(format!("cannot store the base URL: {err}"));
    }
    IdentityOutcome::Ok
}

/// `logout`: clears the stored key and endpoint (the ad hoc `net` grant
/// survives, per `docs/providers/openai-compatible.md`).
pub fn run_logout(cap: &dyn ProviderCap) -> IdentityOutcome {
    for key in ["api_key", "base_url"] {
        // A missing key is already the desired end state.
        if let Err(err) = cap.credentials_delete(key)
            && !matches!(err, lca_protocol::CapabilityError::NotFound(_))
        {
            return IdentityOutcome::Failed(format!("cannot clear {key}: {err}"));
        }
    }
    IdentityOutcome::Ok
}

// ---------------------------------------------------------------------------
// Login presets (ADR-0031/0033): the extension's own data
// ---------------------------------------------------------------------------

/// One parsed provider preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    /// Stable id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Base URL including the version prefix.
    pub base_url: String,
    /// Environment variables that can supply the key.
    pub env: Vec<String>,
    /// `bearer` or `none`.
    pub auth: String,
    /// A curated model list (a fallback for `GET /models`).
    pub models: Vec<String>,
    /// Whether the endpoint honors a reasoning parameter (R1). Extension
    /// data, like everything in `provider-presets.toml`; `complete` also
    /// gates on the `OPENAI_SUPPORTS_REASONING` setting because it reads
    /// its endpoint from the environment, not from the picker.
    pub supports_reasoning: bool,
}

/// Parse the preset resource.
pub fn parse_presets(text: &str) -> Vec<Preset> {
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };
    value
        .get("preset")
        .and_then(|presets| presets.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(Preset {
                        id: item.get("id")?.as_str()?.to_string(),
                        name: item.get("name")?.as_str()?.to_string(),
                        base_url: item.get("base_url")?.as_str()?.to_string(),
                        env: string_list(item.get("env")),
                        auth: item
                            .get("auth")
                            .and_then(|a| a.as_str())
                            .unwrap_or("bearer")
                            .to_string(),
                        models: string_list(item.get("models")),
                        supports_reasoning: item
                            .get("supports_reasoning")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_list(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The extension's own `resources/` bag, compiled in (ADR-0032): the same
/// bytes an installed package serves from its directory, so both delivery
/// modes answer the picker query identically.
pub const RESOURCES: &[(&str, &[u8])] = &[
    (
        "provider-presets.toml",
        include_bytes!("../resources/provider-presets.toml"),
    ),
    (
        "context-windows.toml",
        include_bytes!("../resources/context-windows.toml"),
    ),
    (
        "image-limits.toml",
        include_bytes!("../resources/image-limits.toml"),
    ),
];

/// The native handle's resource source. The host sets this on the
/// capability engine so `resource_read` resolves against the compiled-in
/// bag instead of finding nothing.
#[cfg(not(target_arch = "wasm32"))]
pub fn resources() -> lca_tools::ResourceSource {
    lca_tools::ResourceSource::Embedded(RESOURCES)
}

/// Load the extension's presets from its own `resources/` bag.
pub fn load_presets(cap: &dyn ProviderCap) -> Vec<Preset> {
    match cap.resource_read("provider-presets.toml") {
        Ok(bytes) => parse_presets(&String::from_utf8_lossy(&bytes)),
        Err(_) => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Context windows (gh #34): the denominator `ctx` and the compaction
// threshold divide by
// ---------------------------------------------------------------------------

/// One model's image behavior from `image-limits.toml` (#39): the vision
/// flag pi's `input` modalities carry, plus an optional resize profile.
/// No vendor publishes resize numbers today, so profiles are `None`
/// until one does — vision models without a profile keep pi's
/// conservative defaults downstream.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageLimitEntry {
    /// Whether the model takes image input.
    pub vision: bool,
    /// A vendor-sourced resize profile, when one exists.
    pub resize: Option<lca_protocol::ImageResize>,
}

/// Parse the curated image-limits table (`[id]` tables with `vision` and
/// optional `max_width`/`max_height`/`max_bytes`). Entries without a
/// boolean `vision` are ignored; unknown keys are ignored, so resize
/// fields can join later without disturbing vision reads.
pub fn parse_image_limits(text: &str) -> BTreeMap<String, ImageLimitEntry> {
    let Ok(value) = text.parse::<toml::Value>() else {
        return BTreeMap::new();
    };
    let Some(table) = value.as_table() else {
        return BTreeMap::new();
    };
    table
        .iter()
        .filter_map(|(model, entry)| {
            let entry = entry.as_table()?;
            let vision = entry.get("vision")?.as_bool()?;
            let resize = (|| {
                let width = entry.get("max_width")?.as_integer()?;
                let height = entry.get("max_height")?.as_integer()?;
                let bytes = entry.get("max_bytes")?.as_integer()?;
                Some(lca_protocol::ImageResize {
                    max_width: u32::try_from(width).ok()?,
                    max_height: u32::try_from(height).ok()?,
                    max_bytes: usize::try_from(bytes).ok()?,
                })
            })();
            Some((model.clone(), ImageLimitEntry { vision, resize }))
        })
        .collect()
}

/// Load the image limits from the extension's own `resources/` bag (#39).
/// Missing bag reads as empty: every model is unknown vision and images
/// pass through as before.
pub fn load_image_limits(cap: &dyn ProviderCap) -> BTreeMap<String, ImageLimitEntry> {
    match cap.resource_read("image-limits.toml") {
        Ok(bytes) => parse_image_limits(&String::from_utf8_lossy(&bytes)),
        Err(_) => BTreeMap::new(),
    }
}

/// The `ModelInfo` extras carrying one model's image behavior (#39):
/// `image.vision` is `"true"`/`"false"`, and `image.resize` rides only
/// when a vendor-sourced profile exists. The host reads both through
/// `ImagePolicy::for_extras` — no ABI change for non-structural data.
pub fn image_extras(
    model: &str,
    limits: &BTreeMap<String, ImageLimitEntry>,
) -> Vec<(String, String)> {
    let Some(entry) = limits.get(model) else {
        return Vec::new();
    };
    let mut extras = vec![(
        lca_protocol::IMAGE_VISION_EXTRA.to_string(),
        entry.vision.to_string(),
    )];
    if let Some(resize) = &entry.resize {
        extras.push((
            lca_protocol::IMAGE_RESIZE_EXTRA.to_string(),
            format!(
                "{}x{}:{}",
                resize.max_width, resize.max_height, resize.max_bytes
            ),
        ));
    }
    extras
}

/// Parse the curated per-model window table (`id = tokens`).
pub fn parse_context_windows(text: &str) -> BTreeMap<String, u32> {
    let Ok(value) = text.parse::<toml::Value>() else {
        return BTreeMap::new();
    };
    value
        .as_table()
        .map(|table| {
            table
                .iter()
                .filter_map(|(model, tokens)| {
                    let tokens = tokens.as_integer().and_then(|n| u32::try_from(n).ok())?;
                    Some((model.clone(), tokens))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The curated windows from the extension's own `resources/` bag (gh #34).
/// A package installed before this resource existed, or a caller that
/// injects only the presets, reads as an empty table: every model then
/// reports no window and the footer keeps `ctx ?` - an honest unknown,
/// never an invented number (FR-SESS-4 divides by this value, so a wrong
/// one silently mis-triggers compaction).
pub fn load_context_windows(cap: &dyn ProviderCap) -> BTreeMap<String, u32> {
    match cap.resource_read("context-windows.toml") {
        Ok(bytes) => parse_context_windows(&String::from_utf8_lossy(&bytes)),
        Err(_) => BTreeMap::new(),
    }
}

/// One model's context window, in precedence order (ADR-0035's settings
/// chain): `OPENAI_CONTEXT_WINDOW` overrides the whole provider, then the
/// endpoint's own `context_length` if it reported one, then the curated
/// catalog, then 0 - which the footer renders `ctx ?` instead of a
/// percentage over a denominator nobody verified.
pub fn context_window_for(
    model: &str,
    env_override: u32,
    endpoint_value: Option<u32>,
    curated: &BTreeMap<String, u32>,
) -> u32 {
    if env_override > 0 {
        return env_override;
    }
    endpoint_value
        .filter(|tokens| *tokens > 0)
        .or_else(|| curated.get(model).copied())
        .unwrap_or(0)
}

/// Split a `models` setting into `(id, window)` pairs (gh #34). Every list
/// written before gh #34 is bare ids, and each reads as "no window known",
/// so the shape is backward-tolerant by construction. An id containing
/// `=` is only split when the tail is a number, so an id that happens to
/// carry one still arrives whole.
pub fn parse_models_setting(value: &str) -> Vec<(String, Option<u32>)> {
    // One parser for the setting (gh #34's `id=window`, gh #31's profile
    // tag): `profiles::model_entries` owns the grammar, this stays as the
    // id/window view callers outside the picker read.
    profiles::model_entries(value)
        .into_iter()
        .map(|entry| (entry.id, entry.window))
        .collect()
}

/// The host's picker options (ADR-0033), one per preset.
pub fn login_options(cap: &dyn ProviderCap) -> Vec<lca_protocol::LoginOption> {
    load_presets(cap)
        .into_iter()
        .map(|preset| {
            let mut extras = std::collections::BTreeMap::new();
            extras.insert("base_url".to_string(), preset.base_url.clone());
            extras.insert("auth".to_string(), preset.auth.clone());
            if !preset.models.is_empty() {
                extras.insert("models".to_string(), preset.models.join(","));
            }
            lca_protocol::LoginOption {
                id: preset.id,
                name: preset.name,
                kind: "api-key".to_string(),
                host: host_of(&preset.base_url),
                fields: if preset.auth == "none" {
                    Vec::new()
                } else {
                    vec!["api-key".to_string()]
                },
                extras,
            }
        })
        .collect()
}

/// The host portion of a base URL (for the ad hoc `net` consent).
pub(crate) fn host_of(base_url: &str) -> String {
    let without_scheme = base_url.split("://").nth(1).unwrap_or(base_url);
    without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .split(':')
        .next()
        .unwrap_or(without_scheme)
        .to_string()
}

/// D2: ask the endpoint for its model list, with each model's own context
/// limit when the response carries one (gh #34). `None` on any failure - a
/// wrong key, a down endpoint, or a host with no `net` grant yet (the ad
/// hoc grant is offered *after* submit, so a first login often cannot
/// reach the network here) - and the caller falls back to the preset's
/// curated short list.
pub(crate) fn discover_models(
    cap: &dyn ProviderCap,
    base_url: &str,
    key: Option<&str>,
) -> Option<Vec<(String, Option<u32>)>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut headers = vec![("accept", "application/json")];
    let auth = key.map(|key| format!("Bearer {key}"));
    if let Some(auth) = &auth {
        headers.push(("authorization", auth.as_str()));
    }
    let handle = cap.net_request("GET", &url, &headers, None).ok()?;
    let status = cap.net_response_status(handle).ok();
    let mut body = Vec::new();
    // One megabyte and one hundred chunks: discovery is a convenience, and
    // a response that never ends must not pin the login flow.
    for _ in 0..100 {
        match cap.net_read_body(handle, 64 * 1024) {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() > 1024 * 1024 {
                    break;
                }
            }
            _ => break,
        }
    }
    let _ = cap.net_close_response(handle);
    if !matches!(status, Some(200..=299)) {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&body).ok()?;
    let mut found: Vec<(String, Option<u32>)> = json
        .get("data")?
        .as_array()?
        .iter()
        .filter_map(|entry| {
            let model = entry.get("id")?.as_str()?.to_string();
            // The one limit field an OpenAI-shaped `GET /models` actually
            // carries: OpenRouter's answer reads `context_length`
            // (`openai/gpt-4o` -> 128000, checked live 2026-10-03, and
            // matching the catalog below), which is the field pi's own
            // catalog generator reads. The endpoint gh #34 reports
            // against sends no limit at all - id/object/created/owned_by
            // only - so absence is the normal case and reads as "no
            // window", never as a guess (gh #34's ground-truth rule).
            let window = entry
                .get("context_length")
                .and_then(|value| value.as_u64())
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0);
            Some((model, window))
        })
        .collect();
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found.dedup_by(|left, right| left.0 == right.0);
    (!found.is_empty()).then_some(found)
}

/// Consume one login answer (ADR-0033): store the secret in the extension's
/// own credentials namespace, return opaque settings for the host.
pub fn login_submit(
    cap: &dyn ProviderCap,
    answer: &lca_protocol::LoginAnswer,
) -> Result<Vec<(String, String)>, String> {
    let presets = load_presets(cap);
    let preset = presets.iter().find(|preset| preset.id == answer.choice);
    let base_url = answer
        .value("base-url")
        .map(str::to_string)
        .or_else(|| preset.map(|preset| preset.base_url.clone()))
        .ok_or_else(|| format!("unknown preset `{}`", answer.choice))?;
    // The choice's id is this login's profile (gh #31): a second login
    // adds its own keys instead of overwriting the first, and a login for
    // a profile that exists updates that one profile only. The keys live
    // in this extension's own namespace, under `profile.<id>.`.
    let profile = answer.choice.clone();
    if let Some(key) = answer.value("api-key")
        && !key.is_empty()
    {
        cap.credentials_set(
            &profiles::credential_key(&Some(profile.clone()), "api_key"),
            key,
        )
        .map_err(|err| format!("cannot store the key: {err}"))?;
    }
    // The base URL rides back as an opaque pair the host persists under
    // the same profile key - the host never parses it (ADR-0031).
    let mut settings = vec![(
        profiles::credential_key(&Some(profile.clone()), "base_url"),
        base_url.clone(),
    )];
    // D2: the endpoint's own model list when it answers, the preset's
    // curated short list otherwise. The host persists whatever comes back
    // and never interprets it; each entry carries its profile id and, when
    // the endpoint reported one, its window (`id@profile=window`, which
    // parses exactly as `id=window` did - gh #34, gh #31).
    let discovered =
        discover_models(cap, &base_url, answer.value("api-key")).unwrap_or_else(|| {
            preset
                .map(|preset| {
                    preset
                        .models
                        .iter()
                        .map(|model| (model.clone(), None))
                        .collect()
                })
                .unwrap_or_default()
        });
    // Merge into the list already stored, so logging into a second
    // service keeps the first one's models instead of replacing them.
    let mut entries = profiles::model_entries(&cap.credentials_get("models").unwrap_or_default());
    for (model, window) in discovered {
        entries.retain(|entry| {
            !(entry.id == model && entry.profile.as_deref() == Some(profile.as_str()))
        });
        entries.push(profiles::ModelEntry {
            id: model,
            profile: Some(profile.clone()),
            window,
        });
    }
    if !entries.is_empty() {
        settings.push(("models".to_string(), profiles::serialize_entries(&entries)));
    }
    if let Some(model) = answer.value("model") {
        settings.push(("model".to_string(), model.to_string()));
    }
    Ok(settings)
}

// ---------------------------------------------------------------------------
// Native delivery mode: the dispatch handle the registry holds
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::OpenAiCompat;

// ---------------------------------------------------------------------------
// WASM delivery mode: the same logic behind the provider world's imports
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode;

/// The most useful message an error body carries.
///
/// OpenAI's shape is `{"error":{"message":...}}`, but gateways differ:
/// some send `{"error":"..."}`, a bare `{"message":...}`, or
/// `{"type":"..."}`; and some send a body with no message at all (the live
/// OpenCode Go rejection of a dangling tool call is
/// `{"model":"deepseek-v4.1-flash"}`). Fall back to a short raw body so a
/// failure is never reported as just "unknown error".
fn error_message(body: &str, status: u16) -> String {
    let text = body.trim();
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
        for pointer in ["/error/message", "/error", "/message", "/detail", "/type"] {
            if let Some(value) = json.pointer(pointer).and_then(|value| value.as_str())
                && !value.trim().is_empty()
            {
                return value.trim().to_string();
            }
        }
    }
    if !text.is_empty() && text.len() <= 300 {
        return text.to_string();
    }
    format!("HTTP {status}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every error-body shape yields a legible message; a body with no
    // message field falls back to the raw text, never "unknown error".
    #[test]
    fn error_message_reads_every_shape() {
        assert_eq!(
            error_message(r#"{"error":{"message":"bad key"}}"#, 401),
            "bad key"
        );
        assert_eq!(error_message(r#"{"error":"plain"}"#, 400), "plain");
        assert_eq!(error_message(r#"{"message":"top"}"#, 500), "top");
        assert_eq!(
            error_message(r#"{"type":"MissingSessionID"}"#, 400),
            "MissingSessionID"
        );
        // The live dangling-call 400 has no message field.
        assert_eq!(
            error_message(r#"{"model":"deepseek-v4.1-flash"}"#, 400),
            r#"{"model":"deepseek-v4.1-flash"}"#
        );
        assert_eq!(error_message("", 503), "HTTP 503");
    }

    // Verifies: ADR-0029 - a message with an image maps to the OpenAI
    // content-part array with a base64 data URI; a text-only message keeps
    // the plain string content (no behavior change for the common case).
    #[test]
    fn an_image_maps_to_the_vision_content_array() {
        let message = ChatMessage {
            role: MessageRole::User,
            content: vec![
                ContentBlock::Text {
                    text: "look".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    bytes: vec![1, 2, 3],
                },
            ],
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        };
        let wire = to_wire(&[message]);
        let parts = wire[0]["content"].as_array().expect("array content");
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,AQID");

        let text_only = to_wire(&[ChatMessage::text(MessageRole::User, "hi")]);
        assert_eq!(
            text_only[0]["content"], "hi",
            "no image keeps the string form"
        );
    }

    #[test]
    fn a_body_with_no_sse_frames_reports_an_error() {
        let mut events = Vec::new();
        parse_sse(b"this is not SSE at all\n", &mut |event| events.push(event));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, StreamEvent::Error { .. })),
            "a non-SSE body must surface, not read as an empty success: {events:?}"
        );
    }

    #[test]
    fn a_valid_stream_with_no_content_is_not_an_error() {
        let mut events = Vec::new();
        parse_sse(
            b"data: {\"choices\":[{\"delta\":{}}]}\n\ndata: [DONE]\n\n",
            &mut |event| events.push(event),
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, StreamEvent::Error { .. })),
            "an empty but well-formed stream is a valid empty answer: {events:?}"
        );
    }
}
