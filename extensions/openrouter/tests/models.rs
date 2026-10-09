//! The OpenRouter catalog + dispatch surface (gh #185): live
//! discovery, the curated floor, and the manifest/grants agreement.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::{mock_server, sandbox};

// Verifies: gh #185 - the catalog reads the gateway's own `/models`
// (ids with their `context_length`) when it answers.
#[test]
fn discovery_reads_the_gateway_catalog() {
    let mock = mock_server();
    let cap = sandbox(
        "openrouter",
        "discovery",
        &mock,
        openrouter::manifest_grants(),
    );
    lca_protocol::ProviderCap::credentials_set(&*cap, "access", "sk-or-mock").expect("seed");
    let models = openrouter::list_models(cap.as_ref());
    let found = models
        .iter()
        .find(|model| model.id == "mock/test-model")
        .expect("the mock row lists");
    assert_eq!(found.context_window, 32000);
    assert!(
        models.iter().any(|model| model.id == "openai/gpt-4o"),
        "the gateway row lists too"
    );
}

// Verifies: gh #185 - an unreachable catalog falls back to the curated
// floor (the preset's rows), never an empty picker.
#[test]
fn an_unreachable_catalog_falls_back_to_curated() {
    let mock = mock_server();
    mock.fail_once("/models", 500, Some("{}"));
    let cap = sandbox(
        "openrouter",
        "fallback",
        &mock,
        openrouter::manifest_grants(),
    );
    lca_protocol::ProviderCap::credentials_set(&*cap, "access", "sk-or-mock").expect("seed");
    let models = openrouter::list_models(cap.as_ref());
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "openai/gpt-4o",
            "anthropic/claude-3.5-sonnet",
            "google/gemini-2.0-flash",
        ]
    );
}

// Verifies: gh #185 - the manifest file and the native grants agree,
// and the login surface stays option-less (the identity flow runs
// directly, like codex).
#[test]
fn manifest_toml_and_native_grants_agree() {
    let manifest: toml::Value = openrouter::MANIFEST.parse().expect("MANIFEST parses");
    let hosts = manifest["capabilities"]["net"]["hosts"]
        .as_array()
        .expect("hosts list");
    let names: Vec<&str> = hosts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names, vec!["openrouter.ai"]);
    assert_eq!(
        manifest["capabilities"]["oauth"]["redirect_path"].as_str(),
        Some("/callback")
    );
    assert_eq!(
        manifest["capabilities"]["credentials"]["namespace"].as_str(),
        Some("openrouter")
    );
    assert!(
        manifest["capabilities"].get("env").is_none(),
        "no env capability (waits on #170)"
    );
    let grants = openrouter::manifest_grants();
    assert!(
        grants.net.iter().any(|p| p.matches("openrouter.ai", 443)),
        "the gateway is granted"
    );
    assert!(grants.oauth.is_some(), "the loopback flow is granted");
    assert!(grants.credentials, "the namespace is granted");
    assert!(
        openrouter::login_options().is_empty(),
        "no picker presets: the identity flow runs directly"
    );
}
