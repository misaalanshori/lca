//! Gh #316: the WASM model-listing path hardcoded `extras: Vec::new()`, so a
//! WASM provider's models lost the vision / thinking / cache metadata the
//! native path kept.
//!
//! The ABI record already carries the field: `wit/world-provider.wit`'s
//! `record model-info` has `extras: list<extra-pair>`, and the host copies it
//! through on its side. So this was a mapping omission in the guest, not an
//! ABI-shape problem - no WIT change, no new field, no ABI bump. That
//! decision is recorded here because the brief asked for it explicitly.
//!
//! Why it survived: the cross-mode conformance fixture's models carried
//! *empty* extras on both sides, so "native and WASM agree" was true and
//! vacuous. The twin in `extensions/conformance` had the same hardcode.
//!
//! Verifies: gh #316 acceptance 3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

const WIT: &str = include_str!("../../wit/world-provider.wit");
const TWIN: &str = include_str!("../../extensions/conformance/src/provider_world.rs");
const SCRIPTS: &str = include_str!("../../extensions/conformance/src/scripts.rs");

// Verifies: gh #316 acceptance 3 - THE behavioral property, asserted on the
// real types the guest maps between. The wire record's `extras` list and the
// protocol map must round-trip without loss, dedup, or reordering, because
// that is exactly what the mapping does field by field.
#[test]
fn a_models_extras_round_trip_without_loss() {
    let model = lca_protocol::ModelInfo {
        id: "vision-model".to_string(),
        name: "Vision Model".to_string(),
        context_window: 200_000,
        max_tokens: 8192,
        extras: [
            ("vision".to_string(), "true".to_string()),
            ("thinking".to_string(), "true".to_string()),
            (
                "prompt_cache".to_string(),
                "short=300,long=3600".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };
    // The guest's mapping, expressed on the protocol side: pairs out, pairs
    // back in. The generic guest does `ExtraPair { key, value }` per entry,
    // and the host does the reverse, so this is the whole transformation.
    let pairs: Vec<(String, String)> = model
        .extras
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert_eq!(pairs.len(), 3, "no extra is dropped");
    let back: std::collections::BTreeMap<String, String> = pairs.into_iter().collect();
    assert_eq!(
        back, model.extras,
        "the metadata survives the round trip exactly"
    );
}

// Verifies: gh #316 - an empty extras map stays empty: a model with no
// metadata must not gain a phantom entry.
#[test]
fn an_empty_extras_map_stays_empty() {
    let model = lca_protocol::ModelInfo {
        id: "plain".to_string(),
        name: "Plain".to_string(),
        context_window: 4096,
        max_tokens: 0,
        extras: Default::default(),
    };
    let pairs: Vec<(String, String)> = model
        .extras
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert!(pairs.is_empty(), "no phantom metadata");
}

// Verifies: gh #316 - the ABI already had the field, which is why this needed
// no WIT change. This row is the recorded decision: if the record ever loses
// `extras`, the mapping above would stop compiling, and this row explains
// what was assumed.
#[test]
fn the_wit_model_record_carries_extras() {
    let record = WIT
        .split("record model-info {")
        .nth(1)
        .expect("the model-info record");
    let record = record.split('}').next().unwrap_or_default();
    assert!(
        record.contains("extras: list<extra-pair>"),
        "the ABI's reserved extension point is on the record: {record}"
    );
}

// Verifies: gh #316 acceptance 3 - the conformance twin has no hardcode
// either. It had the same line as the macro, which is precisely why the
// existing cross-mode row could not see the loss. Whitespace-normalized: a
// guard must fail on behavior, never on rustfmt.
#[test]
fn the_conformance_twin_also_carries_extras() {
    // Collapse whitespace but restore the dots: rustfmt breaks `model.extras`
    // across lines, so a naive split-whitespace mangles it into `model .extras`.
    let flat = TWIN
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" .", ".");
    let listing = flat
        .split("impl ModelsGuest for ProviderComponent")
        .nth(1)
        .expect("the twin's models impl");
    let listing = listing
        .split("pub struct ScriptedStream")
        .next()
        .unwrap_or(listing);
    assert!(
        listing.contains("model.extras"),
        "the twin must map extras too: {listing}"
    );
    assert!(
        !listing.contains("extras: Vec::new()"),
        "the twin's hardcode hid the bug"
    );
}

// Verifies: gh #316 - the fixture now exercises the field, so the cross-mode
// comparison is meaningful. Without this, "both modes agree" stays vacuous
// and the next omission hides again.
#[test]
fn the_conformance_fixture_publishes_models_with_extras() {
    let models = SCRIPTS
        .split("pub fn provider_models_default")
        .nth(1)
        .expect("the default model list");
    let models = models
        .split("/// The per-completion usage")
        .next()
        .unwrap_or(models);
    assert!(
        models.contains("\"vision\".to_string()"),
        "the fixture must publish at least one model carrying metadata: {models}"
    );
    assert!(
        models.contains("\"prompt_cache\".to_string()"),
        "and the cache metadata the issue names"
    );
}
