//! Tool-argument schema validation (extension authoring guide: the host
//! validates a call's arguments against the schema the model saw before it
//! calls `execute`).

use lca_provider::validate_against_schema;
use serde_json::json;

/// The subset the project's tools declare: object/array/scalar types,
/// `required`, and nested `items`.
#[test]
fn required_and_typed_fields_are_enforced() {
    let schema = json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "offset": {"type": "integer"},
            "edits": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "oldText": {"type": "string"},
                        "newText": {"type": "string"}
                    },
                    "required": ["oldText", "newText"]
                }
            }
        },
        "required": ["path"]
    });

    assert!(validate_against_schema(&schema, r#"{"path":"a"}"#).is_ok());
    assert!(
        validate_against_schema(&schema, r#"{"path":1}"#)
            .unwrap_err()
            .contains("$.path must be string")
    );
    assert!(
        validate_against_schema(&schema, "{}")
            .unwrap_err()
            .contains("$.path is required")
    );
    assert!(validate_against_schema(&schema, r#"{"path":"a","offset":1.5}"#).is_err());
    assert!(
        validate_against_schema(&schema, r#"{"path":"a","edits":[{"oldText":"x"}]}"#)
            .unwrap_err()
            .contains("$.edits[0].newText is required")
    );
    assert!(validate_against_schema(&schema, "not json").is_err());
}

/// An unknown type keyword is not a rejection: the validator covers the
/// vocabulary the project ships and leaves the rest alone.
#[test]
fn unknown_keywords_do_not_reject() {
    let schema = json!({"type": "object", "properties": {"x": {"type": "mystery"}}});
    assert!(validate_against_schema(&schema, r#"{"x":1}"#).is_ok());
    // A schema with no `type` at all constrains nothing.
    assert!(validate_against_schema(&json!({}), r#"{"anything":true}"#).is_ok());
}
