//! The Responses wire engine (`POST /v1/responses`, gh #63/#189):
//! one request-body builder and one SSE event mapper over the shapes
//! Codex and Grok speak, plus the pull-based driver over the host's
//! `ProviderCap` surface. Extracted verbatim from `lca-subscription`;
//! provider differences (endpoints, headers, account) stay in the
//! extensions' spec tables, and credential lifecycle (the 401 purge)
//! stays the caller's - it arrives as a callback.

use lca_protocol::{CompletionRequest, ContentBlock, MessageRole, StreamEvent, ToolSpec, Usage};

use crate::error::{StreamFailure, failure_for_status, json_error_message};

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
                // Gh #338: images ride the same content array as text, in the
                // API's `input_image` shape (pi's `openai-responses-shared`):
                // text parts first, then image parts, `detail: "auto"`, the
                // bytes inlined as a data URL. The old serializer filtered
                // `Text` only, so an image was dropped - and an image-only
                // message joined to an empty string and vanished entirely.
                let mut parts: Vec<serde_json::Value> = Vec::new();
                let mut text = String::new();
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text: part } => text.push_str(part),
                        ContentBlock::Image { .. } => {}
                        ContentBlock::Reasoning { .. } | ContentBlock::ToolCall { .. } => {}
                    }
                }
                if !text.is_empty() {
                    parts.push(serde_json::json!({"type": "input_text", "text": text}));
                }
                for block in &message.content {
                    if let ContentBlock::Image { media_type, bytes } = block {
                        parts.push(serde_json::json!({
                            "type": "input_image",
                            "detail": "auto",
                            "image_url": format!(
                                "data:{media_type};base64,{}",
                                lca_protocol::base64_encode(bytes)
                            ),
                        }));
                    }
                }
                if !parts.is_empty() {
                    input.push(serde_json::json!({
                        "type": "message",
                        "role": "user",
                        "content": parts,
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
    // Gh #338: the Responses API reports prompt-cache hits under
    // `input_tokens_details.cached_tokens`, exactly as Chat Completions
    // reports them under `prompt_tokens_details.cached_tokens`. This was
    // hardcoded to 0, so every cache hit on codex/grok was billed as fresh
    // input.
    let cache_read = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if input == 0 && output == 0 {
        return None;
    }
    Some(Usage {
        // `input_tokens` counts the cached tokens too; the rest of this kit
        // treats `input` as the uncached remainder (`map_usage` does the
        // same subtraction for Chat Completions).
        input: input.saturating_sub(cache_read),
        output,
        cache_read,
        cache_write: 0,
        cache_write_1h: 0,
        cost: 0.0,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: Default::default(),
    })
}

/// A pull-based driver over one Responses streaming completion: open
/// sends the request and checks the status (a 401 purges the stored
/// tokens, gh #179's rule, shared by every gateway on this kit);
/// `next_event` yields typed events as the host yields body chunks.
pub struct ResponseStreamDriver<'a> {
    cap: &'a dyn lca_protocol::ProviderCap,
    handle: u32,
    /// Raw bytes until a complete frame is available (gh #337): never a
    /// `String`, or a codepoint straddling a chunk boundary would be
    /// corrupted.
    buffer: crate::sse::FrameBuffer,
    mapper: ResponsesStream,
    pending: std::collections::VecDeque<lca_protocol::StreamEvent>,
    finished: bool,
    /// Whether any frame carried a `data:` payload: a 200 body with none is
    /// not an empty answer (gh #359).
    saw_frame: bool,
}

impl<'a> ResponseStreamDriver<'a> {
    /// Send the request and check the status. A 401 calls
    /// `on_unauthorized` (the caller's credential purge, gh #179's
    /// rule) before the status error is built.
    pub fn open(
        cap: &'a dyn lca_protocol::ProviderCap,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        on_unauthorized: &dyn Fn(),
    ) -> Result<ResponseStreamDriver<'a>, StreamFailure> {
        let handle = cap
            .net_request("POST", url, headers, Some(body))
            .map_err(StreamFailure::from)?;
        let status = cap
            .net_response_status(handle)
            .map_err(StreamFailure::from)?;
        if !(200..300).contains(&status) {
            if status == 401 {
                on_unauthorized();
            }
            let mut detail = Vec::new();
            while let Some(chunk) = cap
                .net_read_body(handle, 64 * 1024)
                .map_err(StreamFailure::from)?
            {
                detail.extend_from_slice(&chunk);
                if detail.len() > 1024 * 1024 {
                    break;
                }
            }
            let _ = cap.net_close_response(handle);
            let text = String::from_utf8_lossy(&detail);
            return Err(failure_for_status(status, &json_error_message(&text)));
        }
        Ok(ResponseStreamDriver {
            cap,
            handle,
            buffer: crate::sse::FrameBuffer::new(),
            mapper: ResponsesStream::new(),
            pending: std::collections::VecDeque::new(),
            finished: false,
            saw_frame: false,
        })
    }

    /// The next typed event.
    pub fn next_event(&mut self) -> Option<Result<lca_protocol::StreamEvent, StreamFailure>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            if self.finished {
                return None;
            }
            match self
                .cap
                .net_read_body(self.handle, 64 * 1024)
                .map_err(StreamFailure::from)
            {
                Ok(Some(chunk)) => {
                    // Gh #337: bytes until a complete frame exists, then
                    // each *complete* frame decodes on its own.
                    let mut frames: Vec<String> = Vec::new();
                    self.buffer
                        .feed(&chunk, &mut |frame| frames.push(frame.to_string()));
                    for frame in frames {
                        self.handle_frame(&frame);
                    }
                }
                Ok(None) => {
                    self.finished = true;
                    self.finish_stream();
                }
                Err(err) => {
                    self.finished = true;
                    return Some(Err(err));
                }
            }
        }
    }

    /// One complete frame's `data:` payloads into events.
    fn handle_frame(&mut self, frame: &str) {
        for payload in crate::sse::frame_payloads(frame) {
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            self.saw_frame = true;
            if let Some(event) = single_error(&value) {
                self.pending.push_back(event);
                continue;
            }
            self.pending.extend(self.mapper.feed(&value));
        }
    }

    /// End of stream (gh #359).
    ///
    /// A truncated tail is the last thing a body can carry, so it is parsed
    /// rather than dropped: a tail with a `data:` line is a frame that lost
    /// its terminator, and one without surfaces as a transport error. Then,
    /// if the body never carried a frame at all, that is reported as a
    /// transport failure - an empty assistant message is indistinguishable
    /// from a model that said nothing, and it must never mean that.
    fn finish_stream(&mut self) {
        if let Some(tail) = self.buffer.take_tail() {
            let parseable = tail.lines().any(|line| {
                line.trim_end_matches('\r')
                    .trim_start()
                    .starts_with("data:")
            });
            self.handle_frame(&tail);
            // A tail with no `data:` line cannot have been part of a frame,
            // so it is a truncated frame whether or not earlier frames
            // arrived; report it either way (gh #359: no received byte
            // disappears silently).
            if !parseable && !tail.trim().is_empty() {
                self.pending
                    .push_back(crate::sse::truncated_tail_event(&tail));
            }
        }
        if !self.saw_frame {
            self.pending.push_back(lca_protocol::StreamEvent::Error {
                message: crate::sse::no_frames_failure("").message,
                retryable: false,
            });
        }
    }
}

impl Drop for ResponseStreamDriver<'_> {
    fn drop(&mut self) {
        let _ = self.cap.net_close_response(self.handle);
    }
}

/// A top-level `{"error": ...}` frame (outside any response object).
fn single_error(value: &serde_json::Value) -> Option<lca_protocol::StreamEvent> {
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
