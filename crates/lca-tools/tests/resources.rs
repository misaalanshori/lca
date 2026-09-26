//! The `resources` bag (ADR-0030, ADR-0032): own-tree reads, traversal
//! refusal, the embedded/directory parity the two delivery modes promise,
//! and the per-call size cap.
//!
//! Verifies: FR-PERM-6, FR-PERM-7, NFR-25.

use std::sync::{Arc, Mutex};

use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::CapabilityError;
use lca_tools::{Capabilities, CapabilityGrants, RESOURCE_READ_MAX_BYTES, ResourceSource};

struct Deny;

impl PermissionPrompt for Deny {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn engine(name: &str, root: &std::path::Path, source: ResourceSource) -> Capabilities {
    let mut cap = Capabilities::new(
        name,
        CapabilityGrants::default(),
        ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Deny)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    );
    cap.set_resources(source);
    cap
}

fn bag(root: &std::path::Path) -> std::path::PathBuf {
    let dir = root.join("resources");
    std::fs::create_dir_all(dir.join("skills")).expect("mkdir");
    std::fs::write(dir.join("presets.toml"), "id = \"x\"").expect("write");
    std::fs::write(dir.join("skills/a.md"), "hi").expect("write");
    dir
}

#[test]
fn a_directory_bag_reads_its_own_tree_and_refuses_escape() {
    let root = lca_testkit::scratch_path("resources-dir");
    let dir = bag(&root);
    let cap = engine("demo", &root, ResourceSource::Dir(dir));

    assert_eq!(
        cap.resource_list("").expect("list"),
        vec![
            ("presets.toml".to_string(), 8),
            ("skills/a.md".to_string(), 2)
        ],
        "every file, sorted, relative to the bag root"
    );
    assert_eq!(
        cap.resource_list("skills").expect("list prefix"),
        vec![("skills/a.md".to_string(), 2)]
    );
    assert_eq!(
        cap.resource_read("presets.toml").expect("read"),
        b"id = \"x\""
    );
    assert_eq!(
        cap.resource_read("skills/a.md").expect("read nested"),
        b"hi"
    );

    // Hostile cases: traversal, absolute, and a missing file are refused
    // with distinct errors, and the escape attempt is recorded.
    let traversal = cap.resource_read("../../etc/passwd").unwrap_err();
    assert!(
        matches!(traversal, CapabilityError::Permission(_)),
        "traversal is a permission refusal: {traversal:?}"
    );
    assert!(
        cap.denials()
            .iter()
            .any(|denial| denial.capability == "resources"),
        "the escape attempt is recorded for `lca ext info`"
    );
    assert!(matches!(
        cap.resource_read("/etc/passwd").unwrap_err(),
        CapabilityError::Permission(_)
    ));
    assert!(matches!(
        cap.resource_read("missing.txt").unwrap_err(),
        CapabilityError::NotFound(_)
    ));
    // A symlink out of the bag is refused too (canonicalized check).
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", root.join("resources/escape")).expect("symlink");
        assert!(
            matches!(
                cap.resource_read("escape").unwrap_err(),
                CapabilityError::Permission(_)
            ),
            "a symlink out of the tree is refused"
        );
    }
}

#[test]
fn an_embedded_bag_serves_the_same_bytes_as_a_directory() {
    static TABLE: &[(&str, &[u8])] = &[("presets.toml", b"id = \"x\""), ("skills/a.md", b"hi")];
    let root = lca_testkit::scratch_path("resources-embedded");
    let dir = bag(&root);
    let dir_cap = engine("demo", &root, ResourceSource::Dir(dir));
    let embedded_cap = engine("demo", &root, ResourceSource::Embedded(TABLE));

    assert_eq!(
        embedded_cap.resource_list("").expect("list"),
        dir_cap.resource_list("").expect("list"),
        "the two delivery modes agree on the listing"
    );
    for path in ["presets.toml", "skills/a.md"] {
        assert_eq!(
            embedded_cap.resource_read(path).expect("read"),
            dir_cap.resource_read(path).expect("read"),
            "and on the bytes of `{path}`"
        );
    }
    assert!(matches!(
        embedded_cap.resource_read("missing.txt").unwrap_err(),
        CapabilityError::NotFound(_)
    ));
}

#[test]
fn a_bag_with_no_resources_lists_empty_rather_than_failing() {
    let root = lca_testkit::scratch_path("resources-none");
    let cap = engine("demo", &root, ResourceSource::None);
    assert!(cap.resource_list("").expect("list").is_empty());
    assert!(matches!(
        cap.resource_read("anything").unwrap_err(),
        CapabilityError::NotFound(_)
    ));
}

#[test]
fn a_read_over_the_size_cap_is_refused() {
    let root = lca_testkit::scratch_path("resources-cap");
    let dir = bag(&root);
    std::fs::write(
        dir.join("big.bin"),
        vec![0u8; RESOURCE_READ_MAX_BYTES as usize + 1],
    )
    .expect("write big");
    let cap = engine("demo", &root, ResourceSource::Dir(dir));
    let err = cap.resource_read("big.bin").unwrap_err();
    assert!(
        matches!(err, CapabilityError::Invalid(ref detail) if detail.contains("read cap")),
        "over the cap: {err:?}"
    );
}
