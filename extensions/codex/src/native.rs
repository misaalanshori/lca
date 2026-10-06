//! Native delivery mode: the dispatch handle the registry holds.

use super::*;
use std::sync::Arc;

use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::{DispatchError, IdentityOutcome, ModelInfo};

/// The native handle over the shared capability engine.
pub struct Codex {
    cap: Arc<lca_tools::Capabilities>,
}

impl Codex {
    /// Build from the engine the manifest's grants live in.
    pub fn new(cap: Arc<lca_tools::Capabilities>) -> Codex {
        Codex { cap }
    }
}

impl ExtensionDispatch for Codex {
    fn name(&self) -> &str {
        "codex"
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<World> {
        vec![World::Provider]
    }

    fn interrupt(&self) {
        self.cap.cancel();
    }

    fn turn_started(&self) {
        self.cap.reset_cancellation();
    }

    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: "codex".to_string(),
            world: "tool",
        })
    }

    fn execute_tool<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: "codex".to_string(),
            world: "tool",
        })))
    }

    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn invoke_command(
        &self,
        _name: &str,
        _argument: &str,
    ) -> Result<lca_protocol::CommandEffect, DispatchError> {
        Ok(lca_protocol::CommandEffect::None)
    }

    fn provider_models(
        &self,
        _settings: &[(String, String)],
    ) -> Result<Vec<ModelInfo>, DispatchError> {
        let cap = self.cap.clone();
        Ok(list_models(cap.as_ref()))
    }

    fn stream_completion<'a>(
        &'a self,
        request: lca_protocol::CompletionRequest,
        sink: &'a dyn lca_protocol::EventSink,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        let cap = self.cap.clone();
        Box::pin(async move {
            let result = lca_tools::bridge_stream(
                move |bridge| {
                    run_provider_stream(cap.as_ref(), &request, &mut |event| bridge.push(event))
                },
                sink,
            )
            .await;
            match result {
                Ok(()) => Ok(()),
                Err(lca_tools::BridgeError::Work(failure)) => {
                    Err(DispatchError::Failed(format!("codex: {}", failure.message)))
                }
                Err(lca_tools::BridgeError::Panicked) => Err(DispatchError::Failed(
                    "codex: the provider call panicked".to_string(),
                )),
            }
        })
    }

    fn identity_login(&self) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        let cap = self.cap.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking({
                let cap = cap.clone();
                move || {
                    run_login(cap.as_ref(), cap.as_ref())
                        .map_err(|err| DispatchError::Failed(format!("codex: {}", err.0)))
                }
            })
            .await
            .map_err(|_| DispatchError::Failed("codex: login panicked".into()))
            .and_then(std::convert::identity)
        })
    }

    fn identity_logout(&self) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        let cap = self.cap.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run_logout(cap.as_ref()))
                .await
                .map_err(|_| DispatchError::Failed("codex: logout panicked".into()))
        })
    }

    fn identity_usage(
        &self,
    ) -> DispatchFuture<'static, Result<Result<lca_protocol::Usage, IdentityOutcome>, DispatchError>>
    {
        let cap = self.cap.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                run_usage(cap.as_ref()).map_err(|err| IdentityOutcome::Failed(err.0))
            })
            .await
            .map_err(|_| DispatchError::Failed("codex: usage panicked".into()))
        })
    }

    fn login_options(
        &self,
    ) -> DispatchFuture<'static, Result<Vec<lca_protocol::LoginOption>, DispatchError>> {
        Box::pin(std::future::ready(Ok(login_options())))
    }

    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
    }

    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a lca_protocol::PostToolObservation,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}
