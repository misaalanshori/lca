//! The Copilot catalog + dispatch surface (gh #184): live policy
//! parsing, the curated floor, and the manifest/grants agreement.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;

mod common;

use common::{mock_server, sandbox};

/// The two shapes the window read must distinguish (gh #283): both limits
/// present, and only the prompt cap present.
const PROMPT_CAP_ONLY_FIXTURE: &str = r#"{"data":[
{"id":"mock/both-limits","name":"Both","model_picker_enabled":true,
 "capabilities":{"family":"x","limits":{"max_context_window_tokens":150000,"max_prompt_tokens":140000,"max_output_tokens":10000},"supports":{"tool_calls":true}}},
{"id":"mock/prompt-cap-only","name":"Prompt Only","model_picker_enabled":true,
 "capabilities":{"family":"y","limits":{"max_prompt_tokens":120000,"max_output_tokens":8000},"supports":{"tool_calls":true}}}
]}"#;

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

// ---- gh #283: a discovered row carries a real context window ----

fn model<'a>(models: &'a [lca_protocol::ModelInfo], id: &str) -> &'a lca_protocol::ModelInfo {
    models
        .iter()
        .find(|model| model.id == id)
        .unwrap_or_else(|| panic!("`{id}` is in the catalog: {:?}", ids_of(models)))
}

