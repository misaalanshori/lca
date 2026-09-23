//! Reference hooks implementation (the `permission-gate` shape from Pi's
//! examples: deny by pattern before the permission prompt runs) plus the
//! command that fills the `/stats` built-in slot — the Phase 2 proof
//! that a built-in behavior can live on the extension path while the
//! user-facing name stays put (ADR-0019).

#![forbid(unsafe_code)]

use std::sync::Arc;

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_protocol::{
    CommandEffect, CommandSpec, DispatchError, HookAction, PostToolObservation, ToolCall,
    ToolResult, ToolSpec,
};

/// Tool-name prefixes this policy refuses (the model sees the reason, so
/// the denial teaches rather than just blocks).
const DENY_PREFIX: &str = "deny-";

/// The reference policy extension.
pub struct HooksExample {
    stats: Arc<dyn Fn() -> String + Send + Sync>,
}

impl HooksExample {
    /// Build it with the source of the `/stats` text.
    pub fn new(stats: Arc<dyn Fn() -> String + Send + Sync>) -> HooksExample {
        HooksExample { stats }
    }
}

impl ExtensionDispatch for HooksExample {
    fn name(&self) -> &str {
        "hooks-example"
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<World> {
        vec![World::Hooks, World::Command]
    }

    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(self.no_world("tool")))
    }

    fn on_pre_turn(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
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

    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name().to_string(),
            world: "tool",
        })
    }

    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Ok(vec![CommandSpec {
            name: "stats".to_string(),
            hint: "session statistics".to_string(),
            completion: "none".to_string(),
            extras: Default::default(),
        }])
    }

    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Ok(CommandEffect::ShowWidget((self.stats)()))
    }

    fn builtin_command_slots(&self) -> Vec<String> {
        vec!["stats".to_string()]
    }

    /// The `permission-gate` pattern: deny before the prompt exists.
    fn on_pre_tool_use<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<HookAction, DispatchError>> {
        let action = if call.name.starts_with(DENY_PREFIX) {
            HookAction::Deny(format!(
                "hooks-example policy: `{}` is denied by pattern",
                call.name
            ))
        } else {
            HookAction::Allow
        };
        Box::pin(std::future::ready(Ok(action)))
    }
}

impl HooksExample {
    fn no_world(&self, world: &'static str) -> Result<ToolResult, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name().to_string(),
            world,
        })
    }
}
