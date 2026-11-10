//! The Anthropic Messages **request builder** (`POST /v1/messages`, gh #189):
//! the system prompt, messages with tool_use/tool_result round-trips, tools with
//! prompt-cache breakpoints, and the breakpoint helper they share.
//!
//! The SSE decoder - text, thinking, input-JSON tool deltas, usage buckets - is
//! [`super::stream`]'s half of the same engine, and `parse_sse` is re-exported
//! here for the one-shot callers. Grammar port from pi's `anthropic-messages.ts`
//! event table; no consumer wires it to a turn yet (that arrives with the
//! `anthropic` provider, #183), so this kit maps onto [`StreamEvent`], the host
//! vocabulary every kit shares.
//!
//! Image blocks map to Anthropic's base64 source blocks like the OpenAI kit's
//! vision parts. Thinking signatures are the decoder's business (see
//! [`super::stream`]): they ride [`StreamEvent::VendorEvent`] with kind
//! `"thinking-signature"` until #41 teaches the host to resend them.

use lca_protocol::{CompletionRequest, ContentBlock, MessageRole, ToolSpec};

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
                let blocks = content_blocks(&message.content);
                if blocks.is_empty() {
                    continue;
                }
                // No breakpoint here: see the single post-loop anchor below.
                // gh #334: planting one per user message grew the count
                // linearly with history and broke Anthropic's 4-cap.
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
                // Gh #113: images serialize natively inside the
                // tool result (the text+image block array); text-only
                // results keep the string form.
                let has_image = message
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::Image { .. }));
                let content = if has_image {
                    let mut blocks = Vec::new();
                    if !text.is_empty() {
                        blocks.push(serde_json::json!({ "type": "text", "text": text }));
                    }
                    for block in &message.content {
                        if let ContentBlock::Image { media_type, bytes } = block {
                            blocks.push(serde_json::json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": media_type,
                                    "data": lca_protocol::base64_encode(bytes),
                                },
                            }));
                        }
                    }
                    serde_json::Value::Array(blocks)
                } else {
                    serde_json::Value::String(text)
                };
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": message.tool_call_id.as_deref().unwrap_or(""),
                        "content": content,
                    }],
                }));
            }
        }
    }
    // gh #334: pi's THIRD cache anchor — the final block of the LAST
    // serialized message — attached once, here, rather than inside the
    // loop. This is deliberately outside the role match so a request whose
    // last message is a tool result (serialized as `role: user` +
    // `tool_result`, which the old code never reached) still gets it.
    //
    // Anthropic caps a request at FOUR `cache_control` breakpoints; this
    // plus the system prompt and the last tool declaration is three, which
    // leaves headroom. Do not attach another here.
    if cache_breakpoints
        && let Some(last_message) = messages.last_mut()
        && let Some(last_block) = last_message["content"]
            .as_array_mut()
            .and_then(|blocks| blocks.last_mut())
    {
        last_block["cache_control"] = breakpoint();
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
///
/// gh #335: Anthropic requires `thinking`/`redacted_thinking` blocks to
/// PRECEDE every other assistant content block — a replay that puts text
/// first is rejected with "thinking blocks must precede all other content
/// blocks". So reasoning is emitted first (relative order preserved,
/// signatures verbatim), then the merged text, then images. The merged
/// text shape is kept: only the *ordering* is the API's requirement.
fn content_blocks(content: &[ContentBlock]) -> Vec<serde_json::Value> {
    let mut blocks = Vec::new();
    for block in content {
        if let ContentBlock::Reasoning {
            reasoning,
            signature,
        } = block
        {
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
    }
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
        if let ContentBlock::Image { media_type, bytes } = block {
            blocks.push(serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type,
                    "data": lca_protocol::base64_encode(bytes),
                },
            }));
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
