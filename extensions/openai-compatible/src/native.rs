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

    fn manifest_text(&self) -> Option<String> {
        Some(super::MANIFEST.to_string())
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

    fn turn_started(&self) {
        // The other half of `interrupt`: without this the flag latches and
        // every later turn's first call is pre-cancelled - one Ctrl+C and
        // the provider answers "request cancelled by the user" forever
        // (FR-CONC-1's turn boundary; the WASM host already does both).
        self.cap.reset_cancellation();
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
        // ADR-0035: the passed settings are the source of truth - the same
        // pairs `complete` gets in its `extras`. The credential read is the
        // fallback for a caller that has not been told the settings yet.
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
        // gh #34's window resolution and gh #31's per-model profile both
        // come from `profiles`, so this list and the WASM form's are built
        // by the same code (NFR-25).
        // gh #31 review: an env-configured session has no stored list, so
        // the picker's `GET /models` discovery happens here - the one live
        // request `/model` makes. It fails closed (the net path refuses an
        // ungranted host before it resolves), and the host runs its
        // endpoint consent before `/model` asks for a list, so the picker
        // prompts instead of reporting nothing. The answer is stored under
        // this namespace, which is where the list already lives, so the
        // next call reads instead of asking again.
        // Two guards keep this from firing where it must not: a model is
        // already configured (so the list is not what the caller is
        // missing), and the endpoint is not the built-in default (a fresh
        // install must never open a socket to api.openai.com on its own).
        // The picker's consent runs before the host asks, so a custom
        // endpoint that is not granted fails closed here and then prompts.
        let base = profiles::base_url_for(self.cap.as_ref(), &self.settings, &None);
        let custom_endpoint = base.trim_end_matches('/') != "https://api.openai.com/v1";
        let stored = if stored.is_empty() && configured.is_empty() && custom_endpoint {
            // On its own thread, which is this crate's blocking region:
            // a capability call drives its future through `drive`, and
            // the ambient-runtime path panics when the caller is already
            // inside one - which every call on the interface's thread is
            // (`main` runs the whole interface under `block_on`), and
            // which is why extension calls arrive through the blocking
            // pool. Discovery is the one host-side call, so it makes its
            // own region: a fresh thread has no runtime, so `drive` takes
            // the shared one (Windows CI caught this at startup: the
            // no-model state never rendered).
            let cap = self.cap.clone();
            let settings = self.settings.clone();
            let discovered = std::thread::scope(|scope| {
                scope
                    .spawn(move || {
                        discover_models(
                            cap.as_ref(),
                            &base,
                            profiles::api_key_for(cap.as_ref(), &settings, &None).as_deref(),
                        )
                    })
                    .join()
                    .ok()
                    .flatten()
            });
            match discovered {
                Some(found) if !found.is_empty() => {
                    let line = found
                        .iter()
                        .map(|(model, window)| match window {
                            Some(window) => format!("{model}={window}"),
                            None => model.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    let _ = self.cap.credentials_set("models", &line);
                    line
                }
                _ => stored,
            }
        } else {
            stored
        };
        let windows = load_context_windows(self.cap.as_ref());
        let image_limits = load_image_limits(self.cap.as_ref());
        // gh #64: the user's file beats curated and discovered. Parsed
        // once per listing; an absent file parses to nothing.
        let overrides = crate::parse_model_overrides(&self.settings.model_overrides);
        Ok(
            profiles::picker_models(self.cap.as_ref(), &self.settings, &stored, &configured)
                .into_iter()
                .map(|picked| {
                    let user = crate::override_for(&overrides, "openai-compatible", &picked.id)
                        .and_then(|item| item.context_window);
                    let context_window = context_window_for(
                        &picked.id,
                        self.settings.context_window,
                        user.or(picked.window),
                        &windows,
                    );
                    let mut extras: std::collections::BTreeMap<String, String> =
                        profiles::row_extras(&picked).into_iter().collect();
                    // #39: image behavior rides the non-structural extras,
                    // so unknown models simply carry nothing.
                    for (key, value) in image_extras(&picked.id, &image_limits) {
                        extras.insert(key, value);
                    }
                    if let Some(item) =
                        crate::override_for(&overrides, "openai-compatible", &picked.id)
                    {
                        for (key, value) in crate::override_extras(item) {
                            extras.insert(key, value);
                        }
                    }
                    ModelInfo {
                        id: picked.id.clone(),
                        name: picked.id,
                        context_window,
                        max_tokens: 0,
                        extras,
                    }
                })
                .collect(),
        )
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
