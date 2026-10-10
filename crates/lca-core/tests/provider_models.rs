//! gh #236: a provider handle that traps during model listing warns
//! loudly instead of dissolving into an empty list.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_core::ExtensionProvider;
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::{CommandEffect, DispatchError, HookAction, PostToolObservation, ToolCall};
use lca_provider::Provider as _;

/// A provider-world handle whose listing traps the way an exhausted
/// fuel budget does (gh #236): the guest call dies, the host answers
/// `Err`, and the adapter must say so out loud.
struct TrappingProvider;

impl ExtensionDispatch for TrappingProvider {
    fn name(&self) -> &str {
        "trapping"
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<World> {
        vec![World::Provider]
    }

    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: "trapping".to_string(),
            world: "tool",
        })))
    }

    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Ok(CommandEffect::None)
    }

    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(HookAction::Allow)))
    }

    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a PostToolObservation,
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

    fn provider_models(
        &self,
        _settings: &[(String, String)],
    ) -> Result<Vec<lca_protocol::ModelInfo>, DispatchError> {
        Err(DispatchError::Failed(
            "trapping: fuel budget exhausted".to_string(),
        ))
    }
}

// Verifies: gh #236 - a trapped listing warns loudly (the failure
// mode errors loud now) and still answers empty for the picker.
#[test]
fn a_trapped_listing_warns_loudly_and_answers_empty() {
    let warned = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = warned.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || TestWriter(sink.clone()))
        .finish();
    let provider = ExtensionProvider::new(Arc::new(TrappingProvider));
    let listed = tracing::dispatcher::with_default(&tracing::Dispatch::new(subscriber), || {
        provider.list_models()
    });
    assert!(listed.is_empty(), "the picker still gets an answer");
    let warned = warned.lock().expect("log").join("\n");
    assert!(
        warned.contains("trapping") && warned.contains("fuel budget exhausted"),
        "the trap is named in diagnostics: {warned}"
    );
}

struct TestWriter(Arc<Mutex<Vec<String>>>);

impl std::io::Write for TestWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log")
            .push(String::from_utf8_lossy(buf).into_owned());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
