//! Regression rows for gh #356: a provider's structured failure must cross
//! the dispatch boundary intact.
//!
//! Every wire kit produces `StreamFailure { message, class, retryable }`.
//! The native bridges then flattened it into `DispatchError::Failed(String)`,
//! and `ExtensionProvider::stream` re-derived `(class, retryable)` by looking
//! for `"provider returned HTTP NNN"` in the text. Anything that did not
//! literally contain that phrase - `invalid_api_key`, `authentication
//! failed`, `the codex session has expired or was revoked`, a vendor's own
//! 400 validation message - was classified `("transport", true)`, so the turn
//! loop retried a permanent auth failure three times with backoff and then
//! exited on the wrong code.
//!
//! Verifies: gh #356 - a `ProviderFailed` carries its own class and
//! retryability, and the text fallback recognizes auth refusals.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::DispatchError;

/// A structured provider failure reports exactly the class and retryability
/// it was built with, with no message inspection.
#[test]
fn a_provider_failure_carries_its_own_classification() {
    let err = DispatchError::provider_failed(
        "codex",
        "the codex session has expired or was revoked; run /login codex",
        "auth",
        false,
    );
    match &err {
        DispatchError::ProviderFailed {
            extension,
            message,
            class,
            retryable,
        } => {
            assert_eq!(extension, "codex");
            assert!(message.contains("/login codex"), "{message}");
            assert_eq!(*class, "auth");
            assert!(!*retryable, "an expired session is not retryable");
        }
        other => panic!("expected ProviderFailed, got {other:?}"),
    }
    // The display form still names the extension and the message, so a
    // log line stays readable without the structured fields.
    let shown = err.to_string();
    assert!(shown.contains("codex"), "{shown}");
    assert!(shown.contains("expired"), "{shown}");
}

/// A transport failure keeps `retryable: true`, and an invalid one keeps
/// `false` - the two directions that decide whether the turn loop retries.
#[test]
fn retryability_survives_the_boundary_in_both_directions() {
    let transport = DispatchError::provider_failed("openai", "connection reset", "transport", true);
    match transport {
        DispatchError::ProviderFailed {
            class, retryable, ..
        } => {
            assert_eq!(class, "transport");
            assert!(retryable);
        }
        other => panic!("expected ProviderFailed, got {other:?}"),
    }

    let invalid =
        DispatchError::provider_failed("openai", "messages[2].role is not valid", "invalid", false);
    match invalid {
        DispatchError::ProviderFailed {
            class, retryable, ..
        } => {
            assert_eq!(class, "invalid");
            assert!(!retryable, "a malformed request is not worth repeating");
        }
        other => panic!("expected ProviderFailed, got {other:?}"),
    }
}

/// The text fallback must recognize the auth refusals a WASM guest can only
/// express as a string (WIT 0.6's `stream-completion` returns
/// `result<_, string>`). These all used to classify as retryable transport
/// failures.
#[test]
fn an_auth_refusal_in_plain_text_is_not_retried() {
    for message in [
        "invalid_api_key",
        "Invalid API key provided",
        "authentication failed: bad token",
        "unauthorized",
        "the codex session has expired or was revoked; run /login codex",
        "no anthropic login yet; run /login anthropic",
        "run /login openrouter first",
        "401 Unauthorized: missing bearer token",
    ] {
        let (class, retryable) = lca_core::ext_provider::classify_message(message);
        assert_eq!(class, "auth", "{message:?} classifies as auth");
        assert!(!retryable, "{message:?} is not retried");
    }
}

