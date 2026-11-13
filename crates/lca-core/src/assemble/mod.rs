//! Message assembly (S2): the outbound message list from a session's records
//! (compaction applied, forks followed), the stable-prefix helpers the loop
//! calls in order, and the `/attach` staging path.
//!
//! Attachment staging lives in `attachments.rs` (gh #113: bytes in, content
//! digest out, one create-race arm per twin); `docs/session-log-format.md` is
//! the record shape both halves read.

use lca_protocol::{ChatMessage, ContentBlock, MessageRole, Record, Usage};

pub use self::attachments::{Attachment, AttachmentError, StagedAttachment};
pub use self::attachments::{
    attachment_hashes, read_attachment_async, stage_image, store_image_bytes,
    store_image_bytes_async,
};

mod attachments;

#[cfg(test)]
#[path = "healing_tests.rs"]
mod healing_tests;
#[cfg(test)]
#[path = "vocabulary_tests.rs"]
mod vocabulary_tests;

/// The resolved message list plus the stable-prefix boundary (FR-CACHE-5).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Assembled {
    /// Messages, oldest first, system prompt leading.
    pub messages: Vec<ChatMessage>,
    /// Count of leading messages inside the stable cache boundary.
    pub stable_prefix: usize,
    /// Whether a compaction record appeared (FR-CACHE-5's anchor).
    pub compaction_seen: bool,
}

/// Build the outbound message list from a session's resolved (display-view)
/// records, with no attachment resolver: image attachments appear only as
/// the text stub the attach path recorded in the message content.
pub fn assemble(records: &[Record], system_prompt: &str) -> Assembled {
    assemble_with(records, system_prompt, &|_| None)
}

/// Self-describing framing around a compaction summary, matching pi
/// (`packages/coding-agent/src/core/messages.ts`). The wire role stays
/// `user` (no ABI change); the framing is what tells the model this is its
/// own compacted memory rather than a note from the user.
pub(crate) const COMPACTION_SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";

