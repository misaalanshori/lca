//! The Kimi catalog + dispatch surface (gh #187): the static table
//! and the manifest/grants agreement.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::{mock_server, sandbox};

// Verifies: gh #187 - the static table names the issue's rows, both
// window-unknown (0) until livedata says otherwise.
#[test]
fn the_model_table_is_grounded() {
    let mock = mock_server();
    let cap = sandbox(
        "kimi-coding",
        "models",
        &mock,
        kimi_coding::manifest_grants(),
    );
    let models = kimi_coding::list_models(cap.as_ref());
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, vec!["kimi-k2-coding", "kimi-latest"]);
    for model in &models {
        assert_eq!(model.context_window, 0, "{} stays unknown", model.id);
    }
}

// Verifies: gh #187 - the manifest file and the native grants agree.
#[test]
fn manifest_toml_and_native_grants_agree() {
    let manifest: toml::Value = kimi_coding::MANIFEST.parse().expect("MANIFEST parses");
    let hosts = manifest["capabilities"]["net"]["hosts"]
        .as_array()
        .expect("hosts list");
    let names: Vec<&str> = hosts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names, vec!["auth.kimi.com", "api.kimi.com"]);
    assert!(
        manifest["capabilities"].get("oauth").is_none(),
        "device flows bind no loopback"
    );
    assert_eq!(
        manifest["capabilities"]["credentials"]["namespace"].as_str(),
        Some("kimi-coding")
    );
    let grants = kimi_coding::manifest_grants();
    for host in ["auth.kimi.com", "api.kimi.com"] {
        assert!(
            grants.net.iter().any(|p| p.matches(host, 443)),
            "{host} granted"
        );
    }
    assert!(grants.oauth.is_none(), "no loopback in the grants either");
    assert!(grants.credentials, "the namespace is granted");
}
