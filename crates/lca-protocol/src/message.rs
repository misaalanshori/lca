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