/// The HTTP-status heuristic keeps working: it is the fallback, not
/// something the structured path replaces.
#[test]
fn the_http_status_heuristic_still_classifies() {
    let cases = [
        ("provider returned HTTP 401: nope", "auth", false),
        ("provider returned HTTP 403: nope", "auth", false),
        ("provider returned HTTP 400: bad", "invalid", false),
        ("provider returned HTTP 404: gone", "invalid", false),
        // 429 keeps its pre-existing `invalid` class with `retryable: true`.
        // The class is display-only (only `retryable` diverts the loop), and
        // re-labelling the throttling bucket is outside gh #356's scope.
        ("provider returned HTTP 429: slow down", "invalid", true),
        // Same shape as 429: a 4xx keeps the `invalid` class but is flagged
        // retryable. Pre-existing and display-only.
        ("provider returned HTTP 408: timeout", "invalid", true),
        ("provider returned HTTP 500: oops", "transport", true),
        ("provider returned HTTP 503: later", "transport", true),
    ];
    for (message, want_class, want_retry) in cases {
        let (class, retryable) = lca_core::ext_provider::classify_message(message);
        assert_eq!((class, retryable), (want_class, want_retry), "{message:?}");
    }
}

/// A refusal that is not auth-shaped stays non-retryable-invalid, as before.
#[test]
fn a_permission_refusal_stays_invalid_and_unretried() {
    for message in [
        "permission denied",
        "capability not granted",
        "invalid argument: rows",
    ] {
        let (class, retryable) = lca_core::ext_provider::classify_message(message);
        assert_eq!((class, retryable), ("invalid", false), "{message:?}");
    }
}

/// Anything unrecognized keeps the historical "transport, retry once the
/// backoff elapses" default - the hardening must not turn unknown failures
/// into hard errors.
#[test]
fn an_unrecognized_failure_stays_retryable_transport() {
    let (class, retryable) = lca_core::ext_provider::classify_message("something odd happened");
    assert_eq!((class, retryable), ("transport", true));
}

/// The wiring half: `ExtensionProvider::stream` must consume a
/// `ProviderFailed` as-is rather than stringifying it and re-guessing. This
/// is the row that catches a fix which adds the variant but leaves the
/// adapter flattening it.
#[test]
fn the_provider_adapter_forwards_a_structured_failure_unchanged() {
    use std::sync::Arc;

    use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
    use lca_protocol::{CompletionRequest, DispatchError, EventSink};
    use lca_provider::Provider;

    /// A provider double that fails with a class the message text does not
    /// spell out, so a text heuristic cannot recover it.
    struct Failing;
    impl ExtensionDispatch for Failing {
        fn name(&self) -> &str {
            "codex"
        }
        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }
        fn worlds(&self) -> Vec<World> {
            vec![World::Provider]
        }
        fn stream_completion<'a>(
            &'a self,
            _request: CompletionRequest,
            _sink: &'a dyn EventSink,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(async {
                Err(DispatchError::provider_failed(
                    "codex",
                    "the session is gone; sign in again",
                    "auth",
                    false,
                ))
            })
        }
        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Ok(Vec::new())
        }
        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            Box::pin(async {
                Err(DispatchError::MissingWorld {
                    extension: "codex".to_string(),
                    world: "tool",
                })
            })
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
        fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(async { Ok(()) })
        }
        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
            Box::pin(async { Ok(lca_protocol::HookAction::Allow) })
        }
        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(async { Ok(()) })
        }
        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(async { Ok(()) })
        }
        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(async { Ok(()) })
        }
        fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(async { Ok(()) })
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let handle: Arc<dyn ExtensionDispatch> = Arc::new(Failing);
        let bridge = lca_core::ExtensionProvider::new(handle);
        let request = CompletionRequest {
            messages: Vec::new(),
            tools: Vec::new(),
            model: "anything".to_string(),
            stable_prefix: 0,
            extras: Default::default(),
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let err = bridge.stream(request, tx).await.expect_err("it fails");
        assert_eq!(
            err.class, "auth",
            "the kit's class survives: {}",
            err.message
        );
        assert!(
            !err.retryable,
            "and its retryability does too - a text heuristic would have \
             retried this three times: {}",
            err.message
        );
        assert!(
            err.message.contains("codex") && err.message.contains("sign in again"),
            "the message still names the extension and the cause: {}",
            err.message
        );
    });
}
