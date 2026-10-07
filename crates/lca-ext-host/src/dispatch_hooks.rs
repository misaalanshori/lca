//! WASM hook dispatch (gh #45 + the six legacy points): every
//! `ExtensionDispatch` hook method over its component call. Split from
//! `lib.rs` for the workspace file ceiling (gate 11).

use super::*;
use crate::tool_hooks::{
    before_compact_work, before_settle_work, cache_decision_work, compact_failed_work,
    message_end_work, observe_work, pre_tool_work, session_close_work, stream_event_work,
    tool_call_work, tool_result_work, trust_work, turn_end_work,
};

impl lca_ext_abi::ExtensionDispatch for WasmExtension {
    fn name(&self) -> &str {
        &self.inner.name
    }

    fn manifest_text(&self) -> Option<String> {
        Some(self.inner.manifest_text.clone())
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Wasm
    }

    fn worlds(&self) -> Vec<World> {
        self.inner
            .worlds
            .iter()
            .filter_map(|world| match world.as_str() {
                "tool" => Some(World::Tool),
                "tool-catalog" => Some(World::ToolCatalog),
                "hooks-message" => Some(World::HooksMessage),
                "hooks-tool-call" => Some(World::HooksToolCall),
                "hooks-tool-result" => Some(World::HooksToolResult),
                "hooks-stream" => Some(World::HooksStream),
                "hooks-settle" => Some(World::HooksSettle),
                "hooks-compaction" => Some(World::HooksCompaction),
                "hooks-cache" => Some(World::HooksCache),
                "hooks-trust" => Some(World::HooksTrust),
                "command" => Some(World::Command),
                "hooks" => Some(World::Hooks),
                "provider" => Some(World::Provider),
                "compaction" => Some(World::Compaction),
                "context-transform" => Some(World::ContextTransform),
                "ui" => Some(World::Ui),
                _ => None,
            })
            .collect()
    }

