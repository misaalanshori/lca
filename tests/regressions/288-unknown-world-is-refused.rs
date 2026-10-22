//! gh #288 - a typo'd world is refused, and the vocabulary has one home.
//!
//! `Manifest::parse` used to collect `worlds` as raw strings and never check
//! them, so `worlds = ["contxt-transform"]` installed and loaded as an extension
//! matching no world: a successful install, no error anywhere, and nothing
//! working. The failure mode is the expensive kind - silence.
//!
//! The behaviour rows live beside the code (`crates/lca-ext-host/tests/abi.rs`:
//! the typo with its suggestion, every vocabulary world, the hooks family,
//! duplicates, non-strings, the schema parity row, and every shipped manifest).
//! This guard is the structural half NFR-24 asks for: the *mechanism* that keeps
//! those rows true cannot be quietly removed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #288 - the vocabulary is one table, and both the validator and the
// dispatcher read it.
//
// The bug this issue filed was not "a list was wrong" but "there were several
// lists". The manifest parser had no list at all, the dispatcher had a `match`,
// the loader had a `==` chain, and the JSON schema had an `enum` - four copies of
// one fact, only three of them agreeing. Adding validation against a fifth
// hand-copied list would have fixed the symptom and left the cause, so this row
// checks that a *single* table is what everything consults.
#[test]
fn the_world_vocabulary_has_one_home() {
    // The table exists in the ABI crate, next to `World`, and covers every
    // variant. `every_vocabulary_world_parses` in `tests/abi.rs` checks the
    // coverage by round-tripping through `parse_world`/`world_name`; here we
    // check the table is *there* and is what the other crates import.
    let abi = read("crates/lca-ext-abi/src/lib.rs");
    assert!(
        abi.contains("pub const WORLDS: &[(&str, World)]"),
        "the string<->World mapping is one table in `lca-ext-abi`"
    );
    assert!(
        abi.contains("pub fn parse_world(") && abi.contains("pub fn world_name("),
        "the table is read through accessors, so no caller needs its own copy"
    );

    // The dispatcher must NOT carry its own match over world names again: that
    // was one of the copies, and a new world added there but not to the table
    // (or the reverse) is how this bug returns in a new dress.
    let dispatch = read("crates/lca-ext-host/src/dispatch_hooks.rs");
    assert!(
        dispatch.contains("lca_ext_abi::parse_world(world)"),
        "`WasmExtension::worlds` resolves through the table"
    );
    for literal in [
        r#""hooks-tool-call" =>"#,
        r#""context-transform" =>"#,
        r#""tool-catalog" =>"#,
    ] {
        assert!(
            !dispatch.contains(literal),
            "the dispatcher still matches on a world name literal {literal}, \
             which is a second copy of the vocabulary"
        );
    }

    // The manifest parser validates through the same table.
    let manifest = read("crates/lca-ext-host/src/manifest.rs");
    assert!(
        manifest.contains("lca_ext_abi::parse_world(name)"),
        "`Manifest::parse` validates each world against the table"
    );
    assert!(
        manifest.contains("lca_ext_abi::nearest_world(name)"),
        "and suggests the nearest valid name, which is what makes a refusal \
         actionable rather than something the author has to grep for"
    );
    // The old silent `filter_map` over `worlds` is gone: a non-string entry must
    // be an error, not a shorter list. The non-empty check itself is *kept* -
    // `worlds = []` plus resources is a legal data-only extension (ADR-0030) -
    // but it must run after validation, so an empty list is the only way to reach
    // it.
    let worlds_binding = manifest
        .split("let worlds = ")
        .nth(1)
        .expect("`parse` binds `worlds`")
        .split(';')
        .next()
        .expect("the binding ends");
    assert!(
        !worlds_binding.contains("filter_map"),
        "`worlds` is no longer built by a silent `filter_map`: {worlds_binding}"
    );
    let validated = manifest
        .find("let worlds = parse_worlds(&value)?;")
        .expect("`parse` validates before anything else touches `worlds`");
    let non_empty = manifest
        .find("`worlds` must list at least one world")
        .expect("the data-only check is still there");
    assert!(
        validated < non_empty,
        "validation runs before the emptiness check, so the only way to reach \
         the data-only refusal is a genuinely empty list"
    );
}

// Verifies: gh #288 - the schema's `worlds` enum is compared to the code's table
// by a test, so the two cannot drift.
//
// The schema is what an author's editor autocompletes from; the code is what the
// loader dispatches on. A world in one and not the other is the exact shape of
// this bug, one indirection out.
#[test]
fn the_schema_enum_is_checked_against_the_code() {
    let abi = read("crates/lca-ext-abi/src/lib.rs");
    assert!(
        abi.contains("pub const WORLDS"),
        "the table the schema is compared against"
    );
    let schema = read("schemas/extension-manifest.schema.json");
    assert!(
        schema.contains("\"enum\""),
        "the schema still enumerates worlds, so the parity row has something to \
         compare against"
    );
    // And the row that does the comparing exists and names both sides.
    let tests = read("crates/lca-ext-host/tests/abi.rs");
    assert!(
        tests.contains("fn the_schema_enum_matches_the_vocabulary"),
        "the parity row is present"
    );
    assert!(
        tests.contains("extension-manifest.schema.json"),
        "the parity row reads the real schema, not a fixture"
    );
}

// Verifies: gh #288 - the shipped extensions are exercised by a test that walks
// the real tree.
//
// Tightening what is accepted is exactly the change that can break a first-party
// extension, and only the real files can tell us. A fixture would have passed.
#[test]
fn the_shipped_manifests_are_exercised() {
    let tests = read("crates/lca-ext-host/tests/abi.rs");
    assert!(
        tests.contains("fn every_shipped_manifest_parses"),
        "a row walks `extensions/*/extension.toml`"
    );
    assert!(
        tests.contains("no longer parses, so this change broke a shipped extension"),
        "and it fails with a message that says what broke"
    );
}
