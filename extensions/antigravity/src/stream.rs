//! The Antigravity SSE stream driver, request body envelope, and chunk parser.
//! Split from `lib.rs` to satisfy workspace Gate 11 ceiling.

use crate::{
    DEFAULT_API_BASE, DEFAULT_USER_AGENT, StreamFailure, access_token, endpoint,
    failure_for_status, json_error_message, now_epoch,
};
use lca_protocol::ProviderCap;

/// Pure agy CLI wire alignment (pi-antigravity `src/utils/util.ts` & `src/stream/stream.ts`):
/// Maps public model ID + reasoning effort to backend runtime model ID.
pub fn resolve_runtime_model(model: &str, effort: Option<&str>) -> &'static str {
    match model {
        "gemini-3.8-flash" => match effort {
            Some("medium") => "gemini-3.8-flash-medium",
            Some("high") | Some("xhigh") => "gemini-3.8-flash-high",
            _ => "gemini-3.8-flash-low",
        },
        "gemini-3.7-flash" => match effort {
            Some("medium") => "gemini-3.7-flash-medium",
            Some("high") | Some("xhigh") => "gemini-3.7-flash-high",
            _ => "gemini-3.7-flash-low",
        },
        "gemini-3.6-flash" => match effort {
            Some("medium") => "gemini-3.6-flash-medium",
            Some("high") | Some("xhigh") => "gemini-3.6-flash-high",
            _ => "gemini-3.6-flash-low",
        },
        "gemini-3.5-flash" => match effort {
            Some("low") => "gemini-3.5-flash-low",
            Some("high") | Some("xhigh") => "gemini-3-flash-agent",
            _ => "gemini-3.5-flash-extra-low",
        },
        "gemini-3.1-pro" => match effort {
            Some("high") | Some("xhigh") => "gemini-pro-agent",
            _ => "gemini-3.1-pro-low",
        },
        "claude-opus-4-6" => "claude-opus-4-6-thinking",
        "claude-sonnet-4-6" => "claude-sonnet-4-6",
        "gpt-oss-120b" => "gpt-oss-120b-medium",
        _ => "gemini-3.8-flash-low",
    }
}

/// Matches ANTIGRAVITY_MODEL_ENUM in pi-antigravity `src/models/models.ts`.
pub fn model_enum_for(runtime_model: &str) -> Option<&'static str> {
    match runtime_model {
        "gemini-3.8-flash-high" | "gemini-3.8-flash" => Some("MODEL_PLACEHOLDER_M318"),
        "gemini-3.8-flash-medium" => Some("MODEL_PLACEHOLDER_M319"),
        "gemini-3.8-flash-low" => Some("MODEL_PLACEHOLDER_M320"),
        "gemini-3.8-flash-tiered" => Some("MODEL_PLACEHOLDER_M322"),
        "gemini-3.7-flash-high" | "gemini-3.7-flash" => Some("MODEL_PLACEHOLDER_M298"),
        "gemini-3.7-flash-medium" => Some("MODEL_PLACEHOLDER_M299"),
        "gemini-3.7-flash-low" => Some("MODEL_PLACEHOLDER_M300"),
        "gemini-3.7-flash-tiered" => Some("MODEL_PLACEHOLDER_M301"),
        "gemini-3.6-flash-high" | "gemini-3.6-flash" => Some("MODEL_PLACEHOLDER_M71"),
        "gemini-3.6-flash-medium" => Some("MODEL_PLACEHOLDER_M72"),
        "gemini-3.6-flash-low" => Some("MODEL_PLACEHOLDER_M73"),
        "gemini-3.6-flash-tiered" => Some("MODEL_PLACEHOLDER_M196"),
        "gemini-3.5-flash-low" | "gemini-3.5-flash" => Some("MODEL_PLACEHOLDER_M20"),
        "gemini-3.5-flash-extra-low" => Some("MODEL_PLACEHOLDER_M187"),
        "gemini-3-flash-agent" => Some("MODEL_PLACEHOLDER_M84"),
        "gemini-3.1-pro-low" | "gemini-3.1-pro" => Some("MODEL_PLACEHOLDER_M36"),
        "gemini-3.1-pro-high" => Some("MODEL_PLACEHOLDER_M37"),
        "gemini-pro-agent" => Some("MODEL_PLACEHOLDER_M16"),
        "claude-sonnet-4-6" => Some("MODEL_PLACEHOLDER_M35"),
        "claude-opus-4-6" | "claude-opus-4-6-thinking" => Some("MODEL_PLACEHOLDER_M26"),
        _ => None,
    }
}

