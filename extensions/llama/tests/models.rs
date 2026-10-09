//! The llama catalog + command surface (gh #62): router parsing,
//! the curated-nothing fallback, the three commands, and the
//! manifest/grants agreement.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::{mock_server, sandbox};

fn effect_text(effect: lca_protocol::CommandEffect) -> String {
    match effect {
        lca_protocol::CommandEffect::ShowWidget(text) => text,
        other => panic!("commands answer text, got {other:?}"),
    }
}

// Verifies: gh #62 - the catalog parses router rows (statuses,
// windows from runtime/config/trained precedence) and drops
// decision-only models.
#[test]
fn the_catalog_parses_router_rows() {
    let mock = mock_server();
    let cap = sandbox("llama", "catalog", &mock, llama::manifest_grants());
    let models = llama::list_models(cap.as_ref());
    let row = |id: &str| models.iter().find(|model| model.id == id);
    let loaded = row("mock/loaded").expect("loaded lists");
    assert!(
        loaded.name.contains("loaded"),
        "status rides: {}",
        loaded.name
    );
    assert_eq!(loaded.context_window, 8192, "runtime n_ctx wins");
    let unloaded = row("mock/unloaded").expect("unloaded lists");
    assert_eq!(
        unloaded.context_window, 16384,
        "the launch flag covers unloaded rows"
    );
    let sleeping = row("mock/sleeping").expect("sleeping lists");
    assert_eq!(
        sleeping.context_window, 32768,
        "sleeping rows keep the trained window"
    );
    assert!(row("mock/hybrid").is_some(), "text-and-decisions rows stay");
    assert!(row("mock/judge").is_none(), "decision-only rows never list");
}

// Verifies: gh #62 - a down server lists nothing (there is no curated
// floor for your own GGUFs); the command names the way instead.
#[test]
fn a_down_server_lists_nothing_and_says_so() {
    let mock = mock_server();
    // Twice: the list probe and the command each call the server.
    mock.fail_once("/models", 500, Some("{}"));
    mock.fail_once("/models", 500, Some("{}"));
    let cap = sandbox("llama", "down", &mock, llama::manifest_grants());
    assert!(llama::list_models(cap.as_ref()).is_empty());
    let text = effect_text(llama::invoke_command(cap.as_ref(), ""));
    assert!(
        text.contains("llama-server"),
        "the way back, not a blank: {text}"
    );
}

// Verifies: gh #62 - load and unload POST the router endpoints with
// the model id; usage text answers anything else.
#[test]
fn load_and_unload_post_the_router_endpoints() {
    let mock = mock_server();
    let cap = sandbox("llama", "manage", &mock, llama::manifest_grants());
    let text = effect_text(llama::invoke_command(cap.as_ref(), "load mock/unloaded"));
    assert!(text.contains("mock/unloaded"), "names the model: {text}");
    let text = effect_text(llama::invoke_command(cap.as_ref(), "unload mock/loaded"));
    assert!(text.contains("mock/loaded"), "names the model: {text}");
    let loads = mock.requests_of("/models/load");
    assert_eq!(loads.len(), 1, "one load: {loads:?}");
    assert!(
        loads[0].contains("mock/unloaded"),
        "the id crosses: {}",
        loads[0]
    );
    assert!(loads[0].starts_with("POST"), "a POST: {}", loads[0]);
    let unloads = mock.requests_of("/models/unload");
    assert_eq!(unloads.len(), 1, "one unload: {unloads:?}");
    let text = effect_text(llama::invoke_command(cap.as_ref(), "frobnicate"));
    assert!(text.contains("usage:"), "usage answers the unknown: {text}");
}

// Verifies: gh #62 - the manifest declares both worlds, loopback
// only, and the native handle serves both.
#[test]
fn manifest_toml_and_native_grants_agree() {
    let manifest: toml::Value = llama::MANIFEST.parse().expect("MANIFEST parses");
    let worlds: Vec<&str> = manifest["worlds"]
        .as_array()
        .expect("worlds")
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert_eq!(worlds, vec!["provider", "command"]);
    let addresses = manifest["capabilities"]["net-local"]["addresses"]
        .as_array()
        .expect("addresses");
    let names: Vec<&str> = addresses.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names, vec!["localhost", "127.0.0.1"]);
    assert!(
        manifest["capabilities"].get("net").is_none(),
        "no hosted net on a loopback provider"
    );
    assert_eq!(
        manifest["capabilities"]["credentials"]["namespace"].as_str(),
        Some("llama")
    );
    let grants = llama::manifest_grants();
    assert_eq!(grants.net_local.len(), 2, "both loopback patterns");
    assert!(grants.net.is_empty(), "no hosted patterns either");
    assert!(grants.credentials, "the namespace is granted");
    let spec = llama::command_spec();
    assert_eq!(spec.name, "llama");
}