/// Build the outbound message list from a session's resolved (display-view)
/// records: compaction applied, forks followed, transforms not yet run.
///
/// `resolve` turns an attachment hash into the bytes a provider needs; a
/// `user` record's image attachments become `ContentBlock::Image` blocks
/// after their text (ADR-0029). A hash the resolver does not know is skipped
/// rather than an error: the message's text stub already says it exists.
pub fn assemble_with(
    records: &[Record],
    system_prompt: &str,
    resolve: &dyn Fn(&str) -> Option<Attachment>,
) -> Assembled {
    let mut messages = vec![ChatMessage::text(MessageRole::System, system_prompt)];
    let mut stable_prefix = 0usize;
    let mut compaction_seen = false;
    // The latest edit per target wins (pi's `context_edit` rule); the
    // scan runs first so emission only ever consults this map.
    let mut edits: std::collections::HashMap<&str, Option<&str>> = std::collections::HashMap::new();
    for record in records {
        if let Record::ContextEdit {
            target_id,
            replacement,
            ..
        } = record
        {
            edits.insert(target_id.as_str(), replacement.as_deref());
        }
    }
    for record in records {
        match record {
            Record::SessionStart { .. }
            | Record::SessionEnd { .. }
            | Record::ForkPoint { .. }
            // A branch jump is navigation machinery, not content: the
            // walk already passed through it to reach this chain (gh
            // #37, ADR-0046).
            | Record::BranchPoint { .. }
            | Record::Permission { .. }
            | Record::ExtensionEvent { .. }
            // A model switch is history for a reader, not content for the
            // model (gh #8): the next request runs on the new model
            // anyway, which the request itself names.
            | Record::ModelChange { .. }
            // A thinking-level switch applies to the next request, which
            // names it; usage is accounting, not content; labels name
            // bookmarks for readers; session-info names the session;
            // extension state never enters model context (all gh #47).
            | Record::ThinkingLevelChange { .. }
            | Record::Usage { .. }
            | Record::Label { .. }
            | Record::SessionInfo { .. }
            | Record::Custom { .. } => {}
            Record::ContextEdit { .. } => {
                // Edits have no message of their own; they shaped the
                // messages above through the pre-scan.
            }
            Record::CustomMessage {
                id,
                custom_type,
                content,
                display,
                ..
            } => {
                if matches!(edits.get(id.as_str()), Some(None)) {
                    continue;
                }
                let text = match edits.get(id.as_str()) {
                    Some(Some(replacement)) => (*replacement).to_string(),
                    _ => content.clone(),
                };
                let mut message = ChatMessage::text(MessageRole::User, text);
                // The injection's provenance travels in `extras`, the
                // reserved map transforms already receive: which
                // extension spoke, and whether the interface shows it.
                message.extras.insert(
                    "custom-type".to_string(),
                    custom_type.clone(),
                );
                message
                    .extras
                    .insert("display".to_string(), display.to_string());
                messages.push(message);
            }
            Record::User {
                id,
                content,
                attachments,
                queue,
                ..
            } => {
                if matches!(edits.get(id.as_str()), Some(None)) {
                    continue;
                }
                let text = match edits.get(id.as_str()) {
                    Some(Some(replacement)) => (*replacement).to_string(),
                    _ => content.clone(),
                };
                let mut message = ChatMessage::text(MessageRole::User, text);
                // ADR-0038: the submit-mode marker travels in `extras`, the
                // reserved map extensions already receive in `transform`.
                if let Some(marker) = queue {
                    message.extras.insert("queue".to_string(), marker.clone());
                }
                for hash in attachments {
                    if let Some(attachment) = resolve(hash) {
                        message.content.push(ContentBlock::Image {
                            media_type: attachment.media_type,
                            bytes: attachment.bytes,
                        });
                    }
                }
                messages.push(message);
            }
            Record::Assistant {
                id,
                content,
                reasoning,
                reasoning_signature,
                ..
            } => {
                if matches!(edits.get(id.as_str()), Some(None)) {
                    continue;
                }
                let body: Vec<ContentBlock> = match edits.get(id.as_str()) {
                    // A replacement swaps the text the model sees for one
                    // block; reasoning and tool linkage stay untouched.
                    Some(Some(replacement)) => content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text { .. } => None,
                            other => Some(other.clone()),
                        })
                        .chain(std::iter::once(ContentBlock::Text {
                            text: (*replacement).to_string(),
                        }))
                        .collect(),
                    _ => content.clone(),
                };
                let mut message = ChatMessage {
                    role: MessageRole::Assistant,
                    content: body,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    usage: None,
                    extras: Default::default(),
                };
                // A signature without text still earns its block:
                // Gemini's Cloud Code stream signs function calls but
                // sends no thought text, and vendors that require
                // signatures reject transcripts that drop them (gh #41:
                // empty+signature travels as redacted_thinking where
                // that shape exists, and is inert elsewhere).
                if reasoning.is_some() || reasoning_signature.is_some() {
                    message.content.insert(
                        0,
                        ContentBlock::Reasoning {
                            reasoning: reasoning.clone().unwrap_or_default(),
                            // gh #41: the signature rides the block so
                            // replay resends it verbatim (vendors that
                            // require signatures reject transcripts that
                            // drop them).
                            signature: reasoning_signature.clone(),
                        },
                    );
                }
                messages.push(message);
            }
            Record::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                // Tool calls belong to the assistant message that requested
                // them; the log keeps them as their own records.
                if let Some(assistant) = messages
                    .iter_mut()
                    .rev()
                    .find(|m| m.role == MessageRole::Assistant)
                {
                    assistant.content.push(ContentBlock::ToolCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                    assistant.tool_calls.push(lca_protocol::ToolCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                        parent_call_id: None,
                    });
                }
            }
            Record::ToolResult {
                id,
                call_id,
                content,
                attachment,
                truncated,
                images,
                ..
            } => {
                if matches!(edits.get(id.as_str()), Some(None)) {
                    continue;
                }
                let mut text = match edits.get(id.as_str()) {
                    Some(Some(replacement)) => (*replacement).to_string(),
                    _ => content
                        .clone()
                        .or_else(|| {
                            attachment
                                .clone()
                                .map(|hash| format!("[attachment {hash}]"))
                        })
                        .unwrap_or_default(),
                };
                if *truncated {
                    text.push_str("\n[result truncated]");
                }
                let mut message = ChatMessage::tool_result(call_id.clone(), text);
                // Gh #113: persisted image hashes resolve like user
                // attachments; an unresolvable hash is skipped without
                // failing the turn.
                for hash in images {
                    if let Some(attachment) = resolve(hash) {
                        message.content.push(ContentBlock::Image {
                            media_type: attachment.media_type,
                            bytes: attachment.bytes,
                        });
                    }
                }
                messages.push(message);
            }
            Record::BranchSummary { summary, .. } => {
                // An abandoned path's lesson, injected where the new
                // branch continues (pi's branch-summary shape, gh #37):
                // framed as context, like a compaction summary.
                messages.push(ChatMessage::text(
                    MessageRole::User,
                    format!("abandoned branch summary: {summary}"),
                ));
            }
            Record::Compaction { summary, .. } => {
                compaction_seen = true;
                // The summary stands in for the range it replaced
                // (session-log-format: the reader substitutes it), and
                // everything through it becomes the stable prefix
                // (FR-CACHE-5, ADR-0017). The framing makes the model read
                // it as its own memory, not as a user note.
                messages.push(ChatMessage::text(
                    MessageRole::User,
                    format!("{COMPACTION_SUMMARY_PREFIX}{summary}{COMPACTION_SUMMARY_SUFFIX}"),
                ));
                stable_prefix = messages.len();
            }
        }
    }
    heal_dangling_tool_calls(&mut messages);
    Assembled {
        messages,
        stable_prefix,
        compaction_seen,
    }
}

