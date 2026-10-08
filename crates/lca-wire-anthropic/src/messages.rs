//! The Anthropic Messages wire engine (`POST /v1/messages`, gh #189):
//! the request builder (system, messages with tool_use/tool_result
//! round-trips, tools with prompt-cache breakpoints) and the SSE event
//! decoder (text, thinking, input-JSON tool deltas, usage buckets).
//! Grammar port from pi's `anthropic-messages.ts` event table; no
//! consumer wires it to a turn yet (that arrives with the `anthropic`
//! provider, #183), so this kit maps onto [`StreamEvent`], the host
//! vocabulary every kit shares.
//!
//! Thinking signatures ride [`StreamEvent::VendorEvent`] with kind
//! `"thinking-signature"` until #41 teaches the host to resend them;
//! nothing is dropped, nothing is invented. Image blocks map to
//! Anthropic's base64 source blocks like the OpenAI kit's vision parts.

use lca_protocol::{CompletionRequest, ContentBlock, MessageRole, StreamEvent, ToolSpec, Usage};

/// A thinking signature preserved for multi-turn continuity (#41): the
/// decoder emits it when the thinking block closes. Re-exported from
/// the protocol crate, where the host matches it.
pub use lca_protocol::THINKING_SIGNATURE_KIND;

/// Prompt-cache breakpoint both kits understand: Anthropic's ephemeral
/// marker, attached to the system prompt, the last message block, and
/// the last tool declaration when the caller opts in (pi's cache
/// injection points).
fn breakpoint() -> serde_json::Value {
    serde_json::json!({"type": "ephemeral"})
}

/// Build the `/v1/messages` request body: the model, the token budget,
/// the system prompt, the mapped messages, and the tool declarations.
/// `cache_breakpoints` plants pi's ephemeral markers (system, last
/// message block, last tool); without it the body carries none.
pub fn build_messages_body(
    request: &CompletionRequest,
    system: &str,
    model: &str,
    max_tokens: u32,
    cache_breakpoints: bool,
) -> serde_json::Value {
    let system_value = if system.is_empty() {
        serde_json::Value::Null
    } else if cache_breakpoints {
        serde_json::json!([{"type": "text", "text": system, "cache_control": breakpoint()}])
    } else {
        serde_json::json!(system)
    };
    let mut messages = Vec::new();
    for message in &request.messages {
        match message.role {
            MessageRole::System => {}
            MessageRole::User => {
                let mut blocks = content_blocks(&message.content);
                if blocks.is_empty() {
                    continue;
                }
                if cache_breakpoints && let Some(last) = blocks.last_mut() {
                    last["cache_control"] = breakpoint();
                }
                messages.push(serde_json::json!({"role": "user", "content": blocks}));
            }
            MessageRole::Assistant => {
                let mut blocks = content_blocks(&message.content);
                for call in &message.tool_calls {
                    let input: serde_json::Value =
                        serde_json::from_str(&call.arguments).unwrap_or(serde_json::json!({}));
                    blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": call.call_id,
                        "name": call.name,
                        "input": input,
                    }));
                }
                if blocks.is_empty() {
                    continue;
                }
                messages.push(serde_json::json!({"role": "assistant", "content": blocks}));
            }
            MessageRole::Tool => {
                let text: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": message.tool_call_id.as_deref().unwrap_or(""),
                        "content": text,
                    }],
                }));
            }
        }
    }
    let mut tools: Vec<serde_json::Value> = request.tools.iter().map(tool_declaration).collect();
    if cache_breakpoints && let Some(last) = tools.last_mut() {
        last["cache_control"] = breakpoint();
    }
    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": messages,
    });
    if !system_value.is_null() {
        body["system"] = system_value;
    }
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
    }
    // gh #41: the host resolves the turn's level to a token budget
    // (`thinking-budget-tokens` extra); budget-taking vendors send it
    // as `thinking.enabled`, clamped so at least 1024 tokens remain
    // for the answer under the shared ceiling (pi's `MIN_ANSWER_TOKENS`
    // rule). No budget extra means no `thinking` key.
    if let Some(budget) = request
        .extras
        .get("thinking-budget-tokens")
        .and_then(|value| value.parse::<u64>().ok())
    {
        let room = max_tokens.saturating_sub(1024);
        body["thinking"] = serde_json::json!({
            "type": "enabled",
            "budget_tokens": budget.min(u64::from(room)),
        });
    }
    body
}

