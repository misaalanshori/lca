//! Split from `lib.rs` (cycle 7, P3). Behaviour unchanged.

use super::*;

use lca_ext_abi::host::compaction::exports::lca::ext as compaction_exports;
use lca_ext_abi::host::context_transform::exports::lca::ext as transform_exports;

// ---------------------------------------------------------------------------
// Compaction and context-transform work
// ---------------------------------------------------------------------------

fn to_wit_session_record(
    record: &lca_protocol::Record,
) -> compaction_exports::compact::SessionRecord {
    compaction_exports::compact::SessionRecord {
        kind: record.type_tag().to_string(),
        id: record.id().unwrap_or_default().to_string(),
        body: serde_json::to_string(record).unwrap_or_default(),
        extras: Vec::new(),
    }
}

pub(super) fn compact_work(
    inner: &Inner,
    records: Vec<lca_protocol::Record>,
) -> Result<String, CallError> {
    let (mut store, instance) = inner.checkout_compaction()?;
    let wit_records: Vec<_> = records.iter().map(to_wit_session_record).collect();
    let summary = instance
        .lca_ext_compact()
        .call_compact(&mut store, &wit_records)
        .map_err(|err| inner.classify(err))?;
    // A refusal is the guest's healthy verdict, not a poisoned guest:
    // the instance stays cached (#103).
    let summary = match summary {
        Ok(summary) => summary,
        Err(reason) => {
            inner.checkin_compaction(store, instance);
            return Err(CallError::InvalidArguments(format!(
                "compaction refused: {reason}"
            )));
        }
    };
    inner.checkin_compaction(store, instance);
    Ok(summary)
}

/// Protocol messages -> the `context-transform` world's WIT records.
fn to_wit_messages(
    messages: &[lca_protocol::ChatMessage],
) -> Vec<transform_exports::transform::Message> {
    use transform_exports::transform::Message as WitMessage;
    messages
        .iter()
        .map(|message| WitMessage {
            role: match message.role {
                lca_protocol::MessageRole::System => "system".to_string(),
                lca_protocol::MessageRole::User => "user".to_string(),
                lca_protocol::MessageRole::Assistant => "assistant".to_string(),
                lca_protocol::MessageRole::Tool => "tool".to_string(),
            },
            content: message
                .content
                .iter()
                .filter_map(|block| match block {
                    lca_protocol::ContentBlock::Text { text } => Some(
                        lca_ext_abi::host::context_transform::lca::ext::types::ContentBlock::Text(
                            text.clone(),
                        ),
                    ),
                    lca_protocol::ContentBlock::Image { media_type, bytes } => Some(
                        lca_ext_abi::host::context_transform::lca::ext::types::ContentBlock::Image(
                            (media_type.clone(), bytes.clone()),
                        ),
                    ),
                    lca_protocol::ContentBlock::Reasoning { .. }
                    | lca_protocol::ContentBlock::ToolCall { .. } => None,
                })
                .collect(),
            tool_calls: message
                .tool_calls
                .iter()
                .map(
                    |call| lca_ext_abi::host::context_transform::lca::ext::types::ToolCall {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        extras: Vec::new(),
                    },
                )
                .collect(),
            tool_call_id: message.tool_call_id.clone(),
            extras: message
                .extras
                .iter()
                .map(|(key, value)| {
                    lca_ext_abi::host::context_transform::lca::ext::types::ExtraPair {
                        key: key.clone(),
                        value: value.clone(),
                    }
                })
                .collect(),
        })
        .collect()
}

fn from_wit_messages(
    messages: Vec<transform_exports::transform::Message>,
) -> Vec<lca_protocol::ChatMessage> {
    messages
        .into_iter()
        .map(|message| lca_protocol::ChatMessage {
            role: match message.role.as_str() {
                "system" => lca_protocol::MessageRole::System,
                "user" => lca_protocol::MessageRole::User,
                "assistant" => lca_protocol::MessageRole::Assistant,
                _ => lca_protocol::MessageRole::Tool,
            },
            content: message
                .content
                .into_iter()
                .map(|block| match block {
                    lca_ext_abi::host::context_transform::lca::ext::types::ContentBlock::Text(
                        text,
                    ) => lca_protocol::ContentBlock::Text { text },
                    lca_ext_abi::host::context_transform::lca::ext::types::ContentBlock::Image(
                        (media_type, bytes),
                    ) => lca_protocol::ContentBlock::Image { media_type, bytes },
                })
                .collect(),
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
            extras: message
                .extras
                .into_iter()
                .map(|pair| (pair.key, pair.value))
                .collect(),
        })
        .collect()
}

/// One transform pass: the guest's `Err(reason)` is the rejection
/// (FR-CTX-3), not a host failure.
pub(super) fn transform_work(
    inner: &Inner,
    messages: Vec<lca_protocol::ChatMessage>,
) -> Result<Result<Vec<lca_protocol::ChatMessage>, String>, CallError> {
    let (mut store, instance) = inner.checkout_transform()?;
    let wit_messages = to_wit_messages(&messages);
    let outcome = instance
        .lca_ext_transform()
        .call_transform(&mut store, &wit_messages)
        .map_err(|err| inner.classify(err))?;
    // A rejection is the guest's healthy verdict (FR-CTX-3), not a
    // poisoned guest: the instance stays cached (#103).
    inner.checkin_transform(store, instance);
    Ok(match outcome {
        Ok(list) => Ok(from_wit_messages(list)),
        Err(reason) => Err(reason),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The submit-mode marker (ADR-0038) must survive the WIT boundary, so a
    // context-transform extension can see `extras["queue"]`.
    #[test]
    fn message_extras_round_trip_through_the_wit_boundary() {
        let mut message =
            lca_protocol::ChatMessage::text(lca_protocol::MessageRole::User, "steer me");
        message
            .extras
            .insert("queue".to_string(), "steer".to_string());
        let wit = to_wit_messages(std::slice::from_ref(&message));
        assert_eq!(wit[0].extras.len(), 1);
        assert_eq!(wit[0].extras[0].key, "queue");
        assert_eq!(wit[0].extras[0].value, "steer");
        let back = from_wit_messages(wit);
        assert_eq!(
            back[0].extras.get("queue").map(String::as_str),
            Some("steer")
        );
    }
}
