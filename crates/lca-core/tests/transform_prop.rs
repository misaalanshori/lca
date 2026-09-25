//! Property-based invariants for the `context-transform` chain (testing plan
//! section 8): for any generated chain of transforms and any generated message
//! list, the chain preserves the input's order and appends in registration
//! order, and a rejection at any position stops every transform after it.

use std::sync::Arc;

use lca_core::ExtensionRegistry;
use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_protocol::{
    ChatMessage, CommandEffect, CommandSpec, ContentBlock, DispatchError, HookAction, MessageRole,
    PostToolObservation, ToolCall, ToolResult, ToolSpec,
};
use proptest::prelude::*;

/// A transform that either rejects or appends its own marker, so the chain's
/// order is readable from the output.
struct MarkerTransform {
    name: String,
    marker: String,
    reject: bool,
}

impl ExtensionDispatch for MarkerTransform {
    fn name(&self) -> &str {
        &self.name
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::ContextTransform]
    }
    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name.clone(),
            world: "tool",
        })
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.name.clone(),
            world: "tool",
        })))
    }
    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name.clone(),
            world: "command",
        })
    }
    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name.clone(),
            world: "command",
        })
    }
    fn on_pre_turn(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(HookAction::Allow)))
    }
    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a PostToolObservation,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_session_close(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn transform_messages(
        &self,
        mut messages: Vec<ChatMessage>,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<Result<Vec<ChatMessage>, String>, DispatchError>>
    {
        if self.reject {
            return Box::pin(std::future::ready(Ok(Err(
                "rejected by property".to_string()
            ))));
        }
        messages.push(ChatMessage::text(MessageRole::User, self.marker.clone()));
        Box::pin(std::future::ready(Ok(Ok(messages))))
    }
}

fn text_of(message: &ChatMessage) -> Option<String> {
    message.content.iter().find_map(|block| match block {
        ContentBlock::Text { text } => Some(text.clone()),
        _ => None,
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn a_transform_chain_preserves_order_and_a_rejection_stops_it(
        chain_len in 1usize..6,
        reject_index in proptest::option::of(0usize..6),
    ) {
        let reject = reject_index.filter(|index| *index < chain_len);
        let mut registry = ExtensionRegistry::new();
        for index in 0..chain_len {
            registry.register(Arc::new(MarkerTransform {
                name: format!("transform-{index}"),
                marker: format!("marker-{index}"),
                reject: reject == Some(index),
            }));
        }
        let input = vec![ChatMessage::text(MessageRole::User, "hello")];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = runtime.block_on(registry.transform(input.clone()));

        match reject {
            Some(_) => prop_assert!(result.is_err(), "a rejection ends the chain"),
            None => {
                let output = result.expect("no rejection");
                prop_assert_eq!(&output[..input.len()], &input[..], "the input is preserved as a prefix");
                let markers: Vec<String> = output[input.len()..]
                    .iter()
                    .filter_map(text_of)
                    .collect();
                let expected: Vec<String> =
                    (0..chain_len).map(|index| format!("marker-{index}")).collect();
                prop_assert_eq!(markers, expected, "transforms append in registration order");
            }
        }
    }
}