/// The request body: pure agy CLI wire envelope, following pi's `buildRequest`
/// (pi-antigravity `src/stream/stream.ts:879-980`).
pub fn build_request(
    request: &lca_protocol::CompletionRequest,
    system: &str,
    project_id: &str,
    runtime_model: &str,
) -> serde_json::Value {
    let mut contents = Vec::new();
    let call_names: std::collections::HashMap<&str, &str> = request
        .messages
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .map(|call| (call.call_id.as_str(), call.name.as_str()))
        .collect();
    for message in &request.messages {
        let role = match message.role {
            lca_protocol::MessageRole::Tool => "user",
            lca_protocol::MessageRole::Assistant => "model",
            _ => "user",
        };
        let mut parts = Vec::new();
        let text: String = message
            .content
            .iter()
            .filter_map(|block| match block {
                lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if message.role == lca_protocol::MessageRole::Tool {
            let name = message
                .tool_call_id
                .as_deref()
                .and_then(|id| call_names.get(id).copied())
                .unwrap_or("tool");
            parts.push(serde_json::json!({
                "functionResponse": {
                    "name": name,
                    "response": { "result": text },
                }
            }));
        } else {
            if !text.is_empty() {
                parts.push(serde_json::json!({ "text": text }));
            }
            for block in &message.content {
                if let lca_protocol::ContentBlock::Image { media_type, bytes } = block {
                    parts.push(serde_json::json!({
                        "inlineData": {
                            "mimeType": media_type,
                            "data": lca_protocol::base64_encode(bytes),
                        }
                    }));
                }
            }
            for call in &message.tool_calls {
                let args: serde_json::Value = serde_json::from_str(&call.arguments)
                    .unwrap_or_else(|_| serde_json::json!({ "arguments": call.arguments }));
                parts.push(serde_json::json!({
                    "functionCall": { "id": call.call_id, "name": call.name, "args": args }
                }));
            }
        }
        if parts.is_empty() {
            continue;
        }
        contents.push(serde_json::json!({ "role": role, "parts": parts }));
    }
    let tools = if request.tools.is_empty() {
        serde_json::Value::Null
    } else {
        let declarations: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect();
        serde_json::json!([{ "functionDeclarations": declarations }])
    };

    let step = std::cmp::max(1, contents.len());
    let last_step_index = format!("{}", std::cmp::max(0, contents.len().saturating_sub(1)));
    let request_index = request
        .messages
        .iter()
        .filter(|m| m.role == lca_protocol::MessageRole::Assistant)
        .count();

    let trajectory_id = uuid_v4_like();
    let conv_id = uuid_v4_like();
    let is_claude = runtime_model.starts_with("claude-");
    let is_non_gemini = is_claude || runtime_model.starts_with("gpt-oss-");

    let mut labels = serde_json::json!({
        "last_step_index": last_step_index,
        "request_id": format!("{trajectory_id}-{request_index}"),
        "trajectory_id": trajectory_id,
        "used_claude": if is_claude { "true" } else { "false" },
        "used_claude_conservative": if is_claude { "true" } else { "false" },
        "used_non_gemini_model": if is_non_gemini { "true" } else { "false" },
    });
    if let Some(m_enum) = model_enum_for(runtime_model) {
        labels["model_enum"] = serde_json::Value::String(m_enum.to_string());
    }

    let mut inner_request = serde_json::json!({
        "contents": contents,
        "systemInstruction": {
            "role": "user",
            "parts": [{ "text": system }],
        },
        "sessionId": format!("{}", now_epoch() * 1000),
        "labels": labels,
    });
    if !tools.is_null() {
        inner_request["tools"] = tools;
    }

    serde_json::json!({
        "project": project_id,
        "model": runtime_model,
        "request": inner_request,
        "requestType": "agent",
        "userAgent": "antigravity",
        "requestId": format!("agent/{conv_id}/{}/{trajectory_id}/{step}", now_epoch() * 1000),
    })
}

fn uuid_v4_like() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

/// Decode one SSE frame of `streamGenerateContent` into typed events.
/// A `functionCall` part arrives whole, so it opens the call, carries
/// its arguments, and closes it in order (FR-PROV-7's shape).
fn handle_chunk(
    value: &serde_json::Value,
    open_calls: &mut Vec<String>,
    emit: &mut dyn FnMut(lca_protocol::StreamEvent) -> bool,
) -> bool {
    use lca_protocol::StreamEvent as E;
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("vendor error")
            .to_string();
        return emit(E::Error {
            message,
            retryable: false,
        });
    }
    if let Some(candidate) = value
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
    {
        for part in candidate {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                if part.get("thought").and_then(|t| t.as_bool()) == Some(true) {
                    if !emit(E::ReasoningDelta {
                        delta: text.to_string(),
                    }) {
                        return false;
                    }
                } else if !emit(E::TextDelta {
                    delta: text.to_string(),
                }) {
                    return false;
                }
            }
            if let Some(call) = part.get("functionCall") {
                let name = call
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let id = call
                    .get("id")
                    .and_then(|i| i.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("ag-{}", open_calls.len()));
                if !emit(E::ToolCallStart {
                    call_id: id.clone(),
                    name,
                }) {
                    return false;
                }
                open_calls.push(id.clone());
                let args = call
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                if !emit(E::ToolCallArgDelta {
                    call_id: id.clone(),
                    delta: args.to_string(),
                }) {
                    return false;
                }
                if !emit(E::ToolCallEnd { call_id: id }) {
                    return false;
                }
                open_calls.pop();
            }
        }
    }
    if let Some(usage) = value.get("usageMetadata") {
        let count = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
        let cache_read = count("cachedContentTokenCount");
        let prompt = count("promptTokenCount");
        let output = count("candidatesTokenCount") + count("thoughtsTokenCount");
        if prompt > 0 || output > 0 || cache_read > 0 {
            return emit(E::Usage {
                usage: lca_protocol::Usage {
                    input: prompt.saturating_sub(cache_read),
                    output,
                    cache_read,
                    cache_write: 0,
                    cache_write_1h: 0,
                    cost: 0.0,
                    cost_input: 0.0,
                    cost_cache_read: 0.0,
                    cost_cache_write: 0.0,
                    extras: Default::default(),
                },
            });
        }
    }
    true
}

