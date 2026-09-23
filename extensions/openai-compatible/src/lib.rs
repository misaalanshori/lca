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
                .unwrap_or_else(|_| "gpt-4o-mini".to_string()),
        }
    }
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
/// blocks are model-internal and are not resent.
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
                })
                .collect();
            let mut wire = serde_json::json!({ "role": role, "content": text });
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

/// Streaming response decoder: feed it bytes, it emits typed events.
#[derive(Default)]
pub struct SseDecoder {
    buffer: String,
    open_calls: Vec<(usize, String, String)>, // (index, call_id, name)
    finished: bool,
}

impl SseDecoder {
    /// Feed a chunk of response bytes.
    pub fn feed(&mut self, chunk: &[u8], emit: &mut dyn FnMut(StreamEvent)) {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        while let Some(end) = self.buffer.find("\n\n") {
            let event: String = self.buffer.drain(..end + 2).collect();
            self.handle_event(&event, emit);
        }
    }

    /// Flush any trailing event that was not newline-terminated.
    pub fn finish(&mut self, emit: &mut dyn FnMut(StreamEvent)) {
        if !self.buffer.is_empty() {
            let rest = std::mem::take(&mut self.buffer);
            self.handle_event(&rest, emit);
        }
        if !self.finished {
            self.finished = true;
            // Close any call the server forgot to close with a finish_reason:
            // the host's accumulator then sees a normal end, and a stream
            // that truly ended open is reported there instead.
            for (_, call_id, _) in std::mem::take(&mut self.open_calls) {
                emit(StreamEvent::ToolCallEnd { call_id });
            }
        }
    }

    fn handle_event(&mut self, raw: &str, emit: &mut dyn FnMut(StreamEvent)) {
        for line in raw.lines() {
            let line = line.trim_end_matches('\r');
            let Some(payload) = line.strip_prefix("data:") else {
                continue; // comments and keep-alives
            };
            let payload = payload.trim();
            if payload.is_empty() {
                continue;
            }
            if payload == "[DONE]" {
                for (_, call_id, _) in std::mem::take(&mut self.open_calls) {
                    emit(StreamEvent::ToolCallEnd { call_id });
                }
                self.finished = true;
                return;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue; // a malformed frame drops; the stream continues
            };
            self.handle_value(&value, emit);
        }
    }

    fn handle_value(&mut self, value: &serde_json::Value, emit: &mut dyn FnMut(StreamEvent)) {
        if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
            emit(StreamEvent::Usage {
                usage: map_usage(usage),
            });
        }
        let Some(choice) = value.get("choices").and_then(|c| c.get(0)) else {
            return;
        };
        let delta = choice.get("delta").or_else(|| choice.get("message"));
        if let Some(delta) = delta {
            if let Some(text) = delta.get("content").and_then(|t| t.as_str())
                && !text.is_empty()
            {
                emit(StreamEvent::TextDelta {
                    delta: text.to_string(),
                });
            }
            let reasoning = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(|t| t.as_str());
            if let Some(reasoning) = reasoning
                && !reasoning.is_empty()
            {
                emit(StreamEvent::ReasoningDelta {
                    delta: reasoning.to_string(),
                });
            }
            if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                for call in calls {
                    let index = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    let id = call.get("id").and_then(|i| i.as_str());
                    let name = call
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str());
                    let args = call
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(|a| a.as_str())
                        .unwrap_or("");
                    if let (Some(id), Some(name)) = (id, name) {
                        // FR-PROV-7: the start event comes before any delta.
                        emit(StreamEvent::ToolCallStart {
                            call_id: id.to_string(),
                            name: name.to_string(),
                        });
                        self.open_calls.retain(|(i, _, _)| *i != index);
                        self.open_calls
                            .push((index, id.to_string(), name.to_string()));
                        if !args.is_empty() {
                            emit(StreamEvent::ToolCallArgDelta {
                                call_id: id.to_string(),
                                delta: args.to_string(),
                            });
                        }
                    } else {
                        // FR-PROV-8: only emit a delta for an open call.
                        if !args.is_empty()
                            && let Some((_, call_id, _)) =
                                self.open_calls.iter().find(|(i, _, _)| *i == index)
                        {
                            emit(StreamEvent::ToolCallArgDelta {
                                call_id: call_id.clone(),
                                delta: args.to_string(),
                            });
                        }
                    }
                }
            }
        }
        if choice.get("finish_reason").and_then(|f| f.as_str()) == Some("tool_calls") {
            for (_, call_id, _) in std::mem::take(&mut self.open_calls) {
                emit(StreamEvent::ToolCallEnd { call_id });
            }
        }
    }
}

