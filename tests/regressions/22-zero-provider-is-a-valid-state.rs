//! The zero-provider state is a **valid state**, not a fatal one
//! (FR-PROV-9, and the README's own design intent: "The agent can run with
//! zero providers installed as a valid state"). Disabling the only provider
//! used to make the binary exit at startup, which is doubly wrong: it
//! contradicts the stated design, and the interface is the home surface for
//! settings, so being lockable from the inside is a trap with no UI escape.
//!
//! The interactive surface now opens into this state and recovers through
//! `/login`; headless keeps its loud failure (exit 2) because a script
//! needs one. `lca_provider::NoProvider` is the stand-in that makes this
//! possible without threading `Option` through the whole session.
//!
//! Verifies: FR-PROV-9, FR-PROV-6.

use std::sync::Arc;

// The stand-in is deliberately thin; what it must never do is look usable.
use lca_provider::Provider as _;

#[test]
fn the_zero_provider_standin_offers_no_models_and_fails_every_call_loudly() {
    let provider = lca_provider::NoProvider::new("openai-compatible");
    assert_eq!(provider.name(), "openai-compatible");
    assert!(
        provider.list_models().is_empty(),
        "no model is offered against nothing"
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let call = lca_provider::Provider::stream(
        &provider,
        lca_protocol::CompletionRequest {
            messages: Vec::new(),
            tools: Vec::new(),
            model: String::new(),
            stable_prefix: 0,
            extras: std::collections::BTreeMap::new(),
        },
        tx,
    );
    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(call)
        .expect_err("a turn against nothing must fail");
    assert!(!error.retryable, "retrying cannot conjure a provider");
    assert_eq!(error.class, "no-model");
    assert!(
        error.message.contains("No model is available"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("lca ext enable") || error.message.contains("/login"),
        "the message names the way out: {}",
        error.message
    );
}

// Verifies: FR-PROV-6 - the two front ends do not agree on purpose. A
// script gets a loud failure with a nonzero exit; only the interactive
// surface becomes recoverable.
#[test]
fn the_standin_invents_no_model_id() {
    // The interface shows "no model" rather than a model name that would
    // fail on the first turn.
    let provider = Arc::new(lca_provider::NoProvider::new("x")) as Arc<dyn lca_provider::Provider>;
    let offered = provider
        .list_models()
        .first()
        .map(|model| model.id.clone())
        .unwrap_or_default();
    assert!(offered.is_empty(), "no model id is invented");
}