/// A pull-based driver over one Antigravity streaming completion.
pub struct StreamDriver<'a> {
    cap: &'a dyn ProviderCap,
    handle: u32,
    buffer: String,
    open_calls: Vec<String>,
    pending: std::collections::VecDeque<lca_protocol::StreamEvent>,
    finished: bool,
}

impl<'a> StreamDriver<'a> {
    /// Authenticate, build the request, send it, and check the status.
    pub fn open(
        cap: &'a dyn ProviderCap,
        request: &lca_protocol::CompletionRequest,
    ) -> Result<StreamDriver<'a>, StreamFailure> {
        let token = access_token(cap).map_err(StreamFailure::from)?;
        let api_base = endpoint(cap, "api_base", DEFAULT_API_BASE);
        let system: String = request
            .messages
            .iter()
            .filter(|message| message.role == lca_protocol::MessageRole::System)
            .flat_map(|message| {
                message.content.iter().filter_map(|block| match block {
                    lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        let project_id = crate::stored(cap, "project");
        let effort = request.extras.get("reasoning-effort").map(String::as_str);
        let runtime_model = resolve_runtime_model(&request.model, effort);
        let body = build_request(request, &system, &project_id, runtime_model);
        let body_bytes = serde_json::to_vec(&body).map_err(|err| StreamFailure {
            message: format!("cannot build request: {err}"),
            class: "invalid",
            retryable: false,
        })?;
        let url = format!(
            "{}/v1internal:streamGenerateContent?alt=sse",
            api_base.trim_end_matches('/')
        );
        let bearer = format!("Bearer {token}");
        let headers = [
            ("content-type", "application/json"),
            ("authorization", bearer.as_str()),
            ("user-agent", DEFAULT_USER_AGENT),
        ];
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
            return Err(failure_for_status(status, &json_error_message(&text)));
        }
        Ok(StreamDriver {
            cap,
            handle,
            buffer: String::new(),
            open_calls: Vec::new(),
            pending: std::collections::VecDeque::new(),
            finished: false,
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
            match self.cap.net_read_body(self.handle, 64 * 1024) {
                Ok(Some(chunk)) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&chunk));
                    self.drain_frames();
                }
                Ok(None) => self.finished = true,
                Err(err) => {
                    self.finished = true;
                    return Some(Err(StreamFailure::from(err)));
                }
            }
        }
    }

    /// Decode every complete `\n\n`-terminated frame currently buffered.
    fn drain_frames(&mut self) {
        loop {
            let Some(end) = self.buffer.find("\n\n") else {
                return;
            };
            let frame: String = self.buffer.drain(..end + 2).collect();
            let mut events = Vec::new();
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
                handle_chunk(&value, &mut self.open_calls, &mut |event| {
                    events.push(event);
                    true
                });
            }
            self.pending.extend(events);
        }
    }
}

impl Drop for StreamDriver<'_> {
    fn drop(&mut self) {
        let _ = self.cap.net_close_response(self.handle);
    }
}