fn map_usage(value: &serde_json::Value) -> Usage {
    let prompt = value
        .get("prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = value
        .get("completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read = value
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_write = value
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cache_creation_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    Usage {
        input: prompt.saturating_sub(cache_read),
        output,
        cache_read,
        cache_write,
        cache_write_1h: 0,
        // OpenAI-shaped endpoints report no pricing; cost stays 0 and
        // cache-waste dollar cost is 0 until a provider reports buckets.
        cost: 0.0,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: BTreeMap::new(),
    }
}

/// One-shot decode of a complete SSE body (tests and error paths).
pub fn parse_sse(body: &[u8], emit: &mut dyn FnMut(StreamEvent)) {
    let mut decoder = SseDecoder::default();
    decoder.feed(body, emit);
    decoder.finish(emit);
}

/// The effective base URL: a stored one (saved by `login`) wins over the
/// environment default, so the WASM form and the native form share one
/// configured endpoint through `credentials`.
fn effective_base_url(cap: &dyn ProviderCap, settings: &Settings) -> String {
    cap.credentials_get("base_url")
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| settings.base_url.clone())
}

/// The effective bearer token: stored key first, environment second.
fn effective_key(cap: &dyn ProviderCap, settings: &Settings) -> Option<String> {
    cap.credentials_get("api_key")
        .filter(|key| !key.is_empty())
        .or_else(|| settings.api_key.clone())
}

/// The whole completion call, over capabilities only. `emit` returning
/// `false` stops the read (the receiver went away, FR-CONC-3).
/// ponytail: chunks are decoded as they arrive, but a single call's
/// error body is read to its end first; an endpoint that stalls between
/// chunks holds the call until the capability's read timeout.
pub fn run_provider_stream(
    cap: &dyn ProviderCap,
    settings: &Settings,
    request: &CompletionRequest,
    emit: &mut dyn FnMut(StreamEvent) -> bool,
) -> Result<(), StreamFailure> {
    let base = effective_base_url(cap, settings);
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let mut body = serde_json::json!({
        "model": if request.model.is_empty() { settings.model.clone() } else { request.model.clone() },
        "messages": to_wire(&request.messages),
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    let tools = tools_wire(&request.tools);
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
    }
    let body_bytes = serde_json::to_vec(&body).map_err(|err| StreamFailure {
        message: format!("cannot build request: {err}"),
        class: "invalid",
        retryable: false,
    })?;
    let mut headers: Vec<(&str, &str)> = vec![("content-type", "application/json")];
    let key = effective_key(cap, settings);
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
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let message = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error")
            .to_string();
        return Err(StreamFailure {
            message: format!("provider returned HTTP {status}: {message}"),
            class: class_for_status(status),
            retryable: classify_status(status),
        });
    }

    let mut decoder = SseDecoder::default();
    let mut stopped = false;
    while let Some(chunk) = cap.net_read_body(handle, 64 * 1024)? {
        decoder.feed(&chunk, &mut |event| {
            if !emit(event) {
                stopped = true;
            }
        });
        if stopped {
            break;
        }
    }
    if !stopped {
        decoder.finish(&mut |event| {
            let _ = emit(event);
        });
    }
    let _ = cap.net_close_response(handle);
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
            "no key stored and neither OPENAI_API_KEY nor OPENCODE_API_KEY is set; \
             the interactive setup modal arrives with the ui world"
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
// Native delivery mode: the dispatch handle the registry holds
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::sync::Arc;

    use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
    use lca_protocol::{DispatchError, ModelInfo};

    /// The native handle: shared logic over the capability engine.
    pub struct OpenAiCompat {
        cap: Arc<lca_tools::Capabilities>,
        settings: Settings,
    }

    impl OpenAiCompat {
        /// Build from the engine the manifest's grants live in and the
        /// environment-derived settings.
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> OpenAiCompat {
            Self::with_settings(cap, Settings::default())
        }

        /// Build with explicit settings (tests and anything that
        /// resolves configuration outside the environment).
        pub fn with_settings(
            cap: Arc<lca_tools::Capabilities>,
            settings: Settings,
        ) -> OpenAiCompat {
            OpenAiCompat { cap, settings }
        }

        /// The engine, for tests that inspect recorded denials.
        pub fn capabilities(&self) -> &Arc<lca_tools::Capabilities> {
            &self.cap
        }
    }

    impl ExtensionDispatch for OpenAiCompat {
        fn name(&self) -> &str {
            "openai-compatible"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Provider, World::Command]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Err(DispatchError::MissingWorld {
                extension: "openai-compatible".to_string(),
                world: "tool",
            })
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "openai-compatible".to_string(),
                world: "tool",
            })))
        }

        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            // Declared per the provider template; the identity trio is
            // namespaced by the host (FR-PROV-10), and this provider
            // documents no extra commands of its own.
            Ok(Vec::new())
        }

        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }

        fn provider_models(&self) -> Result<Vec<ModelInfo>, DispatchError> {
            Ok(vec![ModelInfo {
                id: self.settings.model.clone(),
                name: self.settings.model.clone(),
                context_window: 0,
                max_tokens: 0,
            }])
        }

        fn stream_completion<'a>(
            &'a self,
            request: CompletionRequest,
            sink: &'a dyn lca_protocol::EventSink,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            let cap = self.cap.clone();
            let settings = self.settings.clone();
            Box::pin(async move {
                let result = lca_tools::bridge_stream(
                    move |bridge| {
                        run_provider_stream(cap.as_ref(), &settings, &request, &mut |event| {
                            bridge.push(event)
                        })
                    },
                    sink,
                )
                .await;
                match result {
                    Ok(()) => Ok(()),
                    Err(lca_tools::BridgeError::Work(failure)) => Err(DispatchError::Failed(
                        format!("openai-compatible: {}", failure.message),
                    )),
                    Err(lca_tools::BridgeError::Panicked) => Err(DispatchError::Failed(
                        "openai-compatible: the provider call panicked".to_string(),
                    )),
                }
            })
        }

        fn identity_login(
            &self,
        ) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
            let cap = self.cap.clone();
            let settings = self.settings.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || run_login(cap.as_ref(), &settings))
                    .await
                    .map_err(|_| DispatchError::Failed("openai-compatible: login panicked".into()))
            })
        }

        fn identity_logout(
            &self,
        ) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
            let cap = self.cap.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || run_logout(cap.as_ref()))
                    .await
                    .map_err(|_| DispatchError::Failed("openai-compatible: logout panicked".into()))
            })
        }

        fn identity_usage(
            &self,
        ) -> DispatchFuture<'static, Result<Result<Usage, IdentityOutcome>, DispatchError>>
        {
            // No usage endpoint can be assumed across arbitrary
            // OpenAI-compatible servers (ADR-0012's optional export).
            Box::pin(std::future::ready(Ok(Err(IdentityOutcome::NotSupported))))
        }

        fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }

        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::OpenAiCompat;