/// One message's content blocks in the Messages shape.
fn content_blocks(content: &[ContentBlock]) -> Vec<serde_json::Value> {
    let mut blocks = Vec::new();
    let text: String = content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if !text.is_empty() {
        blocks.push(serde_json::json!({"type": "text", "text": text}));
    }
    for block in content {
        match block {
            ContentBlock::Reasoning {
                reasoning,
                signature,
            } => {
                // gh #41: replay resends the thinking verbatim with its
                // signature; a signature over redacted (empty) text
                // travels as `redacted_thinking`, pi's shape, so the
                // vendor accepts the transcript.
                match signature {
                    Some(signature) if reasoning.is_empty() => {
                        blocks.push(serde_json::json!({
                            "type": "redacted_thinking",
                            "data": signature,
                        }));
                    }
                    Some(signature) => {
                        blocks.push(serde_json::json!({
                            "type": "thinking",
                            "thinking": reasoning,
                            "signature": signature,
                        }));
                    }
                    // Unsigned reasoning never goes back on the wire:
                    // resending thinking without its signature is what
                    // signature-requiring vendors reject.
                    None => {}
                }
            }
            ContentBlock::Image { media_type, bytes } => {
                blocks.push(serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": lca_protocol::base64_encode(bytes),
                    },
                }));
            }
            _ => {}
        }
    }
    blocks
}

/// One tool declaration in the Messages shape.
fn tool_declaration(tool: &ToolSpec) -> serde_json::Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.parameters,
    })
}

/// The beta enabling cache-preserving mid-conversation tool changes
/// (gh #201, pi's port of Anthropic's `inline-tools-2026-09-15`): the
/// extension (#183) sends it as the `anthropic-beta` header when the
/// frozen list below is in play.
pub const INLINE_TOOLS_BETA: &str = "inline-tools-2026-09-15";

/// The stable placeholder freezing the top-level tool list (gh #201):
/// pi's name verbatim (wire-visible), so the cached prefix survives
/// tools that arrive later.
pub const DEFERRED_PLACEHOLDER: &str = "__pi_deferred_placeholder__";

/// The frozen top-level tool list (gh #201): the turn-one
/// declarations plus the deferred placeholder, byte-stable across
/// every later turn. Tools introduced later never touch this list —
/// they travel as system `tool_addition` blocks, and removals as
/// `tool_removal` blocks, so the prompt-cache prefix never
/// invalidates. The extension (#183) snapshots this once and sends
/// the `INLINE_TOOLS_BETA` header with it.
pub fn frozen_tools(tools: &[ToolSpec]) -> Vec<serde_json::Value> {
    let mut frozen: Vec<serde_json::Value> = tools.iter().map(tool_declaration).collect();
    frozen.push(serde_json::json!({
        "name": DEFERRED_PLACEHOLDER,
        "defer_loading": true,
    }));
    frozen
}

/// A mid-conversation tool definition for a system message (gh #201):
/// the full definition by value at the exact turn it arrives.
pub fn tool_addition_block(tool: &ToolSpec) -> serde_json::Value {
    serde_json::json!({
        "type": "tool_addition",
        "tool": {
            "type": "tool_definition",
            "definition": {
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.parameters,
            },
        },
    })
}

/// A mid-conversation tool removal for a system message (gh #201): a
/// reference by name, leaving the frozen list untouched.
pub fn tool_removal_block(name: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "tool_removal",
        "tool": {
            "type": "tool_reference",
            "name": name,
        },
    })
}

/// One open content block tracked by stream index.
struct OpenBlock {
    index: u64,
    call_id: String,
    signature: String,
}

