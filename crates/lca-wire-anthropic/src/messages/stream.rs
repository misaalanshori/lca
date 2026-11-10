//! The Anthropic Messages SSE decoder: text, thinking, input-JSON tool deltas,
//! usage buckets, and thinking signatures.
//!
//! A state machine over decoded `data:` payloads (`AnthropicStream::feed`), plus
//! the one-shot [`parse_sse`] for complete bodies. Grammar port from pi's
//! `anthropic-messages.ts` event table.
//!
//! Thinking signatures accumulate off `signature_delta` and emit as a
//! [`THINKING_SIGNATURE_KIND`] vendor event when the block closes, so multi-turn
//! continuity (#41) has the bytes even though the host does not resend them yet;
//! nothing is dropped, nothing is invented.

use lca_protocol::{StreamEvent, THINKING_SIGNATURE_KIND, Usage};

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
                if let Some(reason) = value
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(|reason| reason.as_str())
                {
                    // Gh #390 gap 1: Anthropic spells the output-token cut
                    // `max_tokens`, and it is the same fact the OpenAI kits call
                    // `length` - so it produces the same signal, which the turn
                    // reports as `StopReason::Length`.
                    if reason == "max_tokens" {
                        out.push(StreamEvent::Truncated);
                    }
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
