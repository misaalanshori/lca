//! The install tree and the consent surface (FR-DIST-6/7, FR-PERM-2): the
//! canonical grant hash, the consent sentences, the update prompt, and the
//! directory the component lands in.
//!
//! Split out of `tests/registry.rs` at the subjects gh #561's production split
//! created, so a row sits beside the module that owns it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::support::{
    MANIFEST, MANIFEST_REORDERED, MANIFEST_WIDENED, MANIFEST_WITH_RESOURCES, component, tree,
};
use lca_registry::grant_hash;

// Verifies: the FR-DIST-7 comparison basis - the hash covers the
// manifest-declared grant set only, is insensitive to the order the
// file writes them in, and changes the moment a value changes.
#[test]
fn the_grant_hash_is_canonical_and_value_sensitive() {
    let a = grant_hash(MANIFEST).expect("hash");
    let b = grant_hash(MANIFEST_REORDERED).expect("hash");
    assert_eq!(a, b, "same grants, different written order, same hash");
    assert!(a.starts_with("sha256:"));

    let widened = grant_hash(MANIFEST_WIDENED).expect("hash");
    assert_ne!(a, widened, "a new capability changes the hash");

    let reason_changed = grant_hash(&MANIFEST.replace("Runs wc", "Runs other")).expect("hash");
    assert_ne!(a, reason_changed, "a changed declaration changes the hash");
}

