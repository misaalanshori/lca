use super::*;
use lca_tools::Capabilities;
use std::sync::Arc;

/// The native twin of the guest's `Cap`: it calls the same
/// [`Capabilities`] engine the WASM host's imports call.
pub struct NativeCap(pub Arc<Capabilities>);

impl Cap for NativeCap {
    fn fs_read(&self, scope: &str, path: &str) -> Result<Vec<u8>, CapabilityError> {
        self.0.fs_read(scope, path)
    }
    fn fs_write(&self, scope: &str, path: &str, bytes: &[u8]) -> Result<(), CapabilityError> {
        self.0.fs_write(scope, path, bytes)
    }
    fn fs_stat(&self, scope: &str, path: &str) -> Result<(bool, u64), CapabilityError> {
        self.0.fs_stat(scope, path)
    }
    fn fs_list(&self, scope: &str, path: &str) -> Result<Vec<String>, CapabilityError> {
        self.0.fs_list(scope, path)
    }
    fn resource_list(&self, prefix: &str) -> Result<Vec<(String, u64)>, CapabilityError> {
        self.0.resource_list(prefix)
    }
    fn resource_read(&self, path: &str) -> Result<Vec<u8>, CapabilityError> {
        self.0.resource_read(path)
    }
    fn state_read(&self, key: &str) -> Result<Option<Vec<u8>>, CapabilityError> {
        self.0.state_read(key)
    }
    fn state_write(&self, key: &str, value: &[u8]) -> Result<(), CapabilityError> {
        self.0.state_write(key, value)
    }
    fn state_delete(&self, key: &str) -> Result<(), CapabilityError> {
        self.0.state_delete(key)
    }
    fn state_list(&self) -> Result<Vec<(String, u64)>, CapabilityError> {
        self.0.state_list()
    }
    fn process_spawn(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
    ) -> Result<u32, CapabilityError> {
        self.0.process_spawn(program, args, cwd)
    }
    fn process_read_stdout(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        self.0.process_read_stdout(handle, max)
    }
    fn process_read_stderr(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        self.0.process_read_stderr(handle, max)
    }
    fn process_write_stdin(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
        self.0.process_write_stdin(handle, bytes)
    }
    fn process_wait(&self, handle: u32) -> Result<i32, CapabilityError> {
        self.0.process_wait(handle)
    }
    fn process_kill(&self, handle: u32) -> Result<(), CapabilityError> {
        self.0.process_kill(handle)
    }
    fn pty_spawn(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
        rows: u16,
        cols: u16,
    ) -> Result<u32, CapabilityError> {
        self.0.pty_spawn(program, args, cwd, rows, cols)
    }
    fn pty_read(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        self.0.pty_read(handle, max)
    }
    fn pty_write(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
        self.0.pty_write(handle, bytes)
    }
    fn pty_resize(&self, handle: u32, rows: u16, cols: u16) -> Result<(), CapabilityError> {
        self.0.pty_resize(handle, rows, cols)
    }
    fn pty_wait(&self, handle: u32) -> Result<i32, CapabilityError> {
        self.0.pty_wait(handle)
    }
    fn pty_kill(&self, handle: u32) -> Result<(), CapabilityError> {
        self.0.pty_kill(handle)
    }
}

impl crate::IdentityCap for NativeCap {
    fn credentials_set(&self, key: &str, value: &str) -> Result<(), CapabilityError> {
        self.0.credentials_set(key, value)
    }
    fn credentials_get(&self, key: &str) -> Result<Option<String>, CapabilityError> {
        self.0.credentials_get(key)
    }
    fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError> {
        self.0.credentials_delete(key)
    }
    fn oauth_begin(&self, redirect_path: &str) -> Result<(String, u32), CapabilityError> {
        self.0.oauth_begin(redirect_path)
    }
    fn oauth_open(&self, url: &str) -> Result<(), CapabilityError> {
        self.0.oauth_open(url)
    }
    fn oauth_await(&self, handle: u32) -> Result<Vec<(String, String)>, CapabilityError> {
        self.0.oauth_await(handle)
    }
    fn oauth_end(&self, handle: u32) -> Result<(), CapabilityError> {
        self.0.oauth_end(handle)
    }
}

