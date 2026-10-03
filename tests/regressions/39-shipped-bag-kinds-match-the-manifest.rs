//! Review finding on gh #34 (no issue id: a review, not a report): the
//! shipped `openai-compatible` package declared `resources =
//! ["provider-presets"]` while its bag shipped `provider-presets.toml`, and
//! a kind is the *first path segment* of a resource's path - so the segment
//! is `provider-presets.toml`, which the manifest did not name. Installing
//! that extension with its bag has failed with `invalid artifact: resource
//! ... does not declare` ever since the flat-file layout landed, which the
//! bundled build hides (it reads the embedded bag and never installs).
//!
//! `crates/lca-registry/tests/registry.rs`'s kind rows could not catch it:
//! they pair a synthetic manifest with synthetic files, and nothing ever
//! asked whether the REAL package's manifest matched its REAL bag. This row
//! does, through the registry's own install validation rather than a copy of
//! its rule, so the two cannot drift apart.
//!
//! Only one extension ships a bag (`find extensions/*/resources` says so);
//! the row walks whatever is there rather than naming the files, so a new
//! one is covered the day it lands.
//!
//! Verifies: NFR-24 (a released defect's guard), ADR-0030 (the installer
//! refuses a kind the manifest does not declare), `docs/extension-authoring.md`
//! "Resources and state".

use std::path::Path;

/// Every file in a package's `resources/` bag as the registry models it:
/// `(relative path, bytes)` - the path is what a kind is derived from.
fn bag(dir: &Path, prefix: &str) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = format!("{prefix}{name}");
        if path.is_dir() {
            files.extend(bag(&path, &format!("{rel}/")));
        } else if let Ok(bytes) = std::fs::read(&path) {
            files.push((rel, bytes));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

// Verifies: gh #34 review finding - every file the shipped package's bag
// carries has its kind declared by that package's own manifest, so the
// registry accepts a real install of a real package with its real bag.
#[test]
fn every_shipped_resource_kind_is_declared_by_its_own_manifest() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let package = repo.join("extensions/openai-compatible");
    let manifest = std::fs::read_to_string(package.join("extension.toml")).expect("manifest");
    let resources = bag(&package.join("resources"), "");
    assert!(
        !resources.is_empty(),
        "the package still ships a bag; if it ever stops, this row's premise is gone"
    );

    // The registry's own validation: `InstallTree::install` parses the
    // manifest, derives each resource's kind, and refuses an undeclared one
    // before writing anything.
    let component = std::fs::read(package.join("fixtures/component.wasm")).expect("component");
    let root = lca_testkit::scratch_path("regression-39-shipped-bag-kinds");
    let _ = std::fs::remove_dir_all(&root);
    let tree = lca_registry::InstallTree::new(root.clone());
    tree.install(lca_registry::Resolved {
        digest: lca_registry::Resolved::digest_of(&component),
        source: "extensions/openai-compatible".to_string(),
        manifest,
        component,
        resources: resources.clone(),
    })
    .expect("the real package installs with its real bag");

    // And the bag landed where the host serves it from (ADR-0030), file for
    // file - a validation that passed by dropping a file would be no guard.
    let installed = root.join("openai-compatible/resources");
    for (file, _) in &resources {
        assert!(
            installed.join(file).is_file(),
            "`{file}` is in the shipped bag but not in the installed one"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