// ---------------------------------------------------------------------------
// WASM delivery mode: the same logic behind the provider world's imports
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    use super::*;
    use core::cell::RefCell;

    wit_bindgen::generate!({
        path: "../../wit",
        world: "provider",
        export_macro_name: "export_provider",
        with: {
            "lca:host/log@0.1.0": generate,
            "lca:host/net@0.1.0": generate,
            "lca:host/oauth@0.1.0": generate,
            "lca:host/credentials@0.1.0": generate,
        },
    });

    use exports::lca::ext::provider_completion::{
        CompletionStream, Guest as CompletionGuest, GuestCompletionStream, StreamEvent as WasmEvent,
    };
    use exports::lca::ext::provider_identity::{
        Guest as IdentityGuest, IdentityOutcome as WasmOutcome, TokenUsage,
    };
    use exports::lca::ext::provider_models::{Guest as ModelsGuest, ModelInfo as WasmModel};
    use lca::ext::types::{ExtraPair, Usage as WasmUsage};
    use lca::host::{credentials, net};

    use crate::{Settings, run_login, run_logout};

    fn map_net(err: net::Error) -> lca_protocol::CapabilityError {
        use lca_protocol::CapabilityError as E;
        match err {
            net::Error::Permission(d) => E::Permission(d),
            net::Error::NotGranted(d) => E::NotGranted(d),
            net::Error::Dns(d) => E::Io(d),
            net::Error::Tls(d) => E::Io(d),
            net::Error::Io(d) => E::Io(d),
            net::Error::Invalid(d) => E::Invalid(d),
        }
    }

    /// The guest's capability view: host imports, no sockets, no files.
    struct GuestCap;

    impl ProviderCap for GuestCap {
        fn net_request(
            &self,
            method: &str,
            url: &str,
            headers: &[(&str, &str)],
            body: Option<&[u8]>,
        ) -> Result<u32, lca_protocol::CapabilityError> {
            net::request(
                method,
                url,
                &headers
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect::<Vec<_>>(),
                body,
            )
            .map_err(map_net)
        }

        fn net_response_status(&self, handle: u32) -> Result<u16, lca_protocol::CapabilityError> {
            net::response_status(handle).map_err(map_net)
        }

        fn net_read_body(
            &self,
            handle: u32,
            max: usize,
        ) -> Result<Option<Vec<u8>>, lca_protocol::CapabilityError> {
            net::read_body(handle, max as u64).map_err(map_net)
        }

        fn net_close_response(&self, handle: u32) -> Result<(), lca_protocol::CapabilityError> {
            net::close_response(handle).map_err(map_net)
        }

        fn credentials_get(&self, key: &str) -> Option<String> {
            credentials::get(key)
        }

        fn credentials_set(
            &self,
            key: &str,
            value: &str,
        ) -> Result<(), lca_protocol::CapabilityError> {
            credentials::set(key, value).map_err(|err| match err {
                credentials::Error::Permission(d) => lca_protocol::CapabilityError::Permission(d),
                credentials::Error::NotGranted(d) => lca_protocol::CapabilityError::NotGranted(d),
                credentials::Error::Io(d) => lca_protocol::CapabilityError::Io(d),
                credentials::Error::Invalid(d) => lca_protocol::CapabilityError::Invalid(d),
            })
        }

        fn credentials_delete(&self, key: &str) -> Result<(), lca_protocol::CapabilityError> {
            credentials::delete(key).map_err(|err| match err {
                credentials::Error::Permission(d) => lca_protocol::CapabilityError::Permission(d),
                credentials::Error::NotGranted(d) => lca_protocol::CapabilityError::NotGranted(d),
                credentials::Error::Io(d) => lca_protocol::CapabilityError::Io(d),
                credentials::Error::Invalid(d) => lca_protocol::CapabilityError::Invalid(d),
            })
        }
    }

    fn to_wit_usage(usage: &Usage) -> WasmUsage {
        let mut extras: Vec<ExtraPair> = usage
            .extras
            .iter()
            .map(|(key, value)| ExtraPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect();
        for (key, value) in [
            ("cost_input", usage.cost_input),
            ("cost_cache_read", usage.cost_cache_read),
            ("cost_cache_write", usage.cost_cache_write),
        ] {
            if value != 0.0 {
                extras.push(ExtraPair {
                    key: key.to_string(),
                    value: value.to_string(),
                });
            }
        }
        WasmUsage {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cache_write_hour: usage.cache_write_1h,
            cost: usage.cost,
            extras,
        }
    }

    fn to_wit_event(event: StreamEvent) -> WasmEvent {
        use StreamEvent as P;
        match event {
            P::TextDelta { delta } => WasmEvent::TextDelta(delta),
            P::ReasoningDelta { delta } => WasmEvent::ReasoningDelta(delta),
            P::ToolCallStart { call_id, name } => WasmEvent::ToolCallStart((call_id, name)),
            P::ToolCallArgDelta { call_id, delta } => WasmEvent::ToolCallArgDelta((call_id, delta)),
            P::ToolCallEnd { call_id } => WasmEvent::ToolCallEnd(call_id),
            P::Usage { usage } => WasmEvent::Usage(to_wit_usage(&usage)),
            P::Error { message, retryable } => WasmEvent::Error((message, retryable)),
            P::VendorEvent { kind, payload } => WasmEvent::VendorEvent((kind, payload.to_string())),
        }
    }

    fn to_wit_outcome(outcome: IdentityOutcome) -> WasmOutcome {
        match outcome {
            IdentityOutcome::Ok => WasmOutcome::Ok,
            IdentityOutcome::NotSupported => WasmOutcome::NotSupported,
            IdentityOutcome::Failed(reason) => WasmOutcome::Failed(reason),
        }
    }

    /// The WIT request -> the protocol shape the shared logic expects.
    /// The host already converted on its side; this is the exact inverse
    /// (concatenated text becomes the message's text block; reasoning
    /// never crossed the boundary in the first place).
    fn from_wit_request(request: provider_completion::CompletionRequest) -> CompletionRequest {
        let messages = request
            .messages
            .iter()
            .map(|message| ChatMessage {
                role: match message.role.as_str() {
                    "system" => MessageRole::System,
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    _ => MessageRole::Tool,
                },
                content: if message.content.is_empty() {
                    Vec::new()
                } else {
                    vec![ContentBlock::Text {
                        text: message.content.clone(),
                    }]
                },
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(|call| lca_protocol::ToolCall {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    })
                    .collect(),
                tool_call_id: message.tool_call_id.clone(),
                // The WIT record carries no usage; the host's own copy
                // keeps it, this side never needs it.
                usage: None,
                extras: BTreeMap::new(),
            })
            .collect();
        let tools = request
            .tools
            .iter()
            .map(|tool| ToolSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: serde_json::from_str(&tool.parameters)
                    .unwrap_or_else(|_| serde_json::json!({ "type": "object" })),
                extras: tool
                    .extras
                    .iter()
                    .map(|pair| (pair.key.clone(), pair.value.clone()))
                    .collect(),
            })
            .collect();
        CompletionRequest {
            messages,
            tools,
            model: request.model,
            stable_prefix: request.stable_prefix as usize,
            extras: request
                .extras
                .iter()
                .map(|pair| (pair.key.clone(), pair.value.clone()))
                .collect(),
        }
    }

    use exports::lca::ext::provider_completion;

    pub struct OpenAiCompatWasm;

    pub struct ScriptedStream {
        events: RefCell<std::vec::IntoIter<StreamEvent>>,
    }

    impl GuestCompletionStream for ScriptedStream {
        fn next(&self) -> Option<WasmEvent> {
            self.events.borrow_mut().next().map(to_wit_event)
        }
    }

    impl ModelsGuest for OpenAiCompatWasm {
        fn list_models() -> Vec<WasmModel> {
            let settings = Settings::default();
            vec![WasmModel {
                id: settings.model.clone(),
                name: settings.model,
                context_window: 0,
                max_tokens: 0,
                extras: Vec::new(),
            }]
        }
    }

    impl CompletionGuest for OpenAiCompatWasm {
        type CompletionStream = ScriptedStream;

        fn stream_completion(
            request: provider_completion::CompletionRequest,
        ) -> Result<CompletionStream, String> {
            let protocol_request = from_wit_request(request);
            // ponytail: the sandboxed form fetches the whole response
            // before handing events out (the resource is precomputed);
            // the native form streams chunk by chunk. Same events, same
            // order (NFR-25's property applies beyond the conformance
            // extension's own scripts).
            let mut events: Vec<StreamEvent> = Vec::new();
            crate::run_provider_stream(
                &GuestCap,
                &Settings::default(),
                &protocol_request,
                &mut |event| {
                    events.push(event);
                    true
                },
            )
            .map_err(|failure| failure.message)?;
            Ok(CompletionStream::new(ScriptedStream {
                events: RefCell::new(events.into_iter()),
            }))
        }
    }

    impl IdentityGuest for OpenAiCompatWasm {
        fn login() -> WasmOutcome {
            to_wit_outcome(run_login(&GuestCap, &Settings::default()))
        }

        fn logout() -> WasmOutcome {
            to_wit_outcome(run_logout(&GuestCap))
        }

        fn usage() -> Result<TokenUsage, WasmOutcome> {
            Err(WasmOutcome::NotSupported)
        }
    }

    export_provider!(OpenAiCompatWasm);
}
