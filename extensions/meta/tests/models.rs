//! The Meta catalog + dispatch surface (gh #186): the static table
//! and the manifest/grants agreement.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::{mock_server, sandbox};

// Verifies: gh #186 - the static table names the issue's rows: the
// Llama row with Meta's published window, the Spark row unknown (0).
#[test]
fn the_model_table_is_grounded() {
    let mock = mock_server();
    let cap = sandbox("meta", "models", &mock, meta::manifest_grants());
    let models = meta::list_models(cap.as_ref());
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, vec!["llama-3.3-70b-instruct", "muse-spark"]);
    assert_eq!(models[0].context_window, 128000);
    assert_eq!(models[1].context_window, 0, "unknown means unknown");
}

// Verifies: gh #186 - the manifest file and the native grants agree,
// and the login surface stays option-less.
#[test]
fn manifest_toml_and_native_grants_agree() {
    let manifest: toml::Value = meta::MANIFEST.parse().expect("MANIFEST parses");
    let hosts = manifest["capabilities"]["net"]["hosts"]
        .as_array()
        .expect("hosts list");
    let names: Vec<&str> = hosts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names, vec!["auth.meta.com", "api.meta.ai"]);
    assert!(
        manifest["capabilities"].get("oauth").is_none(),
        "device flows bind no loopback"
    );
    assert_eq!(
        manifest["capabilities"]["credentials"]["namespace"].as_str(),
        Some("meta")
    );
    let grants = meta::manifest_grants();
    assert!(grants.oauth.is_none(), "no loopback in the grants either");
    assert!(grants.credentials, "the namespace is granted");
    assert!(
        meta::login_options().is_empty(),
        "no picker presets: the identity flow runs directly"
    );
}