/// The Messages SSE mapper: feed decoded `data:` payloads, collect
/// typed events. Text and thinking deltas stream by block index, tool
/// input assembles from `input_json_delta` fragments, and usage lands
/// off `message_start` (full counts) and `message_delta` (output).
/// Thinking signatures accumulate off `signature_delta` and emit as a
/// [`THINKING_SIGNATURE_KIND`] vendor event when the block closes, so
/// multi-turn continuity (#41) has the bytes even though the host does
/// not resend them yet.
pub struct AnthropicStream {
    open: Vec<OpenBlock>,
}

impl AnthropicStream {
    /// An empty mapper.
    pub fn new() -> Self {
        AnthropicStream { open: Vec::new() }
    }

    /// Map one decoded event payload into typed events.
    pub fn feed(&mut self, value: &serde_json::Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let Some(event_type) = value.get("type").and_then(|kind| kind.as_str()) else {
            return out;
        };
        match event_type {
            "message_start" => {
                if let Some(message) = value.get("message")
                    && let Some(usage) = message_usage(message)
                {
                    out.push(StreamEvent::Usage { usage });
                }
            }
            "content_block_start" => {
                let (index, block) = (
                    value.get("index").and_then(|i| i.as_u64()),
                    value.get("content_block"),
                );
                let block_type = block.and_then(|b| b.get("type")).and_then(|t| t.as_str());
                match (index, block_type) {
                    (Some(_), Some("text")) => {
                        let text = block
                            .and_then(|b| b.get("text"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        if !text.is_empty() {
                            out.push(StreamEvent::TextDelta {
                                delta: text.to_string(),
                            });
                        }
                    }
                    (Some(index), Some("thinking")) => {
                        let thinking = block
                            .and_then(|b| b.get("thinking"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        if !thinking.is_empty() {
                            out.push(StreamEvent::ReasoningDelta {
                                delta: thinking.to_string(),
                            });
                        }
                        let signature = block
                            .and_then(|b| b.get("signature"))
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                        self.open.push(OpenBlock {
                            index,
                            call_id: String::new(),
                            signature,
                        });
                    }
                    (Some(index), Some("redacted_thinking")) => {
                        // pi surfaces the redaction marker as the thinking
                        // content and keeps the bytes for continuity.
                        out.push(StreamEvent::ReasoningDelta {
                            delta: "[Reasoning redacted]".to_string(),
                        });
                        let signature = block
                            .and_then(|b| b.get("data"))
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                        self.open.push(OpenBlock {
                            index,
                            call_id: String::new(),
                            signature,
                        });
                    }
                    (Some(index), Some("tool_use")) => {
                        let (id, name) = (
                            block.and_then(|b| b.get("id")).and_then(|i| i.as_str()),
                            block.and_then(|b| b.get("name")).and_then(|n| n.as_str()),
                        );
                        if let (Some(id), Some(name)) = (id, name)
                            && !self.open.iter().any(|open| open.index == index)
                        {
                            self.open.push(OpenBlock {
                                index,
                                call_id: id.to_string(),
                                signature: String::new(),
                            });
                            out.push(StreamEvent::ToolCallStart {
                                call_id: id.to_string(),
                                name: name.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let (index, delta) = (
                    value.get("index").and_then(|i| i.as_u64()),
                    value.get("delta"),
                );
                let delta_type = delta.and_then(|d| d.get("type")).and_then(|t| t.as_str());
                match (index, delta_type) {
                    (Some(_), Some("text_delta")) => {
                        if let Some(text) =
                            delta.and_then(|d| d.get("text")).and_then(|t| t.as_str())
                            && !text.is_empty()
                        {
                            out.push(StreamEvent::TextDelta {
                                delta: text.to_string(),
                            });
                        }
                    }
                    (Some(_), Some("thinking_delta")) => {
                        if let Some(thinking) = delta
                            .and_then(|d| d.get("thinking"))
                            .and_then(|t| t.as_str())
                            && !thinking.is_empty()
                        {
                            out.push(StreamEvent::ReasoningDelta {
                                delta: thinking.to_string(),
                            });
                        }
                    }
                    (Some(index), Some("input_json_delta")) => {
                        if let Some(fragment) = delta
                            .and_then(|d| d.get("partial_json"))
                            .and_then(|p| p.as_str())
                            && !fragment.is_empty()
                            && let Some(open) = self.open.iter().find(|open| open.index == index)
                            && !open.call_id.is_empty()
                        {
                            out.push(StreamEvent::ToolCallArgDelta {
                                call_id: open.call_id.clone(),
                                delta: fragment.to_string(),
                            });
                        }
                    }
                    (Some(index), Some("signature_delta")) => {
                        if let Some(signature) = delta
                            .and_then(|d| d.get("signature"))
                            .and_then(|s| s.as_str())
                            && let Some(open) =
                                self.open.iter_mut().find(|open| open.index == index)
                        {
                            open.signature.push_str(signature);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(index) = value.get("index").and_then(|i| i.as_u64())
                    && let Some(position) = self.open.iter().position(|open| open.index == index)
                {
                    let open = self.open.remove(position);
                    if !open.call_id.is_empty() {
                        out.push(StreamEvent::ToolCallEnd {
                            call_id: open.call_id,
                        });
                    } else if !open.signature.is_empty() {
                        out.push(StreamEvent::VendorEvent {
                            kind: THINKING_SIGNATURE_KIND.to_string(),
                            payload: serde_json::json!({"signature": open.signature}),
                        });
                    }
                }
            }
            "message_delta" => {
                if let Some(usage) = value.get("usage")
                    && let Some(mapped) = delta_usage(usage)
                {
                    out.push(StreamEvent::Usage { usage: mapped });
                }
                if value
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(|reason| reason.as_str())
                    .is_some()
                {
                    // A stopped message closes whatever is still open, the
                    // same way the other kits close forgotten calls.
                    for open in self.open.drain(..) {
                        if !open.call_id.is_empty() {
                            out.push(StreamEvent::ToolCallEnd {
                                call_id: open.call_id,
                            });
                        }
                    }
                }
            }
            "message_stop" => {
                for open in self.open.drain(..) {
                    if !open.call_id.is_empty() {
                        out.push(StreamEvent::ToolCallEnd {
                            call_id: open.call_id,
                        });
                    }
                }
            }
            "error" => {
                let message = value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(|message| message.as_str())
                    .unwrap_or("vendor error");
                out.push(StreamEvent::Error {
                    message: message.to_string(),
                    retryable: lca_protocol::is_capacity_error(message),
                });
            }
            _ => {}
        }
        out
    }
}

impl Default for AnthropicStream {
    fn default() -> Self {
        AnthropicStream::new()
    }
}

/// The token counts off a `message_start` message object, including
/// both cache buckets.
pub fn message_usage(message: &serde_json::Value) -> Option<Usage> {
    let usage = message.get("usage")?;
    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if input == 0 && output == 0 {
        return None;
    }
    Some(Usage {
        input,
        output,
        cache_read: usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_write: usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_write_1h: usage
            .get("cache_creation")
            .and_then(|creation| creation.get("ephemeral_1h_input_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cost: 0.0,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: Default::default(),
    })
}

/// The token counts off a `message_delta` usage object (output only;
/// proxies may omit the input counts the start already reported, pi's
/// rule, so a zero/zero delta reports nothing).
fn delta_usage(usage: &serde_json::Value) -> Option<Usage> {
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if output == 0 {
        return None;
    }
    Some(Usage {
        input: 0,
        output,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: 0,
        cost: 0.0,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: Default::default(),
    })
}

/// One-shot decode of a complete Messages SSE body (tests and error
/// paths): split on frame boundaries, take each `data:` payload, feed
/// the values. `event:` lines carry no payload and are skipped.
pub fn parse_sse(body: &[u8], emit: &mut dyn FnMut(StreamEvent)) {
    let mut stream = AnthropicStream::new();
    let text = String::from_utf8_lossy(body);
    for frame in text.split("\n\n") {
        for line in frame.lines() {
            let line = line.trim_end_matches('\r');
            let Some(payload) = line.strip_prefix("data:").map(str::trim) else {
                continue;
            };
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            for event in stream.feed(&value) {
                emit(event);
            }
        }
    }
}
