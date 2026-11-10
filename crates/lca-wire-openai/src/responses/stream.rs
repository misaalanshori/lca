//! The Responses SSE event mapper: typed events from the vendor's frame shapes,
//! the open-call bookkeeping, and the top-level error envelope.
//!
//! The decoder is a state machine over **complete** frames (gh #337: bytes until a
//! frame exists, then each frame on its own), so a codepoint straddling a chunk
//! boundary cannot be corrupted and one bad frame cannot poison a whole body.

use lca_protocol::StreamEvent;

use super::body::responses_usage;

/// One open tool call tracked by output index.
struct OpenCall {
    index: u64,
    id: String,
    args: String,
}

/// The Responses SSE mapper: feed decoded `data:` payloads, collect
/// typed events. Tool calls open on `output_item.added`, stream args
/// on `function_call_arguments.delta`, and close on `.done` (or on the
/// `output_item.done` whole, when deltas never came).
pub struct ResponsesStream {
    open: Vec<OpenCall>,
}

impl ResponsesStream {
    /// An empty mapper.
    pub fn new() -> Self {
        ResponsesStream { open: Vec::new() }
    }

    /// Map one decoded event payload into typed events.
    pub fn feed(&mut self, value: &serde_json::Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let Some(event_type) = value.get("type").and_then(|kind| kind.as_str()) else {
            return out;
        };
        match event_type {
            "response.output_item.added" => {
                let index = value.get("output_index").and_then(|i| i.as_u64());
                let item = value.get("item");
                let (item_type, call_id, name) = (
                    item.and_then(|i| i.get("type")).and_then(|t| t.as_str()),
                    item.and_then(|i| i.get("call_id"))
                        .and_then(|id| id.as_str()),
                    item.and_then(|i| i.get("name")).and_then(|n| n.as_str()),
                );
                if let (Some(index), Some("function_call"), Some(call_id), Some(name)) =
                    (index, item_type, call_id, name)
                    && !self.open.iter().any(|call| call.index == index)
                {
                    self.open.push(OpenCall {
                        index,
                        id: call_id.to_string(),
                        args: String::new(),
                    });
                    out.push(StreamEvent::ToolCallStart {
                        call_id: call_id.to_string(),
                        name: name.to_string(),
                    });
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(delta) = value.get("delta").and_then(|d| d.as_str()) {
                    out.push(StreamEvent::TextDelta {
                        delta: delta.to_string(),
                    });
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = value.get("delta").and_then(|d| d.as_str()) {
                    out.push(StreamEvent::ReasoningDelta {
                        delta: delta.to_string(),
                    });
                }
            }
            "response.reasoning_summary_part.done" => {
                out.push(StreamEvent::ReasoningDelta {
                    delta: "\n\n".to_string(),
                });
            }
            "response.function_call_arguments.delta" => {
                let (index, delta) = (
                    value.get("output_index").and_then(|i| i.as_u64()),
                    value.get("delta").and_then(|d| d.as_str()),
                );
                if let (Some(index), Some(delta)) = (index, delta)
                    && let Some(call) = self.open.iter_mut().find(|call| call.index == index)
                {
                    call.args.push_str(delta);
                    out.push(StreamEvent::ToolCallArgDelta {
                        call_id: call.id.clone(),
                        delta: delta.to_string(),
                    });
                }
            }
            "response.function_call_arguments.done" => {
                let (index, arguments) = (
                    value.get("output_index").and_then(|i| i.as_u64()),
                    value.get("arguments").and_then(|a| a.as_str()),
                );
                if let (Some(index), Some(arguments)) = (index, arguments)
                    && let Some(call) = self.open.iter_mut().find(|call| call.index == index)
                {
                    // The whole replaces streaming fragments only when
                    // the fragments are not its prefix (fx's rule).
                    if !arguments.starts_with(&call.args) {
                        call.args.clear();
                        call.args.push_str(arguments);
                    } else if arguments.len() > call.args.len() {
                        let suffix = arguments[call.args.len()..].to_string();
                        call.args.push_str(&suffix);
                        out.push(StreamEvent::ToolCallArgDelta {
                            call_id: call.id.clone(),
                            delta: suffix,
                        });
                    }
                    let id = call.id.clone();
                    out.push(StreamEvent::ToolCallEnd { call_id: id });
                    self.open.retain(|call| call.index != index);
                }
            }
            "response.output_item.done" => {
                let index = value.get("output_index").and_then(|i| i.as_u64());
                let item = value.get("item");
                let item_type = item.and_then(|i| i.get("type")).and_then(|t| t.as_str());
                if let (Some(index), Some("function_call")) = (index, item_type)
                    && let Some(position) = self.open.iter().position(|call| call.index == index)
                {
                    // Whole arguments land when no deltas ever came.
                    if self.open[position].args.is_empty()
                        && let Some(arguments) = item
                            .and_then(|i| i.get("arguments"))
                            .and_then(|a| a.as_str())
                    {
                        self.open[position].args.push_str(arguments);
                        out.push(StreamEvent::ToolCallArgDelta {
                            call_id: self.open[position].id.clone(),
                            delta: arguments.to_string(),
                        });
                    }
                    let id = self.open[position].id.clone();
                    out.push(StreamEvent::ToolCallEnd { call_id: id });
                    self.open.remove(position);
                }
            }
            "response.completed" | "response.done" | "response.incomplete" => {
                if let Some(response) = value.get("response")
                    && let Some(usage) = responses_usage(response)
                {
                    out.push(StreamEvent::Usage { usage });
                }
            }
            "response.failed" | "error" => {
                let message = value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(|message| message.as_str())
                    .or_else(|| value.get("message").and_then(|message| message.as_str()))
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

impl Default for ResponsesStream {
    fn default() -> Self {
        ResponsesStream::new()
    }
}

/// A top-level `{"error": ...}` frame (outside any response object).
pub(super) fn single_error(value: &serde_json::Value) -> Option<lca_protocol::StreamEvent> {
    let error = value.get("error")?;
    let message = error
        .get("message")
        .and_then(|message| message.as_str())
        .unwrap_or("vendor error");
    Some(lca_protocol::StreamEvent::Error {
        message: message.to_string(),
        retryable: lca_protocol::is_capacity_error(message),
    })
}
