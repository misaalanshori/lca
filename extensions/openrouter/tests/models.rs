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

// ---------------------------------------------------------------------------
// gh #295: one malformed entry must not kill discovery
// ---------------------------------------------------------------------------

/// A catalog with `bad` spliced in among the good rows.
fn catalog_with(bad: &str) -> String {
    format!(
        r#"{{"data":[
          {{"id":"openai/gpt-4o","context_length":128000}},
          {bad},
          {{"id":"mock/test-model","context_length":32000}},
          {{"id":"anthropic/claude-3.5-sonnet","context_length":200000}}
        ]}}"#
    )
}

/// Run discovery against a forced catalog body.
fn discover(body: &str, name: &str) -> Vec<lca_protocol::ModelInfo> {
    let mock = mock_server();
    mock.fail_once("/models", 200, Some(body));
    let cap = sandbox("openrouter", name, &mock, openrouter::manifest_grants());
    lca_protocol::ProviderCap::credentials_set(&*cap, "access", "sk-or-mock").expect("seed");
    openrouter::list_models(cap.as_ref())
}

// Verifies: gh #295 acceptance 1 and 2 - a null `id` on one entry no
// longer aborts discovery. Before the fix the loop's `?` returned `None`
// for the whole payload, so the caller fell back to three curated rows.
#[test]
fn a_null_id_entry_does_not_abort_discovery() {
    let models = discover(&catalog_with(r#"{"id":null}"#), "null-id");
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "openai/gpt-4o",
            "mock/test-model",
            "anthropic/claude-3.5-sonnet",
        ],
        "every good entry survives one bad one: {ids:?}"
    );
}

// Verifies: gh #295 - a notice-shaped entry (no `id`, a `message`
// instead) is skipped and the rest are returned. Gateways send these
// for deprecations and rate-limit advisories.
#[test]
fn a_notice_entry_is_skipped_and_the_rest_return() {
    let models = discover(
        &catalog_with(r#"{"message":"this model is deprecated"}"#),
        "notice",
    );
    assert_eq!(models.len(), 3, "{models:?}");
    assert!(models.iter().all(|model| !model.id.is_empty()));
}

// Verifies: gh #295 - an empty-string `id` is skipped rather than
// admitted as a model with a blank name (the picker would show a row
// the user cannot tell apart from another).
#[test]
fn an_empty_string_id_is_skipped() {
    let models = discover(&catalog_with(r#"{"id":""}"#), "empty-id");
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "openai/gpt-4o",
            "mock/test-model",
            "anthropic/claude-3.5-sonnet",
        ],
        "a blank id is not a model: {ids:?}"
    );
}

// Verifies: gh #295 - an entry whose `id` is a number (or any
// non-string) is skipped, not coerced.
#[test]
fn a_non_string_id_is_skipped() {
    let models = discover(&catalog_with(r#"{"id":42}"#), "number-id");
    assert_eq!(models.len(), 3, "{models:?}");
}

// Verifies: gh #295's degradation row - a payload where *every* entry
// is malformed still takes the documented curated fallback. Resilience
// must not turn into an empty picker.
#[test]
fn a_wholly_malformed_catalog_still_falls_back_to_curated() {
    let body = r#"{"data":[{"id":null},{"message":"nope"},{"id":""}]}"#;
    let models = discover(body, "all-bad");
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "openai/gpt-4o",
            "anthropic/claude-3.5-sonnet",
            "google/gemini-2.0-flash",
        ],
        "an empty discovery falls back, as before: {ids:?}"
    );
}

// Verifies: gh #295 - `data` not being an array at all is still the
// fallback, unchanged (the shape half of the degradation row).
#[test]
fn a_non_array_catalog_still_falls_back_to_curated() {
    let models = discover(r#"{"data":{"id":"openai/gpt-4o"}}"#, "not-array");
    assert_eq!(models.len(), 3, "{models:?}");
}

// Verifies: gh #295 - order is preserved. A skipped entry must not
// reorder the rest, so the picker does not shuffle between refreshes.
#[test]
fn a_skipped_entry_does_not_reorder_the_rest() {
    let models = discover(
        &catalog_with(r#"{"id":911,"message":"bad row in the middle"}"#),
        "order",
    );
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "openai/gpt-4o",
            "mock/test-model",
            "anthropic/claude-3.5-sonnet",
        ],
        "gateway order is preserved: {ids:?}"
    );
}

// Verifies: gh #295 design rule 2 - the skip is observable, not
// silent. The count of dropped entries comes back from the seam the
// extension reports through, so "returned 2 of 300 and said nothing"
// cannot happen.
#[test]
fn the_skip_count_is_reported_not_swallowed() {
    let mock = mock_server();
    mock.fail_once("/models", 200, Some(&catalog_with(r#"{"id":null}"#)));
    let cap = sandbox("openrouter", "count", &mock, openrouter::manifest_grants());
    lca_protocol::ProviderCap::credentials_set(&*cap, "access", "sk-or-mock").expect("seed");
    let (models, skipped) = openrouter::discover_report(cap.as_ref());
    assert_eq!(models.len(), 3, "the good rows still return");
    assert_eq!(skipped, 1, "and the dropped row is counted, not hidden");
}

// Verifies: gh #295 design rule 2 - the diagnostic sentence carries the
// counts and nothing secret. Both delivery modes format this one string,
// so its wording is the contract a user reads.
#[test]
fn the_skip_message_names_both_counts() {
    assert_eq!(
        openrouter::skip_message(1, 40),
        "openrouter: skipped 1 malformed catalog entry; 40 models usable"
    );
    assert_eq!(
        openrouter::skip_message(7, 1),
        "openrouter: skipped 7 malformed catalog entries; 1 model usable"
    );
}
