//! The **ABI crossing**: the WIT records `provider`, `compaction`, and
//! `context-transform` exchange with the protocol types (ADR-0004's case-per-case
//! stream, ADR-0017's cost buckets, ADR-0029's typed content).

use lca_ext_abi::host::provider::exports::lca::ext::provider_completion as wit_completion;
use lca_ext_abi::host::provider::exports::lca::ext::provider_identity as wit_identity;
use lca_protocol::{CapabilityError, CompletionRequest, IdentityOutcome, ToolCall, Usage};

use crate::HostState;

pub(crate) fn role_str(role: lca_protocol::MessageRole) -> &'static str {
    use lca_protocol::MessageRole;
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

/// Protocol request -> the WIT record. Text and image blocks travel as
/// typed content (the ABI 0.2 `content-block` variant); reasoning blocks are
/// model-internal and not resent.
pub(crate) fn to_wit_request(request: &CompletionRequest) -> wit_completion::CompletionRequest {
    use wit_completion::{
        CompletionRequest as WitRequest, Message as WitMessage, ToolSpec as WitToolSpec,
    };
    let extra_pairs = |extras: &std::collections::BTreeMap<String, String>| {
        extras
            .iter()
            .map(|(key, value)| wit_completion::ExtraPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    };
    WitRequest {
        messages: request
            .messages
            .iter()
            .map(|message| WitMessage {
                role: role_str(message.role).to_string(),
                content: message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        lca_protocol::ContentBlock::Text { text } => Some(
                            lca_ext_abi::host::provider::lca::ext::types::ContentBlock::Text(
                                text.clone(),
                            ),
                        ),
                        lca_protocol::ContentBlock::Image { media_type, bytes } => Some(
                            lca_ext_abi::host::provider::lca::ext::types::ContentBlock::Image((
                                media_type.clone(),
                                bytes.clone(),
                            )),
                        ),
                        // Reasoning is model-internal and tool calls travel in
                        // their own field, so neither is resent as content.
                        lca_protocol::ContentBlock::Reasoning { .. }
                        | lca_protocol::ContentBlock::ToolCall { .. } => None,
                    })
                    .collect(),
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(
                        |call| lca_ext_abi::host::provider::lca::ext::types::ToolCall {
                            call_id: call.call_id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                            extras: Vec::new(),
                        },
                    )
                    .collect(),
                tool_call_id: message.tool_call_id.clone(),
                // The content-block variant has no reasoning case
                // (frozen), so a run's replay signature rides the
                // reserved map under THINKING_SIGNATURE_EXTRA and the
                // guest rebuilds the Reasoning block from it; message
                // extras cross verbatim (the reserved-map contract).
                extras: {
                    let mut pairs: Vec<wit_completion::ExtraPair> = extra_pairs(&message.extras);
                    if !pairs
                        .iter()
                        .any(|pair| pair.key == lca_protocol::THINKING_SIGNATURE_EXTRA)
                        && let Some(signature) =
                            message.content.iter().find_map(|block| match block {
                                lca_protocol::ContentBlock::Reasoning {
                                    signature: Some(signature),
                                    ..
                                } => Some(signature),
                                _ => None,
                            })
                    {
                        pairs.push(wit_completion::ExtraPair {
                            key: lca_protocol::THINKING_SIGNATURE_EXTRA.to_string(),
                            value: signature.clone(),
                        });
                    }
                    pairs
                },
            })
            .collect(),
        tools: request
            .tools
            .iter()
            .map(|tool| WitToolSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.to_string(),
                extras: extra_pairs(&tool.extras),
            })
            .collect(),
        model: request.model.clone(),
        stable_prefix: request.stable_prefix as u32,
        extras: extra_pairs(&request.extras),
    }
}

/// The WIT stream event -> the protocol event, case per case
/// (ADR-0004: one case each, `vendor-event` the reserved hatch).
pub(crate) fn from_wit_event(event: wit_completion::StreamEvent) -> lca_protocol::StreamEvent {
    use lca_protocol::StreamEvent as P;
    match event {
        wit_completion::StreamEvent::TextDelta(delta) => P::TextDelta { delta },
        wit_completion::StreamEvent::ReasoningDelta(delta) => P::ReasoningDelta { delta },
        wit_completion::StreamEvent::ToolCallStart((call_id, name)) => {
            P::ToolCallStart { call_id, name }
        }
        wit_completion::StreamEvent::ToolCallArgDelta((call_id, delta)) => {
            P::ToolCallArgDelta { call_id, delta }
        }
        wit_completion::StreamEvent::ToolCallEnd(call_id) => P::ToolCallEnd { call_id },
        wit_completion::StreamEvent::Usage(usage) => P::Usage {
            usage: from_wit_usage(usage),
        },
        wit_completion::StreamEvent::Error((message, retryable)) => P::Error { message, retryable },
        // Gh #390 gap 1: a guest re-encodes `P::Truncated` as a vendor event, since
        // the WIT stream has no case for it. Undo that here so the cut survives the
        // boundary - otherwise every provider delivered through the subscription kit
        // would report a truncated answer as a complete one, which is the exact
        // defect this cycle fixes, one hop away.
        wit_completion::StreamEvent::VendorEvent((kind, _))
            if kind == lca_protocol::TRUNCATED_KIND =>
        {
            P::Truncated
        }
        wit_completion::StreamEvent::VendorEvent((kind, payload)) => P::VendorEvent {
            kind,
            payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::String(payload)),
        },
    }
}

