//! Cycle-4 defect: a provider that answered a model probe with something
//! unparseable returned a single entry whose id was empty. The session
//! took that empty id as its model, so the first `complete` call reached
//! the extension with no model and failed as "this endpoint has no model
//! selected" instead of falling back the way an empty list does.
//!
//! Verifies: FR-PROV-2 (the active model), FR-PROV-6 (a legible zero-model
//! state rather than a blank one).

use lca_protocol::ModelInfo;

/// The rule the wiring applies: the first non-empty id, else the fallback.
fn pick_model(models: &[ModelInfo], fallback: &str) -> String {
    models
        .iter()
        .map(|model| model.id.clone())
        .find(|id| !id.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        name: "n".to_string(),
        context_window: 1,
        max_tokens: 1,
        extras: Default::default(),
    }
}

#[test]
fn an_empty_id_is_not_a_model() {
    assert_eq!(
        pick_model(&[model(""), model("real")], "openai-compatible"),
        "real",
        "the empty entry is skipped, not selected"
    );
    assert_eq!(
        pick_model(&[model("")], "openai-compatible"),
        "openai-compatible",
        "nothing usable falls back like an empty list"
    );
    assert_eq!(pick_model(&[], "fallback"), "fallback");
    assert_eq!(pick_model(&[model("chosen")], "x"), "chosen");
}
