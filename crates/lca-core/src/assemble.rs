//! Message assembly and attachment staging (S2): the outbound message list
//! from a session's records (compaction applied, forks followed), the
//! stable-prefix boundary's helpers, and the `/attach` staging path.
//!
//! Split out of `lib.rs`; the loop body calls [`assemble_with`] and the
//! fingerprint helpers in order.

use lca_protocol::{ChatMessage, ContentBlock, MessageRole, Record, Usage};
use lca_session::Session;

/// The resolved message list plus the stable-prefix boundary (FR-CACHE-5).
#[derive(Debug, Clone)]
pub struct Assembled {
    /// Messages, oldest first, system prompt leading.
    pub messages: Vec<ChatMessage>,
    /// Count of leading messages inside the stable cache boundary.
    pub stable_prefix: usize,
    /// Whether a compaction record appeared (FR-CACHE-5's anchor).
    pub compaction_seen: bool,
}

/// One resolved attachment: a media type and the bytes a provider carries.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// IANA media type (`image/png`, ...), from magic-byte sniffing.
    pub media_type: String,
    /// Raw bytes.
    pub bytes: Vec<u8>,
}

/// The result of staging one image file as a session attachment.
#[derive(Debug, Clone)]
pub struct StagedAttachment {
    /// The content hash (`sha256`), the record's attachment reference.
    pub hash: String,
    /// The text stub a provider (or a model without vision) can read.
    pub stub: String,
}

/// Read `path`, reject anything whose magic bytes are not a known image, and
/// write it into the session's content-addressed attachment store
/// (owner-only). Returns the hash and the model-visible stub text.
///
/// This is the `/attach`/`--attach` input path (ADR-0029). D8's rules hold:
/// the file name is the digest (no traversal), the media type is sniffed and
/// never taken from a user-controlled name, and the bytes are never
/// executable. A non-image is refused rather than attached as opaque text.
pub fn stage_image(session: &Session, path: &std::path::Path) -> Result<StagedAttachment, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let Some(media_type) = lca_protocol::sniff_image_media_type(&bytes) else {
        return Err(format!(
            "{} is not a recognized image (png, jpeg, gif, or webp)",
            path.display()
        ));
    };
    let hash = lca_tools::sha256_hex(&bytes);
    let dir = session.dir().join("attachments");
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    let target = dir.join(&hash);
    if !target.exists() {
        write_attachment(&target, &bytes)?;
    }
    let stub = format!(
        "[image attachment {}, {media_type}, {} bytes]",
        &hash[..8],
        bytes.len()
    );
    Ok(StagedAttachment { hash, stub })
}