/// The native-linked conformance extension: same schema, same shared
/// dispatch, same capability engine (FR-EXT-6).
pub struct NativeConformance {
    cap: Arc<Capabilities>,
}

impl NativeConformance {
    /// Wrap the extension's capability engine.
    pub fn new(cap: Arc<Capabilities>) -> NativeConformance {
        NativeConformance { cap }
    }

    /// The tool schema (identical to the guest's).
    #[allow(clippy::expect_used)] // the fixture's own scripted schema string is JSON by construction.
    pub fn schema(&self) -> lca_protocol::ToolSpec {
        let (name, description, parameters) = schema_json();
        lca_protocol::ToolSpec {
            name,
            description,
            parameters: serde_json::from_str(&parameters).expect("schema is json"),
            extras: Default::default(),
        }
    }

    /// Execute one call through the shared dispatch. Guest-only modes
    /// report unknown here; the conformance diff never uses them.
    pub fn execute(&self, call: &ToolCall) -> ToolResult {
        let (mode, args) = mode_and_args(&call.arguments);
        let outcome = run_shared(&NativeCap(self.cap.clone()), &mode, &args);
        outcome_to_result(&call.call_id, outcome)
    }
}

impl lca_ext_abi::ExtensionDispatch for NativeConformance {
    fn name(&self) -> &str {
        "conformance"
    }