// Verifies: the install boundary never joins an attacker-supplied
// manifest name onto the filesystem (FR-DIST-5 consent path; ADR-0010's
// unzip). A path-traversal name is refused before any write, and no
// directory escapes the tree.
#[test]
fn install_refuses_a_manifest_name_that_escapes_the_tree() {
    let root = lca_testkit::scratch_path("lca-registry-traversal");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let tree = lca_registry::InstallTree::new(root.clone());

    let component = component();
    let resolved = lca_registry::Resolved {
        digest: lca_registry::Resolved::digest_of(&component),
        source: "https://example.invalid/evil.zip".to_string(),
        commit: String::new(),
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

// Verifies: FR-DIST-6 (digest and source recorded, reused) and the
// SRDD install tree: component named by content digest, manifest
// beside it, lockfile at the top; remove forgets both (FR-DIST... the
// ext remove command's backing store).
#[test]
fn install_records_by_digest_and_roundtrips() {
    let tree = tree("install");
    let digest = lca_registry::Resolved::digest_of(&component());
    let resolved = lca_registry::Resolved {
        manifest: MANIFEST.to_string(),
        component: component(),
        digest: digest.clone(),
        source: "ghcr.io/example/word-count:abi-0.4".to_string(),
        commit: String::new(),
        resources: Vec::new(),
    };
    let entry = tree.install(resolved).expect("install");
    assert_eq!(entry.digest, digest);
    assert_eq!(entry.source, "ghcr.io/example/word-count:abi-0.4");
    assert_eq!(entry.version, "1.0.0");
    assert_eq!(entry.abi, "0.4");
    assert_eq!(entry.grant_hash, grant_hash(MANIFEST).expect("hash"));

    // The component is on disk under its digest (load-by-digest has a
    // filename to open, FR-DIST-8).
    let bytes = tree
        .component("word-count", &digest)
        .expect("read back by digest");
    assert_eq!(bytes, component());
    assert!(tree.manifest_path("word-count").exists());
    assert!(tree.lockfile_path().exists());

    let listed = tree.list().expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, "word-count");

    assert!(tree.remove("word-count").expect("remove"), "removed");
    assert!(!tree.manifest_path("word-count").exists());
    assert!(tree.list().expect("list").is_empty());
    assert!(!tree.remove("word-count").expect("second remove is a no-op"));
}

// Verifies: the consent screen's content - the capability catalog's
// sentences, the required reason shown verbatim with its suffix
// (FR-DIST-5: the local path applies this same flow).
#[test]
fn consent_lines_are_the_catalog_sentences() {
    let lines = lca_registry::consent_lines(MANIFEST).expect("lines");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        lines[0],
        "Runs wc for you. Each command still asks for your approval."
    );

    let provider = r#"name = "p"
version = "1.0.0"
abi = "0.4"
worlds = ["provider"]
description = "x"

[capabilities.net]
hosts = ["api.openai.com", "*.googleapis.com"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "p"
"#;
    let lines = lca_registry::consent_lines(provider).expect("lines");
    assert_eq!(lines.len(), 3, "{lines:?}");
    // Content, not position: the capability table iterates in map order.
    assert!(
        lines
            .iter()
            .any(|l| l.contains("api.openai.com") && l.contains("any subdomain of googleapis.com")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("local port")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("saved credentials")),
        "{lines:?}"
    );

    let completer = r#"name = "c"
version = "1.0.0"
abi = "0.4"
worlds = ["compaction"]
description = "x"

[capabilities.completion]
reason = "Summarizes older parts of the conversation when compacting."
"#;
    let lines = lca_registry::consent_lines(completer).expect("lines");
    assert_eq!(
        lines[0],
        "Summarizes older parts of the conversation when compacting. \
         This lets it ask the current model for a response."
    );

    let unknown = lca_registry::consent_lines("name=\"x\"\n[capabilities.mystery]\nreason=\"r\"")
        .expect_err("unknown capability");
    assert!(unknown.to_string().contains("mystery"), "{unknown}");
}

// Verifies: FR-DIST-7's prompt rule - a widened capability set prompts,
// an identical or narrower one does not.
#[test]
fn an_update_prompts_only_when_grants_widen() {
    assert!(
        !lca_registry::update_widens_grants(MANIFEST, MANIFEST).expect("compare"),
        "identical never prompts"
    );
    assert!(
        !lca_registry::update_widens_grants(MANIFEST_WIDENED, MANIFEST).expect("compare"),
        "narrowing never prompts"
    );
    assert!(
        lca_registry::update_widens_grants(MANIFEST, MANIFEST_WIDENED).expect("compare"),
        "a new capability prompts (FR-DIST-7)"
    );
    assert!(
        lca_registry::update_widens_grants(MANIFEST, &MANIFEST.replace("Runs wc", "Runs other"))
            .expect("compare"),
        "a changed declaration prompts: the user approved the old words"
    );
}

// The fs consent sentence matches the mode: a read-only grant must not
// claim it can write (the install screen is the user's only warning).
#[test]
fn the_fs_consent_sentence_matches_the_granted_mode() {
    let read_only = r#"name = "r"
version = "1.0.0"
abi = "0.4"
worlds = ["context-transform"]
description = "x"

[capabilities.fs]
workspace = "read"
"#;
    let files_line = |manifest: &str| {
        lca_registry::consent_lines(manifest)
            .expect("lines")
            .into_iter()
            .find(|line| line.starts_with("Files:"))
            .expect("an fs line")
    };
    assert_eq!(
        files_line(read_only),
        "Files: workspace (read). It can read those files."
    );
    let read_write = read_only.replace("workspace = \"read\"", "workspace = \"read-write\"");
    assert_eq!(
        files_line(&read_write),
        "Files: workspace (read and write). It can read and write those files."
    );
}

// Verifies: ADR-0030 (an undeclared kind is refused, and the install
// writes the declared bag where the host serves it).
#[test]
fn install_writes_declared_resources_and_refuses_an_undeclared_kind() {
    let install_tree = tree("resources-install");
    let resources = vec![("presets/p.toml".to_string(), b"id = \"x\"".to_vec())];
    install_tree
        .install(lca_registry::Resolved {
            digest: lca_registry::Resolved::digest_of(&component()),
            source: "https://example.invalid/word-count.zip".to_string(),
            commit: String::new(),
            manifest: MANIFEST_WITH_RESOURCES.to_string(),
            component: component(),
            resources: resources.clone(),
        })
        .expect("install");
    assert_eq!(
        std::fs::read(
            install_tree
                .root()
                .join("word-count/resources/presets/p.toml")
        )
        .expect("read"),
        b"id = \"x\""
    );

    let tree2 = tree("resources-undeclared");
    let err = tree2
        .install(lca_registry::Resolved {
            digest: lca_registry::Resolved::digest_of(&component()),
            source: "https://example.invalid/word-count.zip".to_string(),
            commit: String::new(),
            manifest: MANIFEST_WITH_RESOURCES.to_string(),
            component: component(),
            resources: vec![("wads/doom.wad".to_string(), b"x".to_vec())],
        })
        .expect_err("undeclared kind refused");
    assert!(err.to_string().contains("does not declare"), "{err}");
    assert!(
        !tree2.root().join("word-count").exists(),
        "a refused install writes nothing"
    );
}

// Verifies: ADR-0030 (a data-only package: `worlds = []`, resources only).
#[test]
fn a_data_only_package_installs_and_removes() {
    const DATA_MANIFEST: &str = r#"name = "skill-pack"
version = "1.0.0"
abi = "0.4"
worlds = []
description = "A skill pack."
resources = ["skills"]
"#;
    let resources = vec![(
        "skills/commits/SKILL.md".to_string(),
        b"name: commits\n---\nbody".to_vec(),
    )];
    let packed =
        lca_registry::pack_archive_with_resources(DATA_MANIFEST, &[], &resources).expect("pack");
    let archive = lca_registry::read_archive(&packed).expect("read");
    assert!(
        archive.component.is_empty(),
        "no component in a data-only pack"
    );

    let tree = tree("data-only");
    tree.install(lca_registry::Resolved {
        digest: lca_registry::Resolved::digest_of(&[]),
        source: "https://example.invalid/skill-pack.zip".to_string(),
        commit: String::new(),
        manifest: DATA_MANIFEST.to_string(),
        component: Vec::new(),
        resources: resources.clone(),
    })
    .expect("install");
    assert!(
        tree.root()
            .join("skill-pack/resources/skills/commits/SKILL.md")
            .is_file()
    );
    assert!(tree.remove("skill-pack").expect("remove"));
    assert!(!tree.root().join("skill-pack").exists());
}
