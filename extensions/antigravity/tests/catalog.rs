//! Catalog virtualization (gh #241B, pi-antigravity
//! `src/models/grouping.ts`): Google's internal endpoint variants
//! collapse into canonical parents, and thinking effort maps to the
//! runtime id at request time (which `resolve_runtime_model`
//! already does). The picker lists parents, never suffixed
//! internals.
//!
//! Verifies: suffixed variants collapse, tiered folds in, aliases
//! merge, internal/chat/image ids never list, levels advertise per
//! model, and rank orders newest flash first.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use antigravity::virtualize_catalog;

fn catalog() -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({
        "gemini-3.8-flash-low": {"displayName": "Gemini 3.8 Flash (Low)", "inputTokenLimit": 1048576, "outputTokenLimit": 65536},
        "gemini-3.8-flash-medium": {"displayName": "Gemini 3.8 Flash (Medium)", "inputTokenLimit": 1048576, "outputTokenLimit": 65536},
        "gemini-3.8-flash-high": {"displayName": "Gemini 3.8 Flash (High)", "inputTokenLimit": 1048576, "outputTokenLimit": 65536},
        "gemini-3.7-flash-low": {"displayName": "Gemini 3.7 Flash (Low)", "inputTokenLimit": 1048576, "outputTokenLimit": 32768},
        "gemini-3.7-flash-tiered": {"displayName": "Gemini 3.7 Flash", "inputTokenLimit": 1048576, "outputTokenLimit": 32768},
        "gemini-3.5-flash-extra-low": {"displayName": "Gemini 3.5 Flash (Extra Low)", "inputTokenLimit": 1048576, "outputTokenLimit": 65536},
        "gemini-3-flash-agent": {"displayName": "Gemini 3 Flash Agent", "inputTokenLimit": 1048576, "outputTokenLimit": 65536},
        "claude-sonnet-4-6": {"displayName": "Claude Sonnet 4.6", "inputTokenLimit": 200000, "outputTokenLimit": 64000},
        "gpt-oss-120b": {"displayName": "GPT OSS 120B", "inputTokenLimit": 131072, "outputTokenLimit": 32768},
        "gemini-2.0-secret": {"displayName": "Hidden", "isInternal": true},
        "chat_tab_helper": {"displayName": "Chat"},
        "MODEL_X": {"displayName": "X"},
        "gemini-image-pro": {"displayName": "Image"}
    })
    .as_object()
    .expect("object")
    .clone()
}

fn ids(models: &[lca_protocol::ModelInfo]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

// Verifies: gh #241B - the picker-flooding suffixes collapse into
// their canonical parents; no suffixed id lists.
#[test]
fn suffixed_variants_collapse_into_canonical_parents() {
    let models = virtualize_catalog(&catalog());
    let listed = ids(&models);
    for id in &listed {
        assert!(
            !id.ends_with("-low")
                && !id.ends_with("-medium")
                && !id.ends_with("-high")
                && !id.ends_with("-tiered")
                && !id.ends_with("-extra-low")
                && !id.ends_with("-agent"),
            "suffixed id lists: {id}"
        );
    }
    for parent in ["gemini-3.8-flash", "gemini-3.7-flash", "gemini-3.5-flash"] {
        assert!(listed.contains(&parent), "{parent} lists: {listed:?}");
    }
}

// Verifies: gh #241B - internal, chat/tab, MODEL_, and image ids never
// reach the picker (ground truth `isSelectableRuntimeModelId`).
#[test]
fn unselectable_ids_never_list() {
    let models = virtualize_catalog(&catalog());
    let listed = ids(&models);
    for banned in [
        "gemini-2.0-secret",
        "chat_tab_helper",
        "MODEL_X",
        "gemini-image-pro",
    ] {
        assert!(!listed.contains(&banned), "{banned} lists: {listed:?}");
    }
}

// Verifies: gh #241B - each parent advertises its discovered levels
// for the `/thinking` picker (ground truth `advertisedThinkingLevels`).
#[test]
fn parents_advertise_their_discovered_levels() {
    let models = virtualize_catalog(&catalog());
    let levels = |id: &str| {
        models
            .iter()
            .find(|model| model.id == id)
            .and_then(|model| model.extras.get("thinking_levels"))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(levels("gemini-3.8-flash"), "low,medium,high");
    // Tiered folds into 3.7's family without adding a level of its own.
    assert_eq!(levels("gemini-3.7-flash"), "low");
    // The agent alias rides as 3.5's high.
    assert_eq!(levels("gemini-3.5-flash"), "low,high");
}

// Verifies: gh #241B - rank orders newest flash first, then opus,
// sonnet, pro, gpt-oss (ground truth `comparePublicModels`).
#[test]
fn rank_orders_newest_flash_first() {
    let models = virtualize_catalog(&catalog());
    let listed = ids(&models);
    let flash38 = listed
        .iter()
        .position(|id| *id == "gemini-3.8-flash")
        .expect("3.8");
    let flash37 = listed
        .iter()
        .position(|id| *id == "gemini-3.7-flash")
        .expect("3.7");
    let sonnet = listed
        .iter()
        .position(|id| *id == "claude-sonnet-4-6")
        .expect("sonnet");
    let oss = listed
        .iter()
        .position(|id| *id == "gpt-oss-120b")
        .expect("oss");
    assert!(flash38 < flash37, "{listed:?}");
    assert!(flash37 < sonnet, "{listed:?}");
    assert!(sonnet < oss, "{listed:?}");
}

// Verifies: gh #241B - empty discovery virtualizes to nothing
// (list_models falls back to the static table on empty, unchanged),
// and windows take the group's max.
#[test]
fn empty_discovery_virtualizes_to_nothing() {
    assert!(virtualize_catalog(&serde_json::Map::new()).is_empty());
    let models = virtualize_catalog(&catalog());
    let flash = models
        .iter()
        .find(|model| model.id == "gemini-3.8-flash")
        .expect("3.8");
    assert_eq!(flash.context_window, 1048576);
    assert_eq!(flash.max_tokens, 65536);
    let mut wide = serde_json::Map::new();
    wide.insert(
        "gemini-3.8-flash-low".to_string(),
        serde_json::json!({"inputTokenLimit": 100, "outputTokenLimit": 10}),
    );
    wide.insert(
        "gemini-3.8-flash-high".to_string(),
        serde_json::json!({"inputTokenLimit": 200, "outputTokenLimit": 20}),
    );
    let models = virtualize_catalog(&wide);
    let flash = models
        .iter()
        .find(|model| model.id == "gemini-3.8-flash")
        .expect("3.8");
    assert_eq!((flash.context_window, flash.max_tokens), (200, 20));
}

// Verifies: gh #241B - an explicit display parenthetical beats suffix
// parsing (ground-truth precedence).
#[test]
fn display_levels_beat_suffix_parsing() {
    let mut raw = serde_json::Map::new();
    raw.insert(
        "gemini-3.8-flash-low".to_string(),
        serde_json::json!({"displayName": "Gemini 3.8 Flash (High)"}),
    );
    let models = virtualize_catalog(&raw);
    let levels = models
        .iter()
        .find(|model| model.id == "gemini-3.8-flash")
        .and_then(|model| model.extras.get("thinking_levels"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(levels, "high");
}