    fn delivery(&self) -> lca_ext_abi::DeliveryMode {
        lca_ext_abi::DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<lca_ext_abi::World> {
        vec![
            lca_ext_abi::World::Tool,
            lca_ext_abi::World::Command,
            lca_ext_abi::World::Hooks,
            lca_ext_abi::World::Provider,
            lca_ext_abi::World::Compaction,
            lca_ext_abi::World::ContextTransform,
            lca_ext_abi::World::Ui,
        ]
    }

    fn ui_regions(&self) -> Vec<String> {
        vec![
            "status-line".to_string(),
            "footer".to_string(),
            "panel".to_string(),
            "modal".to_string(),
        ]
    }

    fn render(
        &self,
        region: &str,
    ) -> Result<Option<lca_protocol::WidgetTree>, lca_protocol::DispatchError> {
        Ok(crate::ui_script(region).map(|nodes| lca_protocol::WidgetTree { nodes }))
    }

    fn on_ui_event(
        &self,
        region: &str,
        input: &lca_protocol::UiInput,
    ) -> Result<lca_protocol::UiEffect, lca_protocol::DispatchError> {
        Ok(crate::ui_event_script(region, input))
    }

    fn compact(
        &self,
        records: &[lca_protocol::Record],
    ) -> lca_ext_abi::DispatchFuture<'static, Result<String, lca_protocol::DispatchError>> {
        let excerpts = compact_excerpts(records);
        let cap = self.cap.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let ask = || -> Result<String, String> {
                    let messages = vec![lca_protocol::ChatMessage::text(
                        lca_protocol::MessageRole::User,
                        "conformance completion request",
                    )];
                    cap.complete(messages)
                        .map(|(text, _usage)| text)
                        .map_err(|err| err.to_string())
                };
                compact_script(&excerpts, Some(&ask as &dyn Fn() -> Result<String, String>))
                    .map_err(|reason| {
                        // Mirror the host's wrapping byte for byte so
                        // the refusal reads identically in both modes
                        // (NFR-25): to_dispatch prefixes the extension
                        // and the invalid-arguments class, the host's
                        // compact work prefixes the refusal.
                        lca_protocol::DispatchError::Failed(format!(
                            "{}: invalid arguments: compaction refused: {reason}",
                            "conformance"
                        ))
                    })
            })
            .await
            .map_err(|_| lca_protocol::DispatchError::Failed("conformance panicked".into()))
            .and_then(std::convert::identity)
        })
    }

    fn transform_messages(
        &self,
        messages: Vec<lca_protocol::ChatMessage>,
    ) -> lca_ext_abi::DispatchFuture<
        'static,
        Result<Result<Vec<lca_protocol::ChatMessage>, String>, lca_protocol::DispatchError>,
    > {
        Box::pin(std::future::ready(Ok(transform_script(messages))))
    }

    fn provider_models(
        &self,
        settings: &[(String, String)],
    ) -> Result<Vec<lca_protocol::ModelInfo>, lca_protocol::DispatchError> {
        Ok(crate::provider_models(settings))
    }

    fn stream_completion<'a>(
        &'a self,
        request: lca_protocol::CompletionRequest,
        sink: &'a dyn lca_protocol::EventSink,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
        // The script is deterministic, so both modes push the same
        // events in the same order before completing (NFR-25).
        for event in crate::scripted_events(&request.model) {
            sink.push(event);
        }
        Box::pin(std::future::ready(Ok(())))
    }

    fn identity_login(
        &self,
    ) -> lca_ext_abi::DispatchFuture<
        'static,
        Result<lca_protocol::IdentityOutcome, lca_protocol::DispatchError>,
    > {
        let cap = self.cap.clone();
        // Lazy: the oauth flow blocks in `oauth_await`, so the caller
        // must be able to run this future off the test thread.
        Box::pin(async move { Ok(crate::scripted_login(&NativeCap(cap))) })
    }

    fn identity_logout(
        &self,
    ) -> lca_ext_abi::DispatchFuture<
        'static,
        Result<lca_protocol::IdentityOutcome, lca_protocol::DispatchError>,
    > {
        Box::pin(std::future::ready(Ok(crate::scripted_logout())))
    }

    fn identity_usage(
        &self,
    ) -> lca_ext_abi::DispatchFuture<
        'static,
        Result<
            Result<lca_protocol::Usage, lca_protocol::IdentityOutcome>,
            lca_protocol::DispatchError,
        >,
    > {
        Box::pin(std::future::ready(Ok(Ok(crate::scripted_usage_report()))))
    }

    fn login_options(
        &self,
    ) -> lca_ext_abi::DispatchFuture<
        'static,
        Result<Vec<lca_protocol::LoginOption>, lca_protocol::DispatchError>,
    > {
        Box::pin(std::future::ready(Ok(crate::scripted_login_options())))
    }

    fn login_submit(
        &self,
        answer: lca_protocol::LoginAnswer,
    ) -> lca_ext_abi::DispatchFuture<
        'static,
        Result<Vec<(String, String)>, lca_protocol::DispatchError>,
    > {
        Box::pin(std::future::ready(
            crate::scripted_login_submit(&answer).map_err(lca_protocol::DispatchError::Failed),
        ))
    }

    fn interrupt(&self) {
        // A native call shares the caller's thread, so there is no epoch
        // to bump: flag the capability engine directly, and a blocked
        // host wait (the oauth callback) polls its way out (FR-CONC-1,
        // NFR-21). This is the pattern a native extension with a
        // blocking host wait must follow - together with `turn_started`
        // below, which clears the flag again.
        self.cap.cancel();
    }

    fn turn_started(&self) {
        // The other half of `interrupt` (FR-CONC-1's turn boundary): clear
        // the flag the previous turn's cancel left behind. The WASM host
        // does the same in its own `turn_started`; the native/WASM diff in
        // the conformance run depends on the two behaving alike.
        self.cap.reset_cancellation();
    }

    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, lca_protocol::DispatchError> {
        Ok(vec![self.schema()])
    }

    fn execute_tool<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<
        'a,
        Result<lca_protocol::ToolResult, lca_protocol::DispatchError>,
    > {
        let result = self.execute(call);
        Box::pin(std::future::ready(Ok(result)))
    }

    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, lca_protocol::DispatchError> {
        Ok(vec![command_leaf()])
    }

    fn invoke_command(
        &self,
        _name: &str,
        argument: &str,
    ) -> Result<lca_protocol::CommandEffect, lca_protocol::DispatchError> {
        Ok(invoke_command(argument))
    }

    fn on_pre_tool_use<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<
        'a,
        Result<lca_protocol::HookAction, lca_protocol::DispatchError>,
    > {
        let action = pre_tool_action(&call.name);
        Box::pin(std::future::ready(Ok(action)))
    }

    fn on_pre_turn(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a lca_protocol::PostToolObservation,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_session_close(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}
