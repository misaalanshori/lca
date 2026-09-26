//! The nightly fuzz workspace drifts out of the main workspace's sight: it
//! is its own `[workspace]`, so no gate builds it, and a parser change left
//! `fuzz/fuzz_targets/archive.rs` uncompilable while the `fuzz` workflow
//! burned the other targets' full 600s first.
//!
//! The build gate is `scripts/fuzz-check.sh` (the `fuzz targets build` CI
//! job). These tests pin the two API contracts the targets asserted wrongly
//! after cycle 4: a data-only manifest has no worlds, and an archive's
//! component may be empty when its bag is present. A target that re-grows
//! the old assumptions fails here first.
//!
//! Verifies: docs/testing-plan.md section 13 (the fuzz targets must match
//! the parser they fuzz), FR-DIST-3, FR-EXT-1.

const DATA_ONLY: &str = r#"name = "skill-pack"
version = "1.0.0"
abi = "0.2"
worlds = []
resources = ["skills"]
"#;

#[test]
fn a_data_only_manifest_parses_with_no_worlds() {
    let manifest = lca_ext_host::Manifest::parse(DATA_ONLY).expect("parses");
    assert!(!manifest.name.is_empty());
    assert!(
        manifest.worlds.is_empty(),
        "`worlds = []` is the data-only shape"
    );
    assert_eq!(manifest.resources, vec!["skills".to_string()]);
}

#[test]
fn a_manifest_with_no_worlds_and_no_resources_is_still_just_a_parse() {
    // The fuzz target asserts "worlds or resources" *after* a successful
    // parse; an empty manifest must parse without panicking, and the
    // caller decides. What must never happen is a panic either way.
    let _ = lca_ext_host::Manifest::parse("name = \"x\"\nversion = \"1\"\nabi = \"0.2\"\n");
}

#[test]
fn an_archive_carries_its_resource_bag() {
    let bytes = lca_registry::pack_archive_with_resources(
        DATA_ONLY,
        &[],
        &[("skills/a/SKILL.md".to_string(), b"# a".to_vec())],
    )
    .expect("pack");
    let archive = lca_registry::read_archive(&bytes).expect("reads");
    assert!(
        archive.component.is_empty(),
        "a data-only package has no component"
    );
    assert_eq!(archive.resources.len(), 1);
    assert_eq!(archive.resources[0].0, "skills/a/SKILL.md");
}
