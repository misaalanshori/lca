//! Finding 2 (critical, `../lca-issues.md`): `ext install` joined the
//! attacker-supplied manifest `name` onto the install tree, allowing an
//! arbitrary-path write. A traversal name must be refused before any write.
//!
//! Verifies: FR-DIST-5 (defect 2).

const MANIFEST: &str = r#"name = "word-count"
version = "1.0.0"
abi = "0.1"
worlds = ["tool"]
description = "Counts words."
"#;

#[test]
fn install_refuses_a_manifest_name_that_escapes_the_tree() {
    let root = lca_testkit::scratch_path("regression-traversal");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let tree = lca_registry::InstallTree::new(root.clone());

    // Not a real component: the resolver treats it as opaque bytes, which is
    // all the naming rule under test cares about.
    let component = b"\0asm\x01\0\0\0-lca-test-component".to_vec();
    let resolved = lca_registry::Resolved {
        digest: lca_registry::Resolved::digest_of(&component),
        source: "https://example.invalid/evil.zip".to_string(),
        manifest: MANIFEST.replace("word-count", "../../escape"),
        component,
        resources: Vec::new(),
    };
    let err = tree
        .install(resolved)
        .expect_err("traversal must be refused");
    assert!(
        err.to_string().contains("not a valid extension name"),
        "{err}"
    );
    // Nothing was written two levels above the tree.
    assert!(
        !root.parent().expect("parent").join("escape").exists(),
        "nothing escaped the install tree"
    );
}
