//! The Responses request body and the usage reader: the wire shape Codex and
//! Grok speak, and nothing that touches the host.
//!
//! Extracted verbatim from `lca-subscription` (gh #189); provider differences
//! (endpoints, headers, account) stay in the extensions' spec tables, and the
//! credential lifecycle (the 401 purge) stays the caller's - it arrives as a
//! callback on the driver.

use lca_protocol::{CompletionRequest, ContentBlock, MessageRole, ToolSpec, Usage};

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
