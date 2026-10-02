use super::*;
use std::sync::Arc;

use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::{DispatchError, ModelInfo};

/// The native handle: shared logic over the capability engine.
pub struct OpenAiCompat {
    cap: Arc<lca_tools::Capabilities>,
    settings: Settings,
}

impl OpenAiCompat {
    /// Build from the engine the manifest's grants live in and the
    /// environment-derived settings.
    pub fn new(cap: Arc<lca_tools::Capabilities>) -> OpenAiCompat {
        Self::with_settings(cap, Settings::default())
    }

    /// Build with explicit settings (tests and anything that
    /// resolves configuration outside the environment).
    pub fn with_settings(cap: Arc<lca_tools::Capabilities>, settings: Settings) -> OpenAiCompat {
        OpenAiCompat { cap, settings }
    }

    /// The engine, for tests that inspect recorded denials.
    pub fn capabilities(&self) -> &Arc<lca_tools::Capabilities> {
        &self.cap
    }
}

impl ExtensionDispatch for OpenAiCompat {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<World> {
        vec![World::Provider]
    }

    fn interrupt(&self) {
        // A native call shares the caller's thread, so there is no epoch
        // to bump: flag the capability engine directly, and a blocked
        // `net` request polls its way out (FR-CONC-1, NFR-21).
        self.cap.cancel();
    }

    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: "openai-compatible".to_string(),
            world: "tool",
        })
    }

    fn execute_tool<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: "openai-compatible".to_string(),
            world: "tool",
        })))
    }

    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        // This provider ships no slash commands of its own; the
        // identity trio (`/login`, `/logout`, `/usage`) is namespaced
        // by the host through the provider world (FR-PROV-10). The
        // manifest lists only the `provider` world for the same reason:
        // the schema requires every declared world to be exported.
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
        settings: &[(String, String)],
    ) -> Result<Vec<ModelInfo>, DispatchError> {
        // D2: the list `login-submit` discovered (or the preset's
        // curated short list) is what `/model` offers.
        //
        // The configured model leads, and "configured" now includes what a
        // login just persisted: the environment first (documented
        // precedence over stored settings), then the `model` pair the host
        // passes (ADR-0035: list-models takes settings), then the
        // extension's own credential namespace. A login's model therefore
        // appears the moment it lands, with no restart, and an
        // unconfigured provider lists no empty row (G3, issue #2).
        let configured = if self.settings.model.is_empty() {
            settings
                .iter()
                .find(|(key, _)| key == "model")
                .map(|(_, value)| value.clone())
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    self.cap
                        .credentials_get("model")
                        .ok()
                        .flatten()
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or_default()
        } else {
            self.settings.model.clone()
        };
        let mut models: Vec<ModelInfo> = Vec::new();
        if !configured.is_empty() {
            models.push(ModelInfo {
                id: configured.clone(),
                name: configured,
                context_window: self.settings.context_window,
                max_tokens: 0,
            });
        }
        // ADR-0035: the passed settings are the source of truth -
        // the same pairs `complete` gets in its `extras`. The
        // credential read is the fallback for a caller that has not
        // been told the settings yet.
        let stored = settings
            .iter()
            .find(|(key, _)| key == "models")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| {
                self.cap
                    .credentials_get("models")
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            });
        for id in stored.split(',').filter(|id| !id.is_empty()) {
            if models.iter().any(|model| model.id == id) {
                continue;
            }
            models.push(ModelInfo {
                id: id.to_string(),
                name: id.to_string(),
                context_window: self.settings.context_window,
                max_tokens: 0,
            });
        }
        Ok(models)
    }

    fn stream_completion<'a>(
        &'a self,
        request: CompletionRequest,
        sink: &'a dyn lca_protocol::EventSink,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        let cap = self.cap.clone();
        let settings = self.settings.clone();
        Box::pin(async move {
            let result = lca_tools::bridge_stream(
                move |bridge| {
                    run_provider_stream(cap.as_ref(), &settings, &request, &mut |event| {
                        bridge.push(event)
                    })
                },
                sink,
            )
            .await;
            match result {
                Ok(()) => Ok(()),
                Err(lca_tools::BridgeError::Work(failure)) => Err(DispatchError::Failed(format!(
                    "openai-compatible: {}",
                    failure.message
                ))),
                Err(lca_tools::BridgeError::Panicked) => Err(DispatchError::Failed(
                    "openai-compatible: the provider call panicked".to_string(),
                )),
            }
        })
    }

    fn identity_login(&self) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        let cap = self.cap.clone();
        let settings = self.settings.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run_login(cap.as_ref(), &settings))
                .await
                .map_err(|_| DispatchError::Failed("openai-compatible: login panicked".into()))
        })
    }

    fn identity_logout(&self) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        let cap = self.cap.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run_logout(cap.as_ref()))
                .await
                .map_err(|_| DispatchError::Failed("openai-compatible: logout panicked".into()))
        })
    }

    fn identity_usage(
        &self,
    ) -> DispatchFuture<'static, Result<Result<Usage, IdentityOutcome>, DispatchError>> {
        // No usage endpoint can be assumed across arbitrary
        // OpenAI-compatible servers (ADR-0012's optional export).
        Box::pin(std::future::ready(Ok(Err(IdentityOutcome::NotSupported))))
    }

    fn login_options(
        &self,
    ) -> DispatchFuture<'static, Result<Vec<lca_protocol::LoginOption>, DispatchError>> {
        let cap = self.cap.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || login_options(cap.as_ref()))
                .await
                .map_err(|_| {
                    DispatchError::Failed("openai-compatible: login options panicked".into())
                })
        })
    }

    fn login_submit(
        &self,
        answer: lca_protocol::LoginAnswer,
    ) -> DispatchFuture<'static, Result<Vec<(String, String)>, DispatchError>> {
        let cap = self.cap.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || login_submit(cap.as_ref(), &answer))
                .await
                .map_err(|_| {
                    DispatchError::Failed("openai-compatible: login submit panicked".into())
                })?
                .map_err(DispatchError::Failed)
        })
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
