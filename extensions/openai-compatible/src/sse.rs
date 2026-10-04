//! The streaming response decoder: bytes in, typed events out. Split
//! from `lib.rs` for the 1,200-line ceiling (gate 11); the public
//! surface (`parse_sse`, `SseDecoder`) is re-exported there unchanged.

use std::collections::BTreeMap;

use lca_protocol::{StreamEvent, Usage};

/// Streaming response decoder: feed it bytes, it emits typed events.
#[derive(Default)]
pub struct SseDecoder {
    buffer: String,
    open_calls: Vec<(usize, String, String)>, // (index, call_id, name)
    finished: bool,
    /// Whether any well-formed frame (or `[DONE]`) was seen: a body with
    /// none is not an empty answer, it is not an SSE stream.
    saw_data: bool,
    /// The generation budget the request set, when it set one: a
    /// `finish_reason: length` then becomes a failure that names the
    /// number (gh #169). `None` for ordinary chat turns, which carry no
    /// budget of ours and keep their old quiet end.
    max_tokens: Option<u64>,
}

impl SseDecoder {
    /// A decoder that knows the request's generation budget, so a
    /// `length` finish can name it (gh #169).
    pub fn with_max_tokens(mut self, max_tokens: Option<u64>) -> Self {
        self.max_tokens = max_tokens;
        self
    }

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
        if !self.saw_data {
            // A 200 with a body that never spoke SSE is not an empty
            // answer; surface it instead of reading as a silent success.
            emit(StreamEvent::Error {
                message: "the provider's response was not a server-sent event stream".to_string(),
                retryable: false,
            });
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
                self.saw_data = true;
                for (_, call_id, _) in std::mem::take(&mut self.open_calls) {
                    emit(StreamEvent::ToolCallEnd { call_id });
                }
                self.finished = true;
                return;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue; // a malformed frame drops; the stream continues
            };
            self.saw_data = true;
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
        match choice.get("finish_reason").and_then(|f| f.as_str()) {
            Some("tool_calls") => {
                for (_, call_id, _) in std::mem::take(&mut self.open_calls) {
                    emit(StreamEvent::ToolCallEnd { call_id });
                }
            }
            // gh #169: a budgeted request (the completion capability's
            // summarization) that ends on `length` was cut mid-generation.
            // Say so, with the number, instead of passing the truncation
            // off as an answer; unbudgeted chat turns carry no number of
            // ours to name and end as they always did.
            Some("length") => {
                if let Some(budget) = self.max_tokens {
                    emit(StreamEvent::Error {
                        message: format!(
                            "generation hit the token cap (max_tokens {budget}) before \
                             finishing; the response is incomplete"
                        ),
                        retryable: false,
                    });
                }
            }
            _ => {}
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