    /// Install the tool registry surface (gh #77): the running turn
    /// calls this so the `tools` import serves through it.
    fn set_tools_view(&self, view: Arc<dyn lca_ext_abi::ToolsRegistryView>) {
        *self
            .inner
            .tools_view
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(view);
    }

    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        // A suite guest serves every tool through the catalog (gh
        // #77); a single-tool guest wraps its one schema as a direct
        // tool with no namespace, exactly the pre-catalog shape.
        if self.worlds().contains(&World::ToolCatalog) {
            return self
                .blocking(catalog_specs_work)
                .map_err(|err| to_dispatch(err, self.name()));
        }
        if !self.worlds().contains(&World::Tool) {
            return Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "tool",
            });
        }
        self.schema()
            .map(|spec| vec![spec])
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn execute_tool<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(async move {
            // Suite tools run through the catalog by name (gh #77);
            // the single-tool world keeps its nameless `run`.
            if self.worlds().contains(&World::ToolCatalog) {
                let (name, call) = (call.name.clone(), call.clone());
                return self
                    .on_blocking_pool(move |inner| execute_catalog_work(inner, &name, call))
                    .await
                    .map_err(|err| to_dispatch(err, self.name()));
            }
            if !self.worlds().contains(&World::Tool) {
                return Err(DispatchError::MissingWorld {
                    extension: self.name().to_string(),
                    world: "tool",
                });
            }
            let call = call.clone();
            self.on_blocking_pool(move |inner| execute_work(inner, call))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn command_specs(&self) -> Result<Vec<DispatchCommandSpec>, DispatchError> {
        if !self.worlds().contains(&World::Command) {
            return Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "command",
            });
        }
        self.blocking(command_specs_work)
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn invoke_command(&self, _name: &str, argument: &str) -> Result<CommandEffect, DispatchError> {
        if !self.worlds().contains(&World::Command) {
            return Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "command",
            });
        }
        let leaf = _name.to_string();
        let argument = argument.to_string();
        self.blocking(move |inner| invoke_work(inner, &leaf, &argument))
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn provider_models(
        &self,
        settings: &[(String, String)],
    ) -> Result<Vec<ModelInfo>, DispatchError> {
        if !self.worlds().contains(&World::Provider) {
            return Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "provider",
            });
        }
        let settings = settings.to_vec();
        self.blocking(move |inner| provider_models_work(inner, &settings))
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn stream_completion<'a>(
        &'a self,
        request: CompletionRequest,
        sink: &'a dyn EventSink,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Provider) {
                return Err(DispatchError::MissingWorld {
                    extension: self.name().to_string(),
                    world: "provider",
                });
            }
            // The component call runs on the blocking pool (ADR-0014);
            // events cross back through an unbounded bridge the async
            // side drains into `sink` with real backpressure. A dropped
            // `sink` (or a dropped future) ends the stream: the bridge
            // send fails and the work loop stops (FR-CONC-3).
            let (stx, mut srx) = tokio::sync::mpsc::unbounded_channel();
            struct Bridge(tokio::sync::mpsc::UnboundedSender<lca_protocol::StreamEvent>);
            impl EventSink for Bridge {
                fn push(&self, event: lca_protocol::StreamEvent) -> bool {
                    self.0.send(event).is_ok()
                }
            }
            let engine = self.inner.engine.clone();
            let work_inner = self.inner.clone();
            let fail_inner = self.inner.clone();
            let extension = self.inner.name.clone();
            let mut join = tokio::task::spawn_blocking(move || {
                provider_stream_work(&work_inner, request, Arc::new(Bridge(stx)))
            });
            let mut joined: Option<Result<Result<(), CallError>, tokio::task::JoinError>> = None;
            loop {
                if joined.is_some() {
                    while let Some(event) = srx.recv().await {
                        let _ = sink.push(event);
                    }
                    break;
                }
                tokio::select! {
                    maybe = srx.recv() => match maybe {
                        None => break,
                        Some(event) => if !sink.push(event) {
                            // Receiver gone: trap the guest so the blocking
                            // work ends promptly, then drain the bridge.
                            engine.increment_epoch();
                            while srx.recv().await.is_some() {}
                            break;
                        },
                    },
                    result = &mut join => joined = Some(result),
                }
            }
            let result = match joined {
                Some(result) => result,
                None => join.await,
            };
            match result {
                Ok(inner_result) => inner_result.map_err(|err| to_dispatch(err, &extension)),
                Err(_) => {
                    fail_inner.disable();
                    Err(DispatchError::Failed(format!(
                        "{extension}: host call panicked"
                    )))
                }
            }
        })
    }

    fn identity_login(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"provider".to_string()) {
                return Err(DispatchError::MissingWorld {
                    extension,
                    world: "provider",
                });
            }
            pool_call(inner, |inner| {
                identity_simple_work(inner, IdentityOp::Login)
            })
            .await
            .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn identity_logout(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"provider".to_string()) {
                return Err(DispatchError::MissingWorld {
                    extension,
                    world: "provider",
                });
            }
            pool_call(inner, |inner| {
                identity_simple_work(inner, IdentityOp::Logout)
            })
            .await
            .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn identity_usage(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<Result<Usage, IdentityOutcome>, DispatchError>>
    {
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"provider".to_string()) {
                return Err(DispatchError::MissingWorld {
                    extension,
                    world: "provider",
                });
            }
            pool_call(inner, identity_usage_work)
                .await
                .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn login_options(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<Vec<lca_protocol::LoginOption>, DispatchError>>
    {
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            // A provider without the login surface has no options; the host
            // still shows its own "Custom endpoint…" entry.
            if !inner.worlds.contains(&"provider".to_string()) {
                return Ok(Vec::new());
            }
            pool_call(inner, login_options_work)
                .await
                .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn login_submit(
        &self,
        answer: lca_protocol::LoginAnswer,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<Vec<(String, String)>, DispatchError>> {
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"provider".to_string()) {
                return Ok(Vec::new());
            }
            pool_call(inner, move |inner| login_submit_work(inner, answer))
                .await
                .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn compact(
        &self,
        records: &[lca_protocol::Record],
    ) -> lca_ext_abi::DispatchFuture<'static, Result<String, DispatchError>> {
        if !self.worlds().contains(&World::Compaction) {
            return Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "compaction",
            })));
        }
        let records = records.to_vec();
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            pool_call(inner, move |inner| compact_work(inner, records))
                .await
                .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn transform_messages(
        &self,
        messages: Vec<lca_protocol::ChatMessage>,
    ) -> lca_ext_abi::DispatchFuture<
        'static,
        Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
    > {
        if !self.worlds().contains(&World::ContextTransform) {
            return Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "context-transform",
            })));
        }
        let inner = self.inner.clone();
        let extension = inner.name.clone();
        Box::pin(async move {
            pool_call(inner, move |inner| transform_work(inner, messages))
                .await
                .map_err(|err| to_dispatch(err, &extension))
        })
    }

    fn ui_regions(&self) -> Vec<String> {
        self.inner.ui_regions.clone()
    }

    fn render(&self, region: &str) -> Result<Option<lca_protocol::WidgetTree>, DispatchError> {
        if !self.worlds().contains(&World::Ui) || !self.inner.ui_regions.iter().any(|r| r == region)
        {
            if self.worlds().contains(&World::Ui) {
                // Declared the world but not this region: the denial is
                // recorded, the export is never called (catalog `ui`).
                self.inner.cap.note_ui_denial(region);
            }
            return Ok(None);
        }
        let region = region.to_string();
        // Frame-time call: the same blocking thread a registration call
        // uses (Wasmtime's sync WASI needs no runtime poll). A small
        // wasm component answers within the frame budget; ponytail:
        // measure with NFR-4's numbers if a heavy extension ever
        // misses it.
        self.blocking(move |inner| render_work(inner, &region))
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn on_ui_event(
        &self,
        region: &str,
        input: &lca_protocol::UiInput,
    ) -> Result<lca_protocol::UiEffect, DispatchError> {
        if !self.worlds().contains(&World::Ui) || !self.inner.ui_regions.iter().any(|r| r == region)
        {
            return Ok(lca_protocol::UiEffect::None);
        }
        let region = region.to_string();
        let input = input.clone();
        self.blocking(move |inner| event_work(inner, &region, &input))
            .map_err(|err| to_dispatch(err, self.name()))
    }

    fn interrupt(&self) {
        // FR-CONC-1: epoch interruption, independent of the fuel budget.
        // Flag first: a store built concurrently must see it, because its
        // own deadline is measured from the epoch *after* this bump.
        self.inner.interrupted.store(true, Ordering::SeqCst);
        self.inner.engine.increment_epoch();
        // An epoch bump only fires at a guest code point, so a host import
        // blocked in a long wait would never see it: flag the capability
        // engine too, and the wait polls its way out (FR-CONC-1, NFR-21).
        self.inner.cap.cancel();
    }

    fn turn_started(&self) {
        // The turn boundary clears both halves of the previous turn's
        // cancellation, so this turn's first call starts clean
        // (FR-CONC-1).
        self.inner.interrupted.store(false, Ordering::SeqCst);
        self.inner.cap.reset_cancellation();
    }

    fn oauth_manual_callback(&self, params: Vec<(String, String)>) -> Result<(), DispatchError> {
        self.inner
            .cap
            .oauth_deliver_manual(params)
            .map_err(|err| DispatchError::Failed(err.to_string()))
    }

    fn oauth_last_url(&self) -> Option<String> {
        // Read, never pop: the recorded URL is also what the provider-flow
        // tests assert on, and the host asks once per poll.
        self.inner.cap.oauth_opened().last().cloned()
    }

    fn on_pre_turn(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        let inner = self.inner.clone();
        let name = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"hooks".to_string()) {
                return Ok(());
            }
            pool_call(inner, |inner| observe_work(inner, None, None, None))
                .await
                .map_err(|err| to_dispatch(err, &name))
        })
    }

    fn on_pre_tool_use<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Hooks) {
                return Ok(HookAction::Allow);
            }
            let call = call.clone();
            self.on_blocking_pool(move |inner| pre_tool_work(inner, call))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_post_tool_use<'a>(
        &'a self,
        observation: &'a PostToolObservation,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Hooks) {
                return Ok(());
            }
            let observation = observation.clone();
            self.on_blocking_pool(move |inner| observe_work(inner, Some(&observation), None, None))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_post_turn_end<'a>(
        &'a self,
        status: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Hooks) {
                return Ok(());
            }
            let status = status.to_string();
            self.on_blocking_pool(move |inner| observe_work(inner, None, Some(&status), None))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_attention_required<'a>(
        &'a self,
        reason: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::Hooks) {
                return Ok(());
            }
            let reason = reason.to_string();
            self.on_blocking_pool(move |inner| observe_work(inner, None, None, Some(&reason)))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_session_close(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        let inner = self.inner.clone();
        let name = inner.name.clone();
        Box::pin(async move {
            if !inner.worlds.contains(&"hooks".to_string()) {
                return Ok(());
            }
            pool_call(inner, session_close_work)
                .await
                .map_err(|err| to_dispatch(err, &name))
        })
    }

    // Gh #45's opt-in worlds: an undeclared world answers with the
    // default, so old guests behave exactly like defaulted natives.
    fn on_message_end<'a>(
        &'a self,
        role: &'a str,
        text: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<Option<String>, DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::HooksMessage) {
                return Ok(None);
            }
            let (role, text) = (role.to_string(), text.to_string());
            self.on_blocking_pool(move |inner| message_end_work(inner, &role, &text))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_tool_call<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::ToolCallPatch, DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::HooksToolCall) {
                return Ok(lca_protocol::ToolCallPatch::default());
            }
            let call = call.clone();
            self.on_blocking_pool(move |inner| tool_call_work(inner, call))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_tool_result<'a>(
        &'a self,
        call: &'a ToolCall,
        result: &'a lca_protocol::ToolResult,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::ToolResultPatch, DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::HooksToolResult) {
                return Ok(lca_protocol::ToolResultPatch::default());
            }
            let (call, result) = (call.clone(), result.clone());
            self.on_blocking_pool(move |inner| tool_result_work(inner, &call, &result))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_stream_event<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        kind: &'a str,
        data: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::HooksStream) {
                return Ok(());
            }
            let (provider, model, kind, data) = (
                provider.to_string(),
                model.to_string(),
                kind.to_string(),
                data.to_string(),
            );
            self.on_blocking_pool(move |inner| {
                stream_event_work(inner, &provider, &model, &kind, &data)
            })
            .await
            .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_turn_end<'a>(
        &'a self,
        rounds: u32,
        tool_calls: u32,
        status: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::SettleDecision, DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::HooksSettle) {
                return Ok(lca_protocol::SettleDecision::default());
            }
            let status = status.to_string();
            self.on_blocking_pool(move |inner| turn_end_work(inner, rounds, tool_calls, &status))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_agent_before_settle<'a>(
        &'a self,
        rounds: u32,
        tool_calls: u32,
        status: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::SettleDecision, DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::HooksSettle) {
                return Ok(lca_protocol::SettleDecision::default());
            }
            let status = status.to_string();
            self.on_blocking_pool(move |inner| {
                before_settle_work(inner, rounds, tool_calls, &status)
            })
            .await
            .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_session_before_compact<'a>(
        &'a self,
        reason: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::CompactVerdict, DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::HooksCompaction) {
                return Ok(lca_protocol::CompactVerdict::Allow);
            }
            let reason = reason.to_string();
            self.on_blocking_pool(move |inner| before_compact_work(inner, &reason))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_session_compact_failed<'a>(
        &'a self,
        reason: &'a str,
        error: Option<&'a str>,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::HooksCompaction) {
                return Ok(());
            }
            let (reason, error) = (reason.to_string(), error.map(str::to_string));
            self.on_blocking_pool(move |inner| {
                compact_failed_work(inner, &reason, error.as_deref())
            })
            .await
            .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_cache_warming_decision<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<bool, DispatchError>> {
        Box::pin(async move {
            if !self.worlds().contains(&World::HooksCache) {
                return Ok(true);
            }
            let (provider, model) = (provider.to_string(), model.to_string());
            self.on_blocking_pool(move |inner| cache_decision_work(inner, &provider, &model))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }

    fn on_project_trust<'a>(
        &'a self,
        cwd: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(lca_protocol::TrustVote, bool), DispatchError>>
    {
        Box::pin(async move {
            if !self.worlds().contains(&World::HooksTrust) {
                return Ok((lca_protocol::TrustVote::Undecided, false));
            }
            let cwd = cwd.to_string();
            self.on_blocking_pool(move |inner| trust_work(inner, &cwd))
                .await
                .map_err(|err| to_dispatch(err, self.name()))
        })
    }
}