/// Give every assistant `tool_call` a matching `tool` message when the turn
/// that requested it ended abnormally — the iteration limit (FR-CORE-9),
/// cancellation mid-batch, or a crash. OpenAI-shaped endpoints reject a
/// request whose history holds a tool call with no response (HTTP 400), and
/// because the session log is append-only, that rejection would poison every
/// later turn: the session becomes permanently unusable with an opaque
/// "unknown error". Healing here fixes new sessions and already-poisoned
/// ones, and is a no-op for a normal turn (every call already has a result).
fn heal_dangling_tool_calls(messages: &mut Vec<ChatMessage>) {
    use std::collections::HashSet;
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role != MessageRole::Assistant || messages[i].tool_calls.is_empty() {
            i += 1;
            continue;
        }
        // The `tool` messages that immediately follow answer some or all of
        // this assistant's calls.
        let mut end = i + 1;
        let mut answered: HashSet<String> = HashSet::new();
        while end < messages.len() && messages[end].role == MessageRole::Tool {
            if let Some(id) = &messages[end].tool_call_id {
                answered.insert(id.clone());
            }
            end += 1;
        }
        let missing: Vec<String> = messages[i]
            .tool_calls
            .iter()
            .filter(|call| !answered.contains(&call.call_id))
            .map(|call| call.call_id.clone())
            .collect();
        let mut insert_at = end;
        for call_id in missing {
            messages.insert(
                insert_at,
                ChatMessage::tool_result(
                    call_id,
                    "turn ended before this tool call ran (iteration limit, cancellation, or a crash)"
                        .to_string(),
                ),
            );
            insert_at += 1;
        }
        i = insert_at;
    }
}

/// One request's prompt token count, the number FR-CACHE-1 compares.
/// The compaction cut planner reads it too (gh #36 phase 1).
pub(crate) fn usage_prompt_tokens(usage: &Usage) -> u64 {
    usage.input + usage.cache_read + usage.cache_write + usage.cache_write_1h
}

/// All text a message carries (the comparison key for finding this
/// turn's own user message).
pub(super) fn message_text(message: &ChatMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::Reasoning { reasoning, .. } => Some(reasoning.as_str()),
            ContentBlock::ToolCall { .. } => None,
            // The image's stub text is already in the message content.
            ContentBlock::Image { .. } => None,
        })
        .collect()
}

/// What one message looks like on the wire, for FR-CACHE-6's
/// previous-versus-current comparison: role, text, and tool calls.
pub(super) fn stable_fingerprint(message: &ChatMessage) -> String {
    let text: String = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            ContentBlock::Reasoning { reasoning, .. } => Some(reasoning.clone()),
            // An image changes the wire bytes, so its content hash belongs in
            // the fingerprint (a length-only key would miss a same-size swap).
            ContentBlock::Image { media_type, bytes } => Some(format!(
                "[image {media_type} {}]",
                lca_tools::sha256_hex(bytes)
            )),
            ContentBlock::ToolCall { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let calls: Vec<String> = message
        .tool_calls
        .iter()
        .map(|call| format!("{}:{}:{}", call.call_id, call.name, call.arguments))
        .collect();
    format!("{:?}|{}|{:?}", message.role, text, calls)
}
