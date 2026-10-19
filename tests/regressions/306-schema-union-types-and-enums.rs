//! Regression rows for gh #306: `validate_against_schema` must honour union
//! `type` arrays and `enum` constraints.
//!
//! The defect: `validate_value` opened with
//!
//! ```text
//! let Some(expected) = schema.get("type").and_then(Value::as_str) else {
//!     return Ok(());
//! };
//! ```
//!
//! so a schema whose `type` was an *array* (`["string", "null"]`, the standard
//! JSON-Schema spelling for nullable) fell through the `let-else` and the whole
//! subtree went unvalidated. A `type`-less schema did the same, which also
//! meant `{"enum": [...]}` without a `type` was never checked at all - and
//! `enum` was never inspected even when `type` was present.
//!
//! This is not academic in LCA: `extensions/project-memory` declares
//! `"scope": {"enum": ["project", "global"]}`, so a model could pass
//! `"scope": "whatever"` and the host would forward it to `execute`.
//!
//! Verifies: gh #306 - union types and enums are enforced, and structural
//! recursion still happens for values the union permits.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_provider::validate_against_schema;
use serde_json::json;

/// A nullable string: `"type": ["string", "null"]`. Both arms are accepted
/// and everything else is refused - previously *nothing* was refused.
#[test]
fn a_union_type_accepts_each_of_its_members() {
    let schema = json!({"type": ["string", "null"]});
    assert!(validate_against_schema(&schema, r#""text""#).is_ok());
    assert!(validate_against_schema(&schema, "null").is_ok());
    let err = validate_against_schema(&schema, "42")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("string | null"),
        "the error names both alternatives: {err}"
    );
}

/// A three-member union, including a non-scalar member.
#[test]
fn a_three_member_union_accepts_all_of_them() {
    let schema = json!({"type": ["string", "integer", "boolean"]});
    for ok in [r#""s""#, "7", "true"] {
        assert!(
            validate_against_schema(&schema, ok).is_ok(),
            "{ok} is a member"
        );
    }
    assert!(
        validate_against_schema(&schema, "1.5").is_err(),
        "a float is not an integer"
    );
    assert!(
        validate_against_schema(&schema, "null").is_err(),
        "null is not a member"
    );
}

/// `enum` is enforced even with no `type` at all - the shape
/// `project-memory` uses.
#[test]
fn an_enum_without_a_type_is_enforced() {
    let schema = json!({"enum": ["project", "global"]});
    assert!(validate_against_schema(&schema, r#""project""#).is_ok());
    assert!(validate_against_schema(&schema, r#""global""#).is_ok());
    let err = validate_against_schema(&schema, r#""whatever""#)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("must be one of"),
        "the error names the allowed set: {err}"
    );
    assert!(err.contains("project") && err.contains("global"), "{err}");
}

/// `enum` combined with a `type`: the enum is the tighter constraint and is
/// checked as well as the type.
#[test]
fn an_enum_alongside_a_type_is_enforced() {
    let schema = json!({"type": "string", "enum": ["read", "write"]});
    assert!(validate_against_schema(&schema, r#""read""#).is_ok());
    // A non-string fails the type check, not the enum check.
    assert!(validate_against_schema(&schema, "1").is_err());
    // A string outside the set fails.
    let err = validate_against_schema(&schema, r#""exec""#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("must be one of"), "{err}");
}

/// A non-string enum member works: JSON Schema allows any JSON value.
#[test]
fn an_enum_of_numbers_and_nulls_is_enforced() {
    let schema = json!({"enum": [1, 2, null]});
    assert!(validate_against_schema(&schema, "1").is_ok());
    assert!(validate_against_schema(&schema, "2").is_ok());
    assert!(validate_against_schema(&schema, "null").is_ok());
    assert!(validate_against_schema(&schema, "3").is_err());
    assert!(validate_against_schema(&schema, r#""1""#).is_err());
}

/// An object reached through a union still has its `required` and
/// `properties` checked: the old code skipped the subtree entirely, so a
/// nullable object validated nothing.
#[test]
fn an_object_inside_a_union_still_validates_its_children() {
    let schema = json!({
        "type": ["object", "null"],
        "properties": {"path": {"type": "string"}},
        "required": ["path"]
    });
    assert!(validate_against_schema(&schema, "null").is_ok());
    assert!(validate_against_schema(&schema, r#"{"path":"a"}"#).is_ok());
    let missing = validate_against_schema(&schema, "{}")
        .unwrap_err()
        .to_string();
    assert!(missing.contains("$.path is required"), "{missing}");
    let wrong = validate_against_schema(&schema, r#"{"path":1}"#)
        .unwrap_err()
        .to_string();
    assert!(wrong.contains("$.path must be string"), "{wrong}");
}

/// An array inside a union still validates its `items`.
#[test]
fn an_array_inside_a_union_still_validates_its_items() {
    let schema = json!({"type": ["array", "null"], "items": {"type": "integer"}});
    assert!(validate_against_schema(&schema, "null").is_ok());
    assert!(validate_against_schema(&schema, "[1,2]").is_ok());
    let err = validate_against_schema(&schema, r#"[1,"two"]"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("$[1] must be integer"), "{err}");
}

/// A `type`-less schema that carries `properties` still validates them. The
/// old early return meant a schema written as `{"properties": {...}}` was
/// inert.
#[test]
fn a_typeless_schema_with_properties_still_validates_them() {
    let schema = json!({
        "properties": {"path": {"type": "string"}},
        "required": ["path"]
    });
    assert!(validate_against_schema(&schema, r#"{"path":"a"}"#).is_ok());
    let missing = validate_against_schema(&schema, "{}")
        .unwrap_err()
        .to_string();
    assert!(missing.contains("$.path is required"), "{missing}");
    let wrong = validate_against_schema(&schema, r#"{"path":1}"#)
        .unwrap_err()
        .to_string();
    assert!(wrong.contains("$.path must be string"), "{wrong}");
}

/// A single-string `type` keeps behaving exactly as before - this is the row
/// that would catch a rewrite that broke the common case.
#[test]
fn a_plain_string_type_is_unchanged() {
    let schema = json!({
        "type": "object",
        "properties": {"path": {"type": "string"}, "offset": {"type": "integer"}},
        "required": ["path"]
    });
    assert!(validate_against_schema(&schema, r#"{"path":"a"}"#).is_ok());
    assert!(validate_against_schema(&schema, r#"{"path":1}"#).is_err());
    assert!(validate_against_schema(&schema, "{}").is_err());
    assert!(validate_against_schema(&schema, r#"{"path":"a","offset":1.5}"#).is_err());
}

/// An unknown type keyword stays permissive, in a union as well as alone: a
/// keyword the project does not model must not turn into a rejection.
#[test]
fn an_unknown_keyword_in_a_union_stays_permissive() {
    let schema = json!({"type": ["mystery", "null"]});
    assert!(validate_against_schema(&schema, r#"{"anything":1}"#).is_ok());
    assert!(validate_against_schema(&schema, "null").is_ok());
}

/// The live shape from `extensions/project-memory`: an enum inside a
/// property of a required object.
#[test]
fn the_project_memory_scope_shape_is_enforced() {
    let schema = json!({
        "type": "object",
        "properties": {
            "key": {"type": "string"},
            "scope": {"enum": ["project", "global"]}
        },
        "required": ["key"]
    });
    assert!(validate_against_schema(&schema, r#"{"key":"k","scope":"project"}"#).is_ok());
    let err = validate_against_schema(&schema, r#"{"key":"k","scope":"elsewhere"}"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("$.scope must be one of"), "{err}");
}