fn ids_of(models: &[lca_protocol::ModelInfo]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

// Verifies: gh #283 - the defect itself. Live discovery used to stamp
// `context_window: 0` on every row, discarding the curated table's real
// windows. A zero window leaves the footer at `ctx ?` and hands FR-SESS-4's
// threshold the 128k fallback estimate, so Copilot's 200k models compacted
// as if they had 128k. The witness is a catalog with curated ids *and* an
// uncatalogued one: the curated rows must carry their windows, and only the
// uncatalogued-unknown row may be 0.
#[test]
fn a_discovered_model_keeps_its_context_window() {
    let mock = mock_server();
    let cap = live_cap(&mock, "windows");
    let models = github_copilot::list_models(cap.as_ref());
    assert_eq!(
        model(&models, "claude-3.5-sonnet").context_window,
        200_000,
        "a curated id discovered live keeps its documented window: {:?}",
        model(&models, "claude-3.5-sonnet")
    );
    assert_eq!(
        model(&models, "mock/unlimited-model").context_window,
        0,
        "an id nobody can size stays 0 (unknown), never fabricated"
    );
    assert!(
        models.iter().any(|m| m.context_window > 0),
        "the bug was every window reading 0: {:?}",
        ids_of(&models)
    );
}

// Verifies: gh #283 - the curated table wins over the endpoint's number
// for the same id. Rule: curated is a vendor-documented figure, and the
// endpoint's is the account's own view of it; openai-compatible's
// precedence puts the curated catalog above the endpoint for exactly
// this reason (a wrong denominator silently mis-triggers compaction,
// which is worse than a missing one). The fixture's `gpt-4o` is the
// witness: curated 128000, endpoint 64000.
#[test]
fn the_curated_table_wins_over_the_endpoint_for_the_same_id() {
    let mock = mock_server();
    let cap = live_cap(&mock, "precedence");
    let models = github_copilot::list_models(cap.as_ref());
    let curated = github_copilot::CURATED_MODELS
        .iter()
        .find(|(id, _, _)| *id == "gpt-4o")
        .map(|(_, _, window)| *window)
        .expect("gpt-4o is curated");
    assert_eq!(curated, 128_000, "the curated figure this row pins");
    assert_eq!(
        model(&models, "gpt-4o").context_window,
        curated,
        "the curated window beats the endpoint's smaller one: {:?}",
        model(&models, "gpt-4o")
    );
}

// Verifies: gh #283 - an id the curated table does not know still gets a
// number when the endpoint declares one. Dropping it would leave a real
// window on the floor for every new model the account is offered.
#[test]
fn an_uncatalogued_id_uses_the_endpoints_declared_window() {
    let mock = mock_server();
    let cap = live_cap(&mock, "endpoint-window");
    let models = github_copilot::list_models(cap.as_ref());
    assert!(
        !github_copilot::CURATED_MODELS
            .iter()
            .any(|(id, _, _)| *id == "mock/new-model"),
        "the witness must be uncatalogued for this row to mean anything"
    );
    assert_eq!(
        model(&models, "mock/new-model").context_window,
        150_000,
        "capabilities.limits.max_context_window_tokens rides through: {:?}",
        model(&models, "mock/new-model")
    );
}

// Verifies: gh #283 - the window read is `max_context_window_tokens`,
// never its sibling `max_prompt_tokens` (a different denominator: the
// input cap, not the window). Where both are present only the window may
// be used; where only the prompt cap is present it is the documented
// fallback (opencode's decoder: `max_context_window_tokens ??
// max_prompt_tokens`) rather than reporting 0 with a figure available.
#[test]
fn the_window_is_the_context_limit_and_falls_back_to_the_prompt_cap() {
    let mock = mock_server();
    mock.fail_once("/models", 200, Some(PROMPT_CAP_ONLY_FIXTURE));
    let cap = live_cap(&mock, "prompt-vs-context");
    let models = github_copilot::list_models(cap.as_ref());
    let both = model(&models, "mock/both-limits");
    assert_eq!(
        both.context_window, 150_000,
        "the context window, not `max_prompt_tokens` (140000): {:?}",
        both
    );
    let prompt_only = model(&models, "mock/prompt-cap-only");
    assert_eq!(
        prompt_only.context_window, 120_000,
        "absent context window falls back to the prompt cap: {:?}",
        prompt_only
    );
}

// Verifies: gh #283 - `extras` carries the endpoint's capability hints
// under the vocabulary the rest of the tree already reads, so the picker
// and the tool executor benefit without a new key being invented:
// `image.vision` (gh #39) and `thinking_levels` (gh #242). A row the
// endpoint did not describe carries *nothing* - absent means unknown -
// rather than a synthesized `false`.
#[test]
fn the_endpoint_capability_hints_ride_the_documented_extras() {
    let mock = mock_server();
    let cap = live_cap(&mock, "extras");
    let models = github_copilot::list_models(cap.as_ref());
    let claude = model(&models, "claude-3.5-sonnet");
    assert_eq!(
        claude
            .extras
            .get(lca_protocol::IMAGE_VISION_EXTRA)
            .map(String::as_str),
        Some("true"),
        "`supports.vision` maps onto the documented vision key: {:?}",
        claude.extras
    );
    let new = model(&models, "mock/new-model");
    assert_eq!(
        new.extras
            .get(lca_protocol::THINKING_LEVELS_EXTRA)
            .map(String::as_str),
        Some("low,medium,high"),
        "`supports.reasoning_effort` maps onto the documented levels key          (the picker's own vocabulary, canonical order): {:?}",
        new.extras
    );
    let unknown = model(&models, "mock/unlimited-model");
    assert!(
        unknown.extras.is_empty(),
        "a row the endpoint did not describe carries nothing, not a guess: {:?}",
        unknown.extras
    );
}

// Verifies: gh #283 - the endpoint's `max_output_tokens` reaches
// `ModelInfo::max_tokens` instead of being dropped. Nothing in the tree
// builds a request body from it (the generation cap rides
// `CompletionRequest.extras["max-tokens"]`, set by the core's compaction
// budget), so this is carried data, not new behaviour - and dropping a
// number the endpoint published is the same class of defect as the issue.
#[test]
fn the_endpoints_output_cap_rides_through() {
    let mock = mock_server();
    let cap = live_cap(&mock, "output-cap");
    let models = github_copilot::list_models(cap.as_ref());
    let new = model(&models, "mock/new-model");
    assert_eq!(new.max_tokens, 10_000, "max_output_tokens rides: {new:?}");
    let bare = model(&models, "mock/unlimited-model");
    assert_eq!(bare.max_tokens, 0, "unreported stays 0, not a guess");
}

// Verifies: gh #283 - the display name the endpoint supplies is used
// instead of echoing the id, and a row with no name still falls back to
// the id (never blank).
#[test]
fn the_endpoints_display_name_reaches_the_picker() {
    let mock = mock_server();
    let cap = live_cap(&mock, "names");
    let models = github_copilot::list_models(cap.as_ref());
    assert_eq!(model(&models, "mock/new-model").name, "Brand New Model");
    assert_eq!(
        model(&models, "mock/unlimited-model").name,
        "mock/unlimited-model",
        "no name supplied: the id, never a blank row"
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
    // Gh #283: the floor is the one path that always had real windows, and
    // it must keep them - every row 0 here would mean an offline user's
    // Compaction never fires, the same failure in a different branch.
    assert!(
        models.iter().all(|model| model.context_window > 0),
        "the curated floor is fully sized: {:?}",
        models
            .iter()
            .map(|m| (m.id.as_str(), m.context_window))
            .collect::<Vec<_>>()
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
