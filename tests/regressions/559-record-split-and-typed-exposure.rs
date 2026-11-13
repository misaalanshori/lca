//! gh #559 - `record.rs` is a module directory, and `ToolExposure::parse` reports
//! a typed error with the same copy.
//!
//! The split's seam is the one the file already had: the 20-variant `Record`
//! schema with its serde attributes (the types) versus `impl Record`'s accessors
//! plus the convenience constructors (the codec). `record/mod.rs` keeps the
//! schema, `record/methods.rs` the codec, and **every public path is re-exported
//! from `record/mod.rs`** - so `lca_protocol::Record` and
//! `lca_protocol::usage_record` did not move, and the crate root's `pub use`
//! needed only the new error name.
//!
//! Verifies: gh #559 acceptance 1-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #559 - the schema half and the codec half are separate files, and
// the module's own files are the ones `module_text` walks (so a guard that greps
// `record.rs` would go vacuous - it now reads the module).
#[test]
fn the_record_schema_and_its_codec_are_separate_files() {
    let module = crate::module_sources("crates/lca-protocol/src/record/mod.rs");
    assert!(
        module.len() >= 2,
        "`record` is a module directory with more than one file: {:?}",
        module.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>()
    );
    let schema = &module[0].1;
    assert!(
        schema.contains("pub enum Record {"),
        "the schema half declares the `Record` enum"
    );
    let codec = module
        .iter()
        .find(|(path, _)| path.ends_with("methods.rs"))
        .expect("the codec half exists")
        .1
        .clone();
    assert!(
        codec.contains("impl Record {"),
        "the codec half owns `impl Record`'s accessors"
    );
    assert!(
        !schema.contains("impl Record {"),
        "and the schema half does not carry them"
    );
    // The re-export is what keeps every public path where it was.
    assert!(
        schema.contains("pub use methods::"),
        "`record/mod.rs` re-exports the constructors, so no caller path moved"
    );
    for name in [
        "usage_record",
        "label_record",
        "custom_record",
        "tool_call_record",
        "PREVIOUS_SUMMARY_TYPE",
    ] {
        assert!(
            schema.contains(name),
            "`{name}` is still reachable as `lca_protocol::record::{name}`"
        );
    }
    // The ceiling was the reason: both halves are now well clear of it.
    assert!(
        schema.lines().count() < 700 && codec.lines().count() < 400,
        "the split gave each half real headroom ({} / {})",
        schema.lines().count(),
        codec.lines().count()
    );
}

// Verifies: gh #559 - the refusal is typed, keeps its copy, and the type also
// implements `FromStr` so the standard trait is available.
#[test]
fn the_exposure_refusal_is_typed_with_the_same_copy() {
    use lca_protocol::{ParseToolExposureError, ToolExposure};

    let tool = read("crates/lca-protocol/src/tool.rs");
    assert!(
        tool.contains("pub struct ParseToolExposureError"),
        "the parser reports a typed error, not a `String`"
    );
    assert!(
        tool.contains("#[error(\"unknown tool exposure `{0}`\")]"),
        "the typed error renders the same sentence the `format!` produced"
    );
    assert!(
        tool.contains("impl std::str::FromStr for ToolExposure"),
        "`FromStr` is implemented, so `\"deferred\".parse()` works"
    );

    // The copy, character for character, at runtime.
    let err: ParseToolExposureError = ToolExposure::parse("bogus").expect_err("unknown");
    assert_eq!(err.to_string(), "unknown tool exposure `bogus`");
    let from_str: Result<ToolExposure, _> = "hopscotch".parse();
    assert_eq!(
        from_str.expect_err("unknown").to_string(),
        "unknown tool exposure `hopscotch`",
        "`FromStr` and `parse` are one implementation, so one message"
    );

    // Every documented value still round-trips, and the default is unchanged.
    for value in ["direct", "model-only", "codemode", "deferred", "hidden"] {
        let parsed: ToolExposure = value.parse().expect("a documented value");
        assert_eq!(parsed.as_str(), value);
    }
    assert_eq!(ToolExposure::default(), ToolExposure::Direct);
}
