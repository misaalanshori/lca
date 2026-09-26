//! Cycle-7 driving defect: a data-only extension's version identity was the
//! digest of an **empty component**, so v2 of a skill pack hashed equal to
//! v1 and `lca ext update` answered "up to date" forever - the pack could
//! never be updated. The payload of a data-only package is its `resources/`
//! bag (ADR-0030), so the bag has to be part of the digest.
//!
//! This pins the root cause: two local resolves that differ only in a
//! resource file must produce different digests, and a reinstall must replace
//! the bag wholesale.
//!
//! Verifies: ADR-0030, FR-DIST-7.

use lca_registry::{InstallTree, resolve_local};

fn write_package(dir: &std::path::Path, body: &str) {
    std::fs::create_dir_all(dir.join("resources/skills/demo")).expect("mkdir");
    std::fs::write(
        dir.join("extension.toml"),
        "name = \"demo-pack\"\nversion = \"1.0.0\"\nabi = \"0.4\"\nworlds = []\nresources = [\"skills\"]\n",
    )
    .expect("manifest");
    std::fs::write(
        dir.join("resources/skills/demo/SKILL.md"),
        format!("name: demo\nmatch: demo\n---\n{body}\n"),
    )
    .expect("skill");
}

#[test]
fn a_data_only_digest_covers_its_resources_and_a_reinstall_replaces_them() {
    let root = lca_testkit::scratch_path("regression-23-data-only-digest");
    let _ = std::fs::remove_dir_all(&root);
    let pkg = root.join("pack");
    write_package(&pkg, "v1");

    let tree = InstallTree::new(root.join("extensions"));
    let v1 = resolve_local(&pkg, None).expect("resolve v1");
    tree.install(v1.clone()).expect("install v1");

    write_package(&pkg, "v2");
    let v2 = resolve_local(&pkg, None).expect("resolve v2");
    assert_ne!(
        v1.digest, v2.digest,
        "the resources bag is part of a data-only package's identity"
    );

    // Applying v2 replaces the bag wholesale: the old body is gone.
    tree.install(v2).expect("install v2");
    let installed =
        std::fs::read_to_string(root.join("extensions/demo-pack/resources/skills/demo/SKILL.md"))
            .expect("read installed skill");
    assert!(installed.contains("v2"), "{installed}");
    assert!(!installed.contains("v1"), "{installed}");

    let _ = std::fs::remove_dir_all(&root);
}