/// Write one attachment file owner-only (0600 on Unix), never executable.
fn write_attachment(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    file.write_all(bytes)
        .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
    Ok(())
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
const COMPACTION_SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
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
                if let Some(reasoning) = reasoning {
                    message.content.insert(
                        0,
                        ContentBlock::Reasoning {
                            reasoning: reasoning.clone(),
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
                messages.push(ChatMessage::tool_result(call_id.clone(), text));
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
            ContentBlock::Reasoning { reasoning } => Some(reasoning.as_str()),
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
            ContentBlock::Reasoning { reasoning } => Some(reasoning.clone()),
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

#[cfg(test)]
mod vocabulary_tests {
    use super::*;
    use lca_protocol::FORMAT_VERSION;

    fn user(id: &str, content: &str) -> Record {
        Record::User {
            v: FORMAT_VERSION,
            ts: 1,
            id: id.to_string(),
            content: content.to_string(),
            attachments: vec![],
            queue: None,
        }
    }

    fn assistant(id: &str, text: &str) -> Record {
        Record::Assistant {
            v: FORMAT_VERSION,
            ts: 2,
            id: id.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            reasoning: None,
            model: None,
            provider: None,
            usage: None,
        }
    }

    fn texts(messages: &[ChatMessage]) -> Vec<(&MessageRole, String)> {
        messages
            .iter()
            .map(|m| {
                (
                    &m.role,
                    m.content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect(),
                )
            })
            .collect()
    }

    // A custom-message injects as a user message carrying its
    // provenance in extras; display=false still reaches the model
    // (display governs rendering, never visibility).
    #[test]
    fn a_custom_message_injects_as_a_user_message() {
        let records = vec![
            user("u1", "hi"),
            Record::CustomMessage {
                v: FORMAT_VERSION,
                ts: 2,
                id: "m1".into(),
                custom_type: "my-extension".into(),
                content: "Injected context...".into(),
                display: false,
                details: None,
            },
        ];
        let assembled = assemble(&records, "sys");
        let texts = texts(&assembled.messages);
        assert_eq!(texts.len(), 3);
        assert_eq!(texts[2].1, "Injected context...");
        let injected = &assembled.messages[2];
        assert_eq!(injected.role, MessageRole::User);
        assert_eq!(
            injected.extras.get("custom-type").map(String::as_str),
            Some("my-extension")
        );
        assert_eq!(
            injected.extras.get("display").map(String::as_str),
            Some("false")
        );
    }

    // A null edit omits the target; a string edit replaces its text.
    #[test]
    fn context_edits_omit_and_replace() {
        let records = vec![
            user("u1", "forgettable"),
            user("u2", "original"),
            Record::ContextEdit {
                v: FORMAT_VERSION,
                ts: 3,
                id: "e1".into(),
                target_id: "u1".into(),
                replacement: None,
            },
            Record::ContextEdit {
                v: FORMAT_VERSION,
                ts: 4,
                id: "e2".into(),
                target_id: "u2".into(),
                replacement: Some("revised".into()),
            },
        ];
        let assembled = assemble(&records, "sys");
        let texts = texts(&assembled.messages);
        assert_eq!(texts.len(), 2, "system plus the revised message: {texts:?}");
        assert_eq!(texts[1].1, "revised");
    }

    // The latest edit on a target wins; an assistant replacement keeps
    // the tool linkage the providers require.
    #[test]
    fn the_latest_edit_wins_and_keeps_tool_linkage() {
        let call = lca_protocol::ToolCall {
            call_id: "call_1".into(),
            name: "shell".into(),
            arguments: "{}".into(),
            parent_call_id: None,
        };
        let records = vec![
            assistant("a1", "working"),
            Record::ToolCall {
                v: FORMAT_VERSION,
                ts: 3,
                id: "c1".into(),
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                source: lca_protocol::ToolSource::Builtin,
            },
            Record::ToolResult {
                v: FORMAT_VERSION,
                ts: 4,
                id: "r1".into(),
                call_id: call.call_id.clone(),
                status: lca_protocol::ToolResultStatus::Ok,
                content: Some("ok".into()),
                attachment: None,
                truncated: false,
                exit_code: None,
                nested: Vec::new(),
                full_output_path: None,
            },
            Record::ContextEdit {
                v: FORMAT_VERSION,
                ts: 5,
                id: "e1".into(),
                target_id: "a1".into(),
                replacement: Some("first".into()),
            },
            Record::ContextEdit {
                v: FORMAT_VERSION,
                ts: 6,
                id: "e2".into(),
                target_id: "a1".into(),
                replacement: Some("second".into()),
            },
        ];
        let assembled = assemble(&records, "sys");
        let assistant = assembled
            .messages
            .iter()
            .find(|m| m.role == MessageRole::Assistant)
            .expect("the assistant message survives its edit");
        let text: String = assistant
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "second", "the latest edit wins");
        assert_eq!(assistant.tool_calls.len(), 1, "tool linkage survives");
        assert!(
            assembled
                .messages
                .iter()
                .any(|m| m.role == MessageRole::Tool)
        );
    }

    // Usage, labels, session names, extension state, and the switch
    // records change nothing about the assembled content.
    #[test]
    fn bookkeeping_records_alter_no_content() {
        let records = vec![
            user("u1", "hi"),
            Record::ModelChange {
                v: FORMAT_VERSION,
                ts: 2,
                id: "c1".into(),
                from: None,
                to: "m".into(),
                provider: "p".into(),
                profile: None,
            },
            Record::ThinkingLevelChange {
                v: FORMAT_VERSION,
                ts: 3,
                id: "t1".into(),
                level: "high".into(),
            },
            Record::Usage {
                v: FORMAT_VERSION,
                ts: 4,
                id: "g1".into(),
                kind: "cache_warm".into(),
                provider: None,
                model: None,
                usage: Usage::default(),
            },
            Record::Label {
                v: FORMAT_VERSION,
                ts: 5,
                id: "l1".into(),
                target_id: "u1".into(),
                label: Some("checkpoint-1".into()),
            },
            Record::SessionInfo {
                v: FORMAT_VERSION,
                ts: 6,
                id: "s1".into(),
                name: "Refactor auth module".into(),
            },
            Record::Custom {
                v: FORMAT_VERSION,
                ts: 7,
                id: "x1".into(),
                custom_type: "my-extension".into(),
                data: serde_json::json!({"count": 42}),
            },
        ];
        let assembled = assemble(&records, "sys");
        let texts = texts(&assembled.messages);
        assert_eq!(
            texts,
            vec![
                (&MessageRole::System, "sys".to_string()),
                (&MessageRole::User, "hi".to_string()),
            ]
        );
    }
}

#[cfg(test)]
mod healing_tests {
    use super::*;
    use lca_protocol::ToolCall;

    fn assistant_with_call(call_id: &str) -> ChatMessage {
        let mut message = ChatMessage::text(MessageRole::Assistant, "working");
        message.tool_calls.push(ToolCall {
            call_id: call_id.to_string(),
            name: "shell".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        });
        message.content.push(ContentBlock::ToolCall {
            call_id: call_id.to_string(),
            name: "shell".to_string(),
            arguments: "{}".to_string(),
        });
        message
    }

    // A dangling call (the iteration-limit / cancel shape) gains a tool
    // result, so the next request is valid.
    #[test]
    fn a_dangling_tool_call_is_healed() {
        let mut messages = vec![
            ChatMessage::text(MessageRole::User, "hi"),
            assistant_with_call("call_1"),
            ChatMessage::text(MessageRole::User, "next"),
        ];
        heal_dangling_tool_calls(&mut messages);
        let tool = messages
            .iter()
            .find(|m| m.role == MessageRole::Tool)
            .expect("a synthetic tool result");
        assert_eq!(tool.tool_call_id.as_deref(), Some("call_1"));
        // It sits before the next user message.
        let tool_at = messages
            .iter()
            .position(|m| m.role == MessageRole::Tool)
            .unwrap();
        let user_at = messages
            .iter()
            .rposition(|m| m.role == MessageRole::User)
            .unwrap();
        assert!(tool_at < user_at);
    }

    // A normal turn is untouched (no extra messages, order preserved).
    #[test]
    fn answered_calls_are_not_touched() {
        let mut messages = vec![
            assistant_with_call("call_1"),
            ChatMessage::tool_result("call_1", "ok"),
            ChatMessage::text(MessageRole::Assistant, "done"),
        ];
        let before = messages.len();
        heal_dangling_tool_calls(&mut messages);
        assert_eq!(messages.len(), before);
        assert_eq!(messages[1].role, MessageRole::Tool);
    }

    // One of two calls answered: only the missing one is filled.
    #[test]
    fn only_the_missing_call_is_filled() {
        let mut messages = vec![
            assistant_with_call("call_1"),
            ChatMessage::tool_result("call_1", "ok"),
        ];
        messages[0].tool_calls.push(lca_protocol::ToolCall {
            call_id: "call_2".to_string(),
            name: "shell".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        });
        heal_dangling_tool_calls(&mut messages);
        let ids: Vec<Option<&str>> = messages
            .iter()
            .filter(|m| m.role == MessageRole::Tool)
            .map(|m| m.tool_call_id.as_deref())
            .collect();
        assert_eq!(ids, vec![Some("call_1"), Some("call_2")]);
    }
}
