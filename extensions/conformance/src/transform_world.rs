wit_bindgen::generate!({
    path: "../../wit",
    world: "context-transform",
    export_macro_name: "export_transform",
    with: {
        "lca:host/log@0.7.0": generate,
        "lca:host/fs@0.7.0": generate,
        "lca:host/resources@0.7.0": generate,
        "lca:host/state@0.7.0": generate,
    },
});

use exports::lca::ext::transform::{Guest as TransformGuest, Message as WasmMessage};
use lca::ext::types::ToolCall as WasmToolCall;

pub struct TransformWasm;

impl TransformGuest for TransformWasm {
    fn transform(messages: Vec<WasmMessage>) -> Result<Vec<WasmMessage>, String> {
        let protocol: Vec<lca_protocol::ChatMessage> = messages
            .iter()
            .map(|message| lca_protocol::ChatMessage {
                role: match message.role.as_str() {
                    "system" => lca_protocol::MessageRole::System,
                    "user" => lca_protocol::MessageRole::User,
                    "assistant" => lca_protocol::MessageRole::Assistant,
                    _ => lca_protocol::MessageRole::Tool,
                },
                content: message
                    .content
                    .iter()
                    .map(|block| match block {
                        lca::ext::types::ContentBlock::Text(text) => {
                            lca_protocol::ContentBlock::Text { text: text.clone() }
                        }
                        lca::ext::types::ContentBlock::Image((media_type, bytes)) => {
                            lca_protocol::ContentBlock::Image {
                                media_type: media_type.clone(),
                                bytes: bytes.clone(),
                            }
                        }
                        // 0.7 (gh #352): a reasoning block crosses typed, with its replay
                        // signature; a guest that needs the signature reads it from here.
                        lca::ext::types::ContentBlock::Reasoning(block) => {
                            lca_protocol::ContentBlock::Reasoning {
                                reasoning: block.text.clone(),
                                signature: block.signature.clone(),
                            }
                        }
                    })
                    .collect(),
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(|call| lca_protocol::ToolCall {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        parent_call_id: None,
                    })
                    .collect(),
                tool_call_id: message.tool_call_id.clone(),
                usage: None,
                extras: Default::default(),
            })
            .collect();
        let transformed = crate::transform_script(protocol)?;
        Ok(transformed
            .into_iter()
            .map(|message| WasmMessage {
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
                        lca_protocol::ContentBlock::Text { text } => {
                            Some(lca::ext::types::ContentBlock::Text(text.clone()))
                        }
                        lca_protocol::ContentBlock::Image { media_type, bytes } => {
                            Some(lca::ext::types::ContentBlock::Image((
                                media_type.clone(),
                                bytes.clone(),
                            )))
                        }
                        // 0.7 (gh #352): the reasoning block crosses typed, so the
                        // fixture's transform path carries the signature too.
                        lca_protocol::ContentBlock::Reasoning {
                            reasoning,
                            signature,
                        } => Some(lca::ext::types::ContentBlock::Reasoning(
                            lca::ext::types::ReasoningBlock {
                                text: reasoning.clone(),
                                signature: signature.clone(),
                            },
                        )),
                        // Tool calls never cross (the host filters them); drop
                        // them here too.
                        lca_protocol::ContentBlock::ToolCall { .. } => None,
                    })
                    .collect(),
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(|call| WasmToolCall {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        extras: Vec::new(),
                    })
                    .collect(),
                tool_call_id: message.tool_call_id,
                extras: Vec::new(),
            })
            .collect())
    }
}

export_transform!(TransformWasm);
