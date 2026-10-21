//! gh #436: the read-before-edit gate must be one registry shared by the
//! built-in tools and extension-backed writers.
//!
//! The defect was that `Capabilities::fs_write` never consulted the tracker the
//! built-in `edit` uses, so `tool.edit_requires_read` could not govern an
//! extension. The fix routes both through `lca_tools::{note_read,
//! has_fresh_read}`, and this file proves the two halves meet: what the
//! built-in `read` records, the extension's `fs_write` honours.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

struct Allow;
impl lca_permissions::PermissionPrompt for Allow {
    fn ask(&mut self, _a: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _d: &lca_permissions::ProposalDiff) -> bool {
        true
    }
}

/// An engine with workspace write access, in a scratch root.
fn engine(tag: &str) -> (std::path::PathBuf, Arc<lca_tools::Capabilities>) {
    let root = std::env::temp_dir().join(format!("lca-436-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    let prompt: Arc<Mutex<dyn lca_permissions::PermissionPrompt>> = Arc::new(Mutex::new(Allow));
    let store = Arc::new(Mutex::new(
        lca_permissions::GrantStore::open(&root.join("grants.json")).unwrap(),
    ));
    let mut cap = lca_tools::Capabilities::new(
        "hash-edit",
        lca_tools::CapabilityGrants {
            fs: vec![
                lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::ReadWrite)
                    .expect("workspace read-write is a valid grant"),
            ],
            fs_declared: true,
            ..Default::default()
        },
        lca_permissions::ScopeRoots {
            workspace: root.join("workspace"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        prompt,
        store,
        root.join("workspace"),
        None,
    );
    let _ = &mut cap;
    (root, Arc::new(cap))
}

// Verifies: gh #436 - with the gate on, an extension write to a file this
// session never read is refused, using the built-in edit's sentence verbatim.
#[test]
fn an_extension_write_is_refused_without_a_read_when_the_gate_is_on() {
    lca_tools::reset_read_registry();
    lca_tools::set_edit_requires_read(true);
    let (root, cap) = engine("unread");
    std::fs::write(root.join("workspace/target.txt"), "alpha\n").unwrap();

    let err = cap
        .fs_write("workspace", "target.txt", b"beta\n")
        .expect_err("gate on, never read: refused");
    let text = err.to_string();
    assert!(
        text.contains("changed since it was last read"),
        "the built-in edit's sentence: {text}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("workspace/target.txt")).unwrap(),
        "alpha\n",
        "a refusal writes nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #436 - what the built-in `read` records, the extension honours:
// the same registry, so a read through one editor unlocks a write through the
// other.
#[test]
fn a_write_after_the_built_in_read_is_allowed() {
    lca_tools::reset_read_registry();
    lca_tools::set_edit_requires_read(true);
    let (root, cap) = engine("after-read");
    let path = root.join("workspace/target.txt");
    std::fs::write(&path, "alpha\n").unwrap();

    // Stand in for the built-in `read` tool's recording step.
    let bytes = std::fs::read(&path).unwrap();
    lca_tools::note_read(&path, &bytes);

    cap.fs_write("workspace", "target.txt", b"beta\n")
        .expect("a fresh read unlocks the write");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "beta\n");
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #436 - a write after the file changed since that read is
// refused: the registry compares fingerprints, not just presence.
#[test]
fn a_write_after_the_file_changed_is_refused() {
    lca_tools::reset_read_registry();
    lca_tools::set_edit_requires_read(true);
    let (root, cap) = engine("changed");
    let path = root.join("workspace/target.txt");
    std::fs::write(&path, "alpha\n").unwrap();
    let bytes = std::fs::read(&path).unwrap();
    lca_tools::note_read(&path, &bytes);

    // Someone else edits it after our read.
    std::fs::write(&path, "changed underneath\n").unwrap();
    let err = cap
        .fs_write("workspace", "target.txt", b"beta\n")
        .expect_err("stale read: refused");
    assert!(
        err.to_string().contains("changed since it was last read"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "changed underneath\n",
        "the other writer's contents survive"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #436 acceptance 2 - no behaviour change when the flag is off
// (pi parity, the product default): a blind extension write lands.
#[test]
fn a_blind_extension_write_lands_when_the_gate_is_off() {
    lca_tools::reset_read_registry();
    lca_tools::set_edit_requires_read(false);
    let (root, cap) = engine("off");
    std::fs::write(root.join("workspace/target.txt"), "alpha\n").unwrap();

    cap.fs_write("workspace", "target.txt", b"beta\n")
        .expect("gate off: no read required");
    assert_eq!(
        std::fs::read_to_string(root.join("workspace/target.txt")).unwrap(),
        "beta\n"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #436 - creating a *new* file is never gated: there is nothing
// for it to be stale against, and refusing would break every writer that
// creates files.
#[test]
fn creating_a_new_file_is_never_gated() {
    lca_tools::reset_read_registry();
    lca_tools::set_edit_requires_read(true);
    let (root, cap) = engine("new-file");

    cap.fs_write("workspace", "brand-new.txt", b"fresh\n")
        .expect("a new file has no prior contents to be stale against");
    assert_eq!(
        std::fs::read_to_string(root.join("workspace/brand-new.txt")).unwrap(),
        "fresh\n"
    );
    let _ = std::fs::remove_dir_all(&root);
}
