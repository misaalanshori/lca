//! ADR-0012's host side: identity exports become namespaced commands
//! automatically, the generic commands dispatch across installed
//! providers, and the provider world reaches the core through one
//! adapter regardless of delivery mode.

use std::sync::Arc;

use lca_core::ExtensionRegistry;
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::CommandEffect;
use lca_protocol::{DispatchError, IdentityOutcome, ModelInfo, Usage};
use lca_provider::Provider as _;

/// A minimal second provider so the generic picker has a real choice.
struct FakeProvider {
    label: &'static str,
}

impl ExtensionDispatch for FakeProvider {
    fn name(&self) -> &str {
        self.label
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
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.label.to_string(),
            world: "tool",
        })))
    }

    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Ok(CommandEffect::None)
    }

    fn provider_models(&self) -> Result<Vec<ModelInfo>, DispatchError> {
        Ok(Vec::new())
    }

    fn stream_completion<'a>(
        &'a self,
        _request: lca_protocol::CompletionRequest,
        sink: &'a dyn lca_protocol::EventSink,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        sink.push(lca_protocol::StreamEvent::Usage {
            usage: Usage {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_write: 0,
                cache_write_1h: 0,
                cost: 0.5,
                cost_input: 0.0,
                cost_cache_read: 0.0,
                cost_cache_write: 0.0,
                extras: Default::default(),
            },
        });
        Box::pin(std::future::ready(Ok(())))
    }

    fn identity_login(&self) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        Box::pin(std::future::ready(Ok(IdentityOutcome::Ok)))
    }

    fn identity_logout(&self) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        Box::pin(std::future::ready(Ok(IdentityOutcome::Ok)))
    }

    fn identity_usage(
        &self,
    ) -> DispatchFuture<'static, Result<Result<Usage, IdentityOutcome>, DispatchError>> {
        Box::pin(std::future::ready(Ok(Ok(Usage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 0,
            cache_write_1h: 0,
            cost: 0.5,
            cost_input: 0.0,
            cost_cache_read: 0.0,
            cost_cache_write: 0.0,
            extras: Default::default(),
        }))))
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

fn two_providers() -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(FakeProvider { label: "alpha" }));
    registry.register(Arc::new(FakeProvider { label: "beta" }));
    registry
}

// Verifies: FR-PROV-11 (the generic command dispatches from the
// interface's own thread - a thread a runtime is already driving,
// where building a second runtime panics; the bridge must survive
// that context, which a plain sync test never exercises).
#[tokio::test]
async fn the_generic_command_dispatches_from_inside_a_running_runtime() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(FakeProvider { label: "solo" }));
    let effect = registry.invoke_generic("usage", "", "solo");
    assert!(
        effect.is_some(),
        "the identity op answered from a live runtime"
    );
}

fn widget(effect: Option<CommandEffect>) -> String {
    match effect {
        Some(CommandEffect::ShowWidget(text)) => text,
        other => panic!("expected a notice, got {other:?}"),
    }
}

// Verifies: FR-PROV-10 — every provider extension gets its identity
// exports registered as commands namespaced under its own name, with no
// collision risk between two providers that both export `usage`.
#[test]
fn identity_exports_are_registered_namespaced() {
    let registry = two_providers();
    let names = registry.command_names();
    for expected in [
        "alpha.login",
        "alpha.logout",
        "alpha.usage",
        "beta.login",
        "beta.logout",
        "beta.usage",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "{expected} missing: {names:?}"
        );
    }
}

// Verifies: FR-PROV-10, ADR-0012 — the namespaced forms dispatch to
// their own provider's export: usage prints the standard shape, logout
// answers honestly when the provider has none.
#[test]
fn namespaced_commands_dispatch_to_their_own_provider() {
    let registry = two_providers();

    let usage = widget(registry.invoke_command("alpha.usage", ""));
    assert!(usage.contains("input 1"), "{usage}");
    assert!(usage.contains("cache read 3"), "{usage}");

    let other = widget(registry.invoke_command("beta.usage", ""));
    assert_eq!(usage, other, "same script, both report the same numbers");

    // The conformance extension's not-supported path (ADR-0012).
    let mut conformance = ExtensionRegistry::new();
    conformance.register(Arc::new(FakeProviderWithNotSupported));
    let logout = widget(conformance.invoke_command("no-login.logout", ""));
    assert!(logout.contains("not supported"), "{logout}");
}

struct FakeProviderWithNotSupported;

