//! Exclusive scoped writes: session scratch files must stay inside their
//! grant, never replace an existing path, and be private on Unix.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots,
};
use lca_tools::{Capabilities, CapabilityGrants};

struct Deny;

impl PermissionPrompt for Deny {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn engine(name: &str) -> (Capabilities, std::path::PathBuf) {
    let root = lca_testkit::scratch_path(&format!("lca-scoped-write-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let workspace = root.join("workspace");
    let data = root.join(".lca");
    let temp = data.join("tmp/session");
    for dir in [&workspace, &temp] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    let capabilities = Capabilities::new(
        "mcp",
        CapabilityGrants {
            fs: vec![
                ScopeGrant::parse("temp", lca_permissions::FsMode::ReadWrite).expect("temp grant"),
            ],
            fs_declared: true,
            ..Default::default()
        },
        ScopeRoots {
            workspace,
            private: data.join("private"),
            home_config: root.join("config"),
            temp: temp.clone(),
            state_dir: data,
        },
        Arc::new(Mutex::new(Deny)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.clone(),
        None,
    );
    (capabilities, temp)
}

#[test]
fn a_new_temp_file_is_readable_and_created_with_private_permissions() {
    let (caps, temp) = engine("new");
    let path = caps
        .fs_write_new("temp", "mcp-resource.bin", b"resource")
        .expect("create in the session temp scope");

    assert!(path.is_absolute(), "the model receives a usable path");
    assert!(
        path.starts_with(std::fs::canonicalize(&temp).expect("canonical temp")),
        "the resolved file remains in this session's temp directory: {path:?}"
    );
    assert_eq!(
        caps.fs_read("temp", "mcp-resource.bin")
            .expect("read scoped file"),
        b"resource"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o077,
            0,
            "resource bytes are owner-readable only"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_new_temp_file_refuses_to_follow_a_symlink_to_an_existing_file() {
    use std::os::unix::fs::symlink;

    let (caps, temp) = engine("symlink");
    let target = temp.join("target.bin");
    let link = temp.join("resource.bin");
    std::fs::write(&target, b"original").expect("target");
    symlink(&target, &link).expect("symlink");

    assert!(
        caps.fs_write_new("temp", "resource.bin", b"replacement")
            .is_err(),
        "a pre-existing link must not be overwritten"
    );
    assert_eq!(std::fs::read(target).expect("read target"), b"original");
}
