//! Any OpenAI-compatible chat completions endpoint (the default provider,
//! native-linked by default per ADR-0013).
//!
//! Phase 1 shape: a plain Rust implementation of the `Provider` trait.
//! Phase 3 ports this source to the `provider` world under the same
//! directory (`docs/providers/openai-compatible.md`).
//!
//! The cache-boundary hint is advisory and intentionally unused for marker
//! placement: OpenAI-shaped endpoints cache automatically. `cache_read` and
//! `cache_write` are still reported from `cached_tokens`, which is what the
//! cache-waste measurement consumes.

#![deny(unsafe_code)]

use std::collections::BTreeMap;

use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use lca_protocol::{ChatMessage, ContentBlock, MessageRole, StreamEvent, ToolSpec, Usage};
use lca_provider::{BoxFuture, CompletionRequest, EventSender, ModelInfo, Provider, ProviderError};

/// Configuration, normally from the environment (`docs/providers/openai-compatible.md`:
/// bearer token from the environment when the credential store has none).
#[derive(Debug, Clone)]
pub struct Settings {
    /// Base URL including the version prefix, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    /// Bearer token, when the endpoint needs one.
    pub api_key: Option<String>,
    /// Default model identifier.
    pub model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            api_key: std::env::var("OPENAI_API_KEY")
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

/// The provider.
pub struct OpenAiCompatible {
    settings: Settings,
    client: Client<HttpsConnector<HttpConnector>, Full<hyper::body::Bytes>>,
}

impl OpenAiCompatible {
    /// Build from settings (or the environment with [`Settings::default`]).
    pub fn new(settings: Settings) -> Self {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        let client = Client::builder(hyper_util::rt::TokioExecutor::new()).build(https);
        OpenAiCompatible { settings, client }
    }
}

impl Default for OpenAiCompatible {
    fn default() -> Self {
        OpenAiCompatible::new(Settings::default())
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
/// The cache-boundary hint (`stable_prefix`) is advisory for this shape of
/// endpoint and is not forwarded.
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
        cost: 0.0,
        extras: BTreeMap::new(),
    }
}

/// One-shot decode of a complete SSE body (tests and error paths).
pub fn parse_sse(body: &[u8], emit: &mut dyn FnMut(StreamEvent)) {
    let mut decoder = SseDecoder::default();
    decoder.feed(body, emit);
    decoder.finish(emit);
}

impl Provider for OpenAiCompatible {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: self.settings.model.clone(),
            name: self.settings.model.clone(),
            context_window: 0,
            max_tokens: 0,
        }]
    }

    fn stream(
        &self,
        request: CompletionRequest,
        tx: EventSender,
    ) -> BoxFuture<Result<(), ProviderError>> {
        let settings = self.settings.clone();
        let client = self.client.clone();
        let url = format!(
            "{}/chat/completions",
            settings.base_url.trim_end_matches('/')
        );
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
        Box::pin(async move {
            let mut builder = hyper::Request::builder()
                .method("POST")
                .uri(&url)
                .header(CONTENT_TYPE, "application/json");
            if let Some(key) = &settings.api_key {
                builder = builder.header(AUTHORIZATION, format!("Bearer {key}"));
            }
            let request = builder
                .body(Full::from(hyper::body::Bytes::from(
                    serde_json::to_vec(&body).expect("request serializes"),
                )))
                .map_err(|err| ProviderError::invalid(format!("cannot build request: {err}")))?;

            let response = client
                .request(request)
                .await
                .map_err(|err| ProviderError::transport(format!("request failed: {err}")))?;

            let status = response.status().as_u16();
            if !(200..300).contains(&status) {
                let collected = response
                    .into_body()
                    .collect()
                    .await
                    .map(|collected| collected.to_bytes())
                    .unwrap_or_default();
                let detail = String::from_utf8_lossy(&collected);
                let detail: serde_json::Value = serde_json::from_str(&detail).unwrap_or_default();
                let detail = detail
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                let class = class_for_status(status);
                let retryable = classify_status(status);
                return Err(ProviderError {
                    message: format!("provider returned HTTP {status}: {detail}"),
                    class,
                    retryable,
                });
            }

            let mut decoder = SseDecoder::default();
            let mut body = response.into_body();
            loop {
                match body.frame().await {
                    None => break,
                    Some(Err(err)) => {
                        return Err(ProviderError::transport(format!(
                            "stream interrupted: {err}"
                        )));
                    }
                    Some(Ok(frame)) => {
                        if let Ok(data) = frame.into_data() {
                            decoder.feed(&data, &mut |event| {
                                // The receiver disappears when the host
                                // cancels; that ends the stream quietly.
                                let _ = tx.try_send(event);
                            });
                        }
                    }
                }
            }
            decoder.finish(&mut |event| {
                let _ = tx.try_send(event);
            });
            Ok(())
        })
    }
}
