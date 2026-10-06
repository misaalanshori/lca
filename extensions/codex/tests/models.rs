//! The Codex catalog + dispatch surface (gh #180): the static
//! table and the native handle's identity/model forwarding.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::{mock_server, sandbox};

// Verifies: gh #180 - the static table names the grounded model with
// its window; no guessed rows.
#[test]
fn the_model_table_is_grounded() {
    let mock = mock_server();
    let cap = sandbox(
        "codex",
        "models",
        &mock,
        codex::manifest_grants(),
        "test-client",
    );
    let models = codex::list_models(cap.as_ref());
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-5.4-mini");
    assert_eq!(models[0].context_window, 272000);
}

// Verifies: gh #180 - the manifest file and the native grants agree:
// the hosts, the loopback path, and the namespace say the same thing
// both ways.
#[test]
fn manifest_toml_and_native_grants_agree() {
    let manifest: toml::Value = codex::MANIFEST.parse().expect("MANIFEST parses");
    let hosts = manifest["capabilities"]["net"]["hosts"]
        .as_array()
        .expect("hosts list");
    let names: Vec<&str> = hosts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names, vec!["chatgpt.com", "auth.openai.com"]);
    assert_eq!(
        manifest["capabilities"]["oauth"]["redirect_path"].as_str(),
        Some("/callback")
    );
    assert_eq!(
        manifest["capabilities"]["credentials"]["namespace"].as_str(),
        Some("codex")
    );
    let grants = codex::manifest_grants();
    for host in ["chatgpt.com", "auth.openai.com"] {
        assert!(
            grants.net.iter().any(|p| p.matches(host, 443)),
            "{host} granted"
        );
    }
}

// Verifies: gh #180 - the native handle forwards identity and models
// through the dispatch seam (login options stay empty: the flow, not
// the picker).
#[test]
fn the_native_handle_serves_the_dispatch_seam() {
    use lca_ext_abi::ExtensionDispatch;
    let mock = mock_server();
    let cap = sandbox(
        "codex",
        "dispatch",
        &mock,
        codex::manifest_grants(),
        "test-client",
    );
    let handle = codex::Codex::new(cap);
    assert_eq!(handle.name(), "codex");
    assert!(handle.worlds().contains(&lca_ext_abi::World::Provider));
    let options =
        lca_core::drive_blocking(async move { handle.login_options().await }).expect("options");
    assert!(options.is_empty(), "the identity flow, not the picker");
}
