//! The Responses-protocol core every subscription gateway shares
//! (gh #63): one request-body builder and one SSE event mapper over
//! the OpenAI Responses shapes both Codex and Grok speak. Provider
//! differences (endpoints, headers, account) stay in the extensions'
//! spec tables.

use lca_protocol::{CompletionRequest, ContentBlock, MessageRole, StreamEvent, ToolSpec, Usage};

/// One request body for the Responses gateways: model, instructions,
/// input items, tools, and the reasoning effort when the caller names
/// one. `store: false` always — these endpoints reject stored
/// responses; continuation state rides the conversation, not the
/// server (pi's `store: false` + `previous_response_id` note).
pub fn build_responses_body(
    request: &CompletionRequest,
    system: &str,
    model: &str,
    effort: Option<&str>,
) -> serde_json::Value {
    let mut input = Vec::new();
    for message in &request.messages {
        match message.role {
            MessageRole::System => {}
            MessageRole::User => {
                let text: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    input.push(serde_json::json!({
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": text}],
                    }));
                }
            }
            MessageRole::Assistant => {
                let text: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    input.push(serde_json::json!({
                        "type": "message",
                        "role": "assistant",
                        "status": "completed",
                        "content": [{
                            "type": "output_text",
                            "text": text,
                            "annotations": [],
                        }],
                    }));
                }
                for call in &message.tool_calls {
                    let args: serde_json::Value = serde_json::from_str(&call.arguments)
                        .unwrap_or_else(|_| serde_json::json!({}));
                    input.push(serde_json::json!({
                        "type": "function_call",
                        "call_id": call.call_id,
                        "name": call.name,
                        "arguments": args,
                    }));
                }
            }
            MessageRole::Tool => {
                let text: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": message.tool_call_id.as_deref().unwrap_or(""),
                    "output": text,
                }));
            }
        }
    }
    let mut body = serde_json::json!({
        "model": model,
        "store": false,
        "stream": true,
        "instructions": if system.is_empty() { "You are a helpful assistant.".to_string() } else { system.to_string() },
        "input": input,
        "text": {"verbosity": "low"},
        "include": ["reasoning.encrypted_content"],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
    });
    if !request.tools.is_empty() {
        body["tools"] = serde_json::Value::Array(request.tools.iter().map(function_tool).collect());
    }
    if let Some(effort) = effort {
        // fx maps its `minimal` level to the wire's `low`.
        let effort = if effort == "minimal" { "low" } else { effort };
        body["reasoning"] = serde_json::json!({"effort": effort, "summary": "auto"});
    }
    body
}

/// One function declaration in the Responses shape.
fn function_tool(tool: &ToolSpec) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.parameters,
    })
}

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
                    retryable: false,
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

/// The token counts off a terminal `response` object.
pub fn responses_usage(response: &serde_json::Value) -> Option<Usage> {
    let usage = response.get("usage")?;
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
