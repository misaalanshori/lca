//! GitHub #298: `QuestionWasm::get_schema` hand-wrote a shortened copy of
//! the tool schema — `{"type":"object","properties":{"questions":
//! {"type":"array"}},"required":["questions"]}` — with the `items` object
//! missing. Native mode declared the full schema, so the two delivery modes
//! told the model two different things about the same tool.
//!
//! The consequence is a round trip that cannot succeed. A WASM-mode model
//! reads `questions` as an untyped array, so it answers
//! `{"questions": ["Which port?"]}`. That is the shape the schema invited,
//! and it is not the shape the tool accepts: `parse_question_args` requires
//! an object with `header` and `question` per entry.
//!
//! This got sharper when gh #306 landed `validate_against_schema`'s
//! value-driven structural recursion, because an array's `items` is now
//! enforced. Before that an untyped array skipped validation and the failure
//! surfaced only in the guest parser; now the host rejects the same answer
//! first. Either way the user sees the same thing — a tool that refuses the
//! arguments its own schema produced.
//!
//! The fix is one schema. `question_tool_spec()` sits at the crate root,
//! `question_schema_parameters()` is its JSON text for the WIT export, and
//! both delivery modes call them — the same shape `hash-edit` and `webfetch`
//! already use. There is no second literal left to drift.
//!
//! Verifies: NFR-24 (a shipped defect's guard), GitHub issue #298,
//! gh #298 acceptance 1–3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The native spec, and the WASM export's parameters, are byte-identical.
///
/// Exact equality rather than a spot check: "contains `items`" would pass
/// while a required field drifted or a `default` was dropped. The literal
/// exists once in the crate, so this row is really asserting that both
/// modes go through it.
#[test]
fn both_delivery_modes_declare_the_same_schema() {
    let native = question::question_tool_spec();
    assert_eq!(
        question::question_schema_parameters(),
        native.parameters.to_string(),
        "the WASM export and the native spec must be the same document"
    );
}

/// The declared schema names the structure the parser requires. Acceptance 1
/// states this as "exports the full parameter schema including `items`,
/// `header`, `question`, and `options`".
#[test]
fn the_declared_schema_carries_the_full_item_shape() {
    let parameters: serde_json::Value =
        serde_json::from_str(&question::question_schema_parameters()).expect("valid JSON");
    let items = &parameters["properties"]["questions"]["items"];
    assert_eq!(items["type"], "object", "{parameters}");
    assert_eq!(
        items["required"],
        serde_json::json!(["header", "question"]),
        "the parser requires both"
    );
    assert_eq!(items["properties"]["header"]["type"], "string");
    assert_eq!(items["properties"]["question"]["type"], "string");
    assert_eq!(items["properties"]["options"]["type"], "array");
    assert_eq!(
        items["properties"]["options"]["items"]["required"],
        serde_json::json!(["label"])
    );
    assert_eq!(items["properties"]["multiple"]["type"], "boolean");
    assert_eq!(
        items["properties"]["multiple"]["default"],
        serde_json::json!(false),
        "the `default` field survives the unification"
    );
}

/// The host's validator accepts every well-formed answer to this schema.
///
/// Runs the WASM mode's own declaration through the real
/// `lca_provider::validate_against_schema` — the validator the turn loop
/// calls before a tool runs. A schema the host would reject a good answer
/// against is a broken schema, and this is the row that keeps the
/// declaration and the validator from drifting apart.
#[test]
fn the_host_validator_accepts_a_well_formed_answer() {
    use lca_provider::validate_against_schema;
    let schema: serde_json::Value =
        serde_json::from_str(&question::question_schema_parameters()).expect("valid JSON");
    for answer in [
        r#"{"questions":[{"header":"H","question":"Q?"}]}"#,
        r#"{"questions":[{"header":"H","question":"Q?","options":[{"label":"A"}]}]}"#,
        r#"{"questions":[{"header":"H","question":"Q?","options":[{"label":"A","description":"why"}],"multiple":true}]}"#,
        r#"{"questions":[{"header":"A","question":"Q1?"},{"header":"B","question":"Q2?","multiple":false}]}"#,
    ] {
        assert!(
            validate_against_schema(&schema, answer).is_ok(),
            "a well-formed answer must pass the host's own check: {answer}"
        );
    }
}

/// The shape the truncated schema invited fails **both** gates.
///
/// This is the defect on the record rather than implied: the plain-string
/// array is rejected by gh #306's `items` enforcement *and* by the guest
/// parser. A fix that only repaired one of the two would still leave a model
/// able to produce arguments the other rejects.
#[test]
fn a_plain_string_array_is_rejected_by_the_host_and_the_parser() {
    use lca_provider::validate_against_schema;
    let schema: serde_json::Value =
        serde_json::from_str(&question::question_schema_parameters()).expect("valid JSON");
    let answer = r#"{"questions":["Which port?"]}"#;
    assert!(
        validate_against_schema(&schema, answer).is_err(),
        "gh #306's `items` enforcement catches the shape the truncated schema invited"
    );
    assert!(
        question::parse_question_args(answer).is_err(),
        "and the guest's parser rejects it too, which is what the user sees"
    );
    // The same answer with the shape the schema now declares does pass both.
    let good = r#"{"questions":[{"header":"Port","question":"Which port?"}]}"#;
    assert!(validate_against_schema(&schema, good).is_ok());
    assert!(question::parse_question_args(good).is_ok());
}

/// The schema is valid JSON by construction, and it is the *only* schema
/// literal in the crate — a second hard-coded copy is what caused this
/// defect, so its absence is worth pinning mechanically.
#[test]
fn the_crate_carries_exactly_one_schema_literal() {
    let source = include_str!("../../extensions/question/src/lib.rs");
    let occurrences = source.matches(r#""properties":{"questions""#).count();
    assert_eq!(
        occurrences, 0,
        "a second compact schema literal appeared; the schema must be built once \
         by `question_tool_spec`"
    );
    // And the old truncated form is gone entirely.
    assert!(
        !source.contains(r#"{"type":"object","properties":{"questions":{"type":"array"}}"#),
        "the truncated WASM schema is still present"
    );
    // One declaration of the spec function, in one place.
    assert_eq!(
        source.matches("pub fn question_tool_spec()").count(),
        1,
        "exactly one spec constructor"
    );
}
