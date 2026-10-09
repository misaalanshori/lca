//! The Copilot catalog + dispatch surface (gh #184): live policy
//! parsing, the curated floor, and the manifest/grants agreement.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;

mod common;

use common::{mock_server, sandbox};

fn live_cap(mock: &common::Mock, name: &str) -> Arc<lca_tools::Capabilities> {
    let cap = sandbox(
        "github-copilot",
        name,
        mock,
        github_copilot::manifest_grants(),
    );
    lca_protocol::ProviderCap::credentials_set(&*cap, "access", "tid=mock").expect("seed");
    lca_protocol::ProviderCap::credentials_set(&*cap, "expires", &u64::MAX.to_string())
        .expect("seed");
    cap
}

// Verifies: gh #184 - the live catalog lists picker and
// policy-enabled rows, skipping disabled and tool-less ones.
#[test]
fn discovery_lists_what_the_account_may_use() {
    let mock = mock_server();
    let cap = live_cap(&mock, "discovery");
    let models = github_copilot::list_models(cap.as_ref());
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert!(ids.contains(&"gpt-4o"), "picker rows list: {ids:?}");
    assert!(
        ids.contains(&"mock/policy-model"),
        "policy rows list: {ids:?}"
    );
    assert!(
        !ids.contains(&"mock/disabled-model"),
        "disabled rows never list: {ids:?}"
    );
    assert!(
        !ids.contains(&"mock/text-only"),
        "tool-less rows never list: {ids:?}"
    );
}

// Verifies: gh #184 - an unreachable gateway falls back to the
// issue's curated rows, never an empty picker.
#[test]
fn an_unreachable_gateway_falls_back_to_curated() {
    let mock = mock_server();
    mock.fail_once("/models", 500, Some("{}"));
    let cap = live_cap(&mock, "fallback");
    let models = github_copilot::list_models(cap.as_ref());
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "gpt-4o",
            "claude-3.5-sonnet",
            "claude-3.7-sonnet",
            "o1",
            "o3-mini"
        ]
    );
}

// Verifies: gh #184 - the manifest file and the native grants agree
// (every Copilot API host, no loopback section for a device flow),
// and the login surface is the one device choice with its optional
// enterprise field.
#[test]
fn manifest_toml_and_native_grants_agree() {
    let manifest: toml::Value = github_copilot::MANIFEST.parse().expect("MANIFEST parses");
    let hosts = manifest["capabilities"]["net"]["hosts"]
        .as_array()
        .expect("hosts list");
    let names: Vec<&str> = hosts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "github.com",
            "api.github.com",
            "api.githubcopilot.com",
            "api.individual.githubcopilot.com",
            "api.business.githubcopilot.com",
            "api.enterprise.githubcopilot.com",
        ]
    );
    assert!(
        manifest["capabilities"].get("oauth").is_none(),
        "device flows bind no loopback"
    );
    assert_eq!(
        manifest["capabilities"]["credentials"]["namespace"].as_str(),
        Some("github-copilot")
    );
    let grants = github_copilot::manifest_grants();
    assert!(grants.oauth.is_none(), "no loopback in the grants either");
    assert!(grants.credentials, "the namespace is granted");
    let options = github_copilot::login_options();
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].id, github_copilot::CHOICE_DEVICE);
    assert_eq!(options[0].fields, vec!["enterprise-domain".to_string()]);
}