pub(crate) fn f64_extra(extras: &std::collections::BTreeMap<String, String>, key: &str) -> f64 {
    extras
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0)
}

/// WIT usage -> protocol usage. The cost buckets (ADR-0017) travel in
/// `extras`, since the WIT record predates the bucket split; the reserved
/// keys stay in `extras` too, where a consumer that does not know them
/// ignores them safely.
pub(crate) fn from_wit_usage(usage: wit_completion::Usage) -> Usage {
    let extras: std::collections::BTreeMap<String, String> = usage
        .extras
        .into_iter()
        .map(|pair| (pair.key, pair.value))
        .collect();
    Usage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        cache_write_1h: usage.cache_write_hour,
        cost: usage.cost,
        cost_input: f64_extra(&extras, "cost_input"),
        cost_cache_read: f64_extra(&extras, "cost_cache_read"),
        cost_cache_write: f64_extra(&extras, "cost_cache_write"),
        extras,
    }
}

pub(crate) fn from_wit_identity(outcome: wit_identity::IdentityOutcome) -> IdentityOutcome {
    match outcome {
        wit_identity::IdentityOutcome::Ok => IdentityOutcome::Ok,
        wit_identity::IdentityOutcome::NotSupported => IdentityOutcome::NotSupported,
        wit_identity::IdentityOutcome::Failed(reason) => IdentityOutcome::Failed(reason),
    }
}

// ---------------------------------------------------------------------------
// The `completion` capability: the host's side of ADR-0008's star
// ---------------------------------------------------------------------------

use lca_ext_abi::host::compaction::lca::host::completion as wit_cap_completion;

fn completion_error(err: CapabilityError) -> wit_cap_completion::Error {
    use wit_cap_completion::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(_) | CapabilityError::Timeout(_) => E::Io("unavailable".into()),
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
    }
}

/// `lca:host/types.message` -> the protocol message (the host package
/// keeps its own copies of the records; this is the crossing).
fn from_host_message(
    message: lca_ext_abi::host::compaction::lca::host::types::Message,
) -> lca_protocol::ChatMessage {
    use lca_protocol::{ChatMessage, ContentBlock, MessageRole};
    ChatMessage {
        role: match message.role.as_str() {
            "system" => MessageRole::System,
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            _ => MessageRole::Tool,
        },
        content: if message.content.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock::Text {
                text: message.content,
            }]
        },
        tool_calls: message
            .tool_calls
            .iter()
            .map(|call| ToolCall {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                parent_call_id: None,
            })
            .collect(),
        tool_call_id: message.tool_call_id,
        usage: None,
        extras: Default::default(),
    }
}

/// The protocol reply -> `lca:host/completion.response`.
fn to_host_response(text: String, usage: lca_protocol::Usage) -> wit_cap_completion::Response {
    let mut extras = usage
        .extras
        .iter()
        .map(
            |(key, value)| lca_ext_abi::host::compaction::lca::host::types::ExtraPair {
                key: key.clone(),
                value: value.clone(),
            },
        )
        .collect::<Vec<_>>();
    for (key, value) in [
        ("cost_input", usage.cost_input),
        ("cost_cache_read", usage.cost_cache_read),
        ("cost_cache_write", usage.cost_cache_write),
    ] {
        if value != 0.0 {
            extras.push(lca_ext_abi::host::compaction::lca::host::types::ExtraPair {
                key: key.to_string(),
                value: value.to_string(),
            });
        }
    }
    use lca_ext_abi::host::compaction::lca::host::types::Usage as HostUsage;
    wit_cap_completion::Response {
        text,
        usage: HostUsage {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cache_write_hour: usage.cache_write_1h,
            cost: usage.cost,
            extras,
        },
        extras: Vec::new(),
    }
}

impl wit_cap_completion::Host for HostState {
    fn request(
        &mut self,
        messages: Vec<lca_ext_abi::host::compaction::lca::host::types::Message>,
    ) -> Result<wit_cap_completion::Response, wit_cap_completion::Error> {
        let messages = messages.into_iter().map(from_host_message).collect();
        let (text, usage) = self.cap.complete(messages).map_err(completion_error)?;
        Ok(to_host_response(text, usage))
    }
}