impl ExtensionDispatch for FakeProviderWithNotSupported {
    fn name(&self) -> &str {
        "no-login"
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
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.name().to_string(),
            world: "tool",
        })))
    }
    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn invoke_command(&self, _n: &str, _a: &str) -> Result<CommandEffect, DispatchError> {
        Ok(CommandEffect::None)
    }
    fn identity_login(&self) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        Box::pin(std::future::ready(Ok(IdentityOutcome::NotSupported)))
    }
    fn identity_logout(&self) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        Box::pin(std::future::ready(Ok(IdentityOutcome::NotSupported)))
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

// Verifies: FR-PROV-11 — the generic login lists every installed
// provider by name when several are installed, and invokes the chosen
// one when the user names it.
#[test]
fn generic_login_lists_providers_and_invokes_the_chosen_one() {
    let registry = two_providers();

    // Two installed, no choice named: the response lists them by name.
    let ambiguous = widget(registry.invoke_generic("login", "", "alpha"));
    assert!(ambiguous.contains("alpha"), "{ambiguous}");
    assert!(ambiguous.contains("beta"), "{ambiguous}");

    // The chosen one runs.
    let chosen = widget(registry.invoke_generic("login", "beta", "alpha"));
    assert!(chosen.contains("`beta`"), "{chosen}");

    // One installed: no argument needed.
    let mut single = ExtensionRegistry::new();
    single.register(Arc::new(FakeProvider { label: "solo" }));
    let solo = widget(single.invoke_generic("login", "", "solo"));
    assert!(solo.contains("`solo`"), "{solo}");
}

// Verifies: FR-PROV-11 — /logout and /usage follow the active provider,
// and a setup with no providers answers plainly (FR-PROV-6's shape for
// the identity commands).
#[test]
fn generic_usage_follows_the_active_provider() {
    let registry = two_providers();

    let usage = widget(registry.invoke_generic("usage", "", "alpha"));
    assert!(usage.contains("input 1"), "{usage}");

    let logout = widget(registry.invoke_generic("logout", "", "beta"));
    assert!(logout.contains("`beta`"), "{logout}");

    // Zero enabled providers is a valid state (FR-PROV-9).
    let empty = ExtensionRegistry::new();
    let none = empty.invoke_generic("usage", "", "openai-compatible");
    assert!(
        none.is_none(),
        "no provider registered: the caller reports FR-PROV-6, not the registry"
    );
}

// Verifies: FR-PROV-6's data path — a configured provider name that no
// enabled extension answers to resolves to nothing, which is what the
// agent's "no model is available" report keys off.
#[test]
fn an_unknown_active_provider_resolves_to_nothing() {
    let registry = two_providers();
    assert!(registry.provider("openai-compatible").is_none());
    assert!(registry.provider("alpha").is_some());
    assert_eq!(registry.provider_names(), vec!["alpha", "beta"]);
}

// Verifies: FR-PROV-9 — disabling a provider removes it from the
// installed list and from generic dispatch; zero enabled providers
// stays a supported state.
#[test]
fn a_disabled_provider_leaves_zero_enabled() {
    let mut registry = two_providers();
    registry.set_enabled("alpha", false);
    registry.set_enabled("beta", false);
    assert!(registry.provider_names().is_empty());
    assert!(registry.provider("alpha").is_none());
    let none = registry.invoke_generic("login", "", "");
    assert!(none.is_none(), "zero enabled providers is valid: {none:?}");
}

// Verifies: ADR-0004 through ADR-0019's adapter — the provider world's
// events reach the core's Provider trait case for case, whatever the
// delivery mode, so the turn loop needs no mode branch.
#[test]
fn the_adapter_streams_the_script_into_the_core_channel() {
    let handle: Arc<dyn ExtensionDispatch> = Arc::new(FakeProvider { label: "alpha" });
    let bridge = lca_core::ExtensionProvider::new(handle);
    assert_eq!(bridge.name(), "alpha");
    assert!(bridge.list_models().is_empty());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let request = lca_protocol::CompletionRequest {
            messages: Vec::new(),
            tools: Vec::new(),
            model: "anything".to_string(),
            stable_prefix: 0,
            extras: Default::default(),
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        bridge.stream(request, tx).await.expect("stream");
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        assert!(!events.is_empty(), "the fake pushed its usage event");
        assert!(matches!(
            events.last(),
            Some(lca_protocol::StreamEvent::Usage { .. })
        ));
    });
}
