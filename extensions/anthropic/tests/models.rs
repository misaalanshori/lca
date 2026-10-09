//! The Anthropic catalog + dispatch surface (gh #183): the static
//! table and the manifest/grants agreement.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::{mock_server, sandbox};

// Verifies: gh #183 - the static table names the issue's catalog with
// Anthropic's documented windows and ceilings; no guessed rows.
#[test]
fn the_model_table_is_grounded() {
    let mock = mock_server();
    let cap = sandbox("anthropic", "models", &mock, anthropic::manifest_grants());
    let models = anthropic::list_models(cap.as_ref());
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "claude-3-7-sonnet",
            "claude-3-5-sonnet",
            "claude-3-5-haiku",
            "claude-3-opus",
        ]
    );
    for model in &models {
        assert_eq!(model.context_window, 200000, "{}", model.id);
        assert!(model.max_tokens > 0, "{} publishes a ceiling", model.id);
    }
}

// Verifies: gh #183 - the manifest file and the native grants agree:
// the hosts, the loopback path, and the namespace say the same thing
// both ways.
#[test]
fn manifest_toml_and_native_grants_agree() {
    let manifest: toml::Value = anthropic::MANIFEST.parse().expect("MANIFEST parses");
    let hosts = manifest["capabilities"]["net"]["hosts"]
        .as_array()
        .expect("hosts list");
    let names: Vec<&str> = hosts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        names,
        vec!["api.anthropic.com", "claude.ai", "platform.claude.com"]
    );
    assert_eq!(
        manifest["capabilities"]["oauth"]["redirect_path"].as_str(),
        Some("/callback")
    );
    assert_eq!(
        manifest["capabilities"]["credentials"]["namespace"].as_str(),
        Some("anthropic")
    );
    assert_eq!(manifest["worlds"].as_array().map(Vec::len), Some(1));
    let grants = anthropic::manifest_grants();
    for host in ["api.anthropic.com", "claude.ai", "platform.claude.com"] {
        assert!(
            grants.net.iter().any(|p| p.matches(host, 443)),
            "{host} granted"
        );
    }
    assert!(grants.oauth.is_some(), "the loopback flow is granted");
    assert!(grants.credentials, "the namespace is granted");
}

// Verifies: gh #183 - `/login anthropic` offers all three choices: the
// key, the browser subscription, and the copy-code subscription.
#[test]
fn login_offers_the_key_and_both_subscription_methods() {
    let options = anthropic::login_options();
    let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            anthropic::CHOICE_API_KEY,
            anthropic::CHOICE_SUBSCRIPTION,
            anthropic::CHOICE_COPY_CODE,
        ]
    );
    assert!(
        options
            .iter()
            .find(|option| option.id == anthropic::CHOICE_API_KEY)
            .is_some_and(|option| option.fields == vec!["api-key".to_string()]),
        "the key asks for its secret"
    );
}
