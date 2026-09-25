//! Chat messages as they travel to a provider and back.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::tool::ToolCall;
use crate::usage::Usage;

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// System instructions.
    System,
    /// User input.
    User,
    /// Model output.
    Assistant,
    /// A tool result attached to a preceding tool call.
    Tool,
}

/// One block of message content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// Model reasoning, shown separately from the answer.
    Reasoning {
        /// The reasoning text.
        reasoning: String,
    },
    /// A tool call the model requested.
    ToolCall {
        /// Provider-side call identifier.
        call_id: String,
        /// Tool name.
        name: String,
        /// Argument string, JSON object text, accumulated by the host
        /// (ADR-0004: the extension emits deltas, the host joins them).
        arguments: String,
    },
    /// An image attached to a message: a media type and the raw bytes
    /// (ADR-0029, the typed content the ABI 0.2 window adds).
    Image {
        /// IANA media type, e.g. `image/png`.
        media_type: String,
        /// Raw image bytes.
        bytes: Vec<u8>,
    },
}

/// The media type of an image from its magic bytes, or `None` when the bytes
/// are not a format the host recognizes. Sniffing is deliberately a small
/// allow-list: a provider must never be handed bytes whose type came from a
/// user-controlled file name (D8's rule), and a media type the host does not
/// recognize is carried as text instead.
pub fn sniff_image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Standard base64 (RFC 4648 §4) with padding. Providers need it to build a
/// `data:` URI or an inline image part, and a hand-rolled encoder keeps the
/// closed dependency list closed (no `base64` crate).
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// One message in the resolved list handed to a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Author role.
    pub role: MessageRole,
    /// Content blocks in order.
    pub content: Vec<ContentBlock>,
    /// Tool calls, when the role is `assistant` and the provider's wire
    /// format carries calls as a separate field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// `tool_call_id` this message answers, when the role is `tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Recorded usage when this message is a stored assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Reserved map for non-structural extensions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
}

impl ChatMessage {
    /// A system, user, or assistant message with one text block.
    pub fn text(role: MessageRole, text: impl Into<String>) -> Self {
        ChatMessage {
            role,
            content: vec![ContentBlock::Text { text: text.into() }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: BTreeMap::new(),
        }
    }

    /// A tool result message answering `call_id`.
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        ChatMessage {
            role: MessageRole::Tool,
            content: vec![ContentBlock::Text {
                text: content.into(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
            usage: None,
            extras: BTreeMap::new(),
        }
    }

    /// Concatenated text of every text block, for prompts and tests.
    pub fn plain_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}
