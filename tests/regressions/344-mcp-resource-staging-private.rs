//! Gh #344: `read_result` staged binary MCP resource payloads in a shared
//! world-writable `/tmp/lca-mcp-resources` with predictable
//! `{server}-{counter}-{safe}` names, written by a raw `std::fs::write`.
//!
//! Four flaws came from that one call: the directory is shared (a local
//! attacker pre-plants a symlink at a predictable name and the user's own
//! process writes through it), a raw write bypasses the capability scopes, a
//! counter that restarts at 0 every process makes the names guessable, and
//! nothing ever cleaned the files up.
//!
//! Staging now runs through `Capabilities::fs_write_new` on the `temp` scope:
//! one private directory per session (`<data>/tmp/<session>`, removed at
//! exit), `create_new` so a planted entry is an error rather than a target,
//! and owner-only permissions on both the file and its directory.
//!
//! Verifies: gh #344 acceptance 1-3 at the crate boundary — the scope check
//! runs, the write is exclusive, and the permission bits are private.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Arc;

use lca_permissions::{FsMode, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots};
use lca_tools::{Capabilities, CapabilityGrants};

/// Never consults a prompt: a staging write is a capability-scope question,
/// not a user decision.
struct Deny;
impl PermissionPrompt for Deny {
    fn ask(&mut self, action: &lca_permissions::Action) -> lca_permissions::Decision {
        panic!("staging must not prompt: {}", action.display());
    }
    fn review_proposals(&mut self, _: &ProposalDiff) -> bool {
        false
    }
}

/// The production layout: `temp` lives under the data home.
fn engine(name: &str, with_temp: bool) -> (Arc<Capabilities>, std::path::PathBuf) {
    let root = lca_testkit::scratch_path(&format!("regression-344-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    let temp = data.join("tmp").join("session-1");
    for dir in [
        root.join("ws").as_path(),
        temp.as_path(),
        data.join("sessions").as_path(),
    ] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    let mut scopes = vec![ScopeGrant::parse("workspace", FsMode::Read).expect("grant")];
    if with_temp {
        scopes.push(ScopeGrant::parse("temp", FsMode::ReadWrite).expect("grant"));
    }
    let caps = Capabilities::new(
        "mcp",
        CapabilityGrants {
            fs: scopes,
            fs_declared: true,
            ..Default::default()
        },
        ScopeRoots {
            workspace: root.join("ws"),
            private: data.join("private"),
            home_config: root.join("config"),
            temp: temp.clone(),
            state_dir: data.clone(),
        },
        Arc::new(std::sync::Mutex::new(Deny)),
        Arc::new(std::sync::Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("ws"),
        None,
    );
    (Arc::new(caps), temp)
}

// Verifies: gh #344 acceptance 1 — a staged blob lands inside the session temp
// scope, and its directory and the file are both owner-only.
#[cfg(unix)]
#[test]
fn a_staged_blob_is_private_in_the_session_temp_scope() {
    use std::os::unix::fs::PermissionsExt;
    let (caps, temp) = engine("private", true);
    caps.fs_write_new("temp", "server-0-blob.bin", b"payload")
        .expect("staging inside the granted temp scope");
    let path = temp.join("server-0-blob.bin");
    assert_eq!(std::fs::read(&path).expect("read"), b"payload");

    let dir_mode = std::fs::metadata(&temp)
        .expect("stat dir")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "the staging directory is owner-only: {dir_mode:o}"
    );
    let file_mode = std::fs::metadata(&path)
        .expect("stat file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        file_mode, 0o600,
        "the staged file is owner-only: {file_mode:o}"
    );
}

// Verifies: gh #344 acceptance 1, the attack refuted. A symlink planted at the
// exact path staging is about to use is refused rather than followed.
#[cfg(unix)]
#[test]
fn a_planted_symlink_in_the_staging_area_is_never_followed() {
    let (caps, temp) = engine("symlink", true);
    let victim = temp.parent().expect("parent").join("precious.txt");
    std::fs::write(&victim, b"UNTOUCHED").expect("write victim");
    let planted = temp.join("server-0-blob.bin");
    std::os::unix::fs::symlink(&victim, &planted).expect("plant");

    let err = caps
        .fs_write_new("temp", "server-0-blob.bin", b"attack")
        .expect_err("a planted entry is refused, not written through");
    // Defense in depth: the scope resolver canonicalises the path, sees the
    // link leaving the temp root, and refuses with `Escape` before
    // `create_new` is even reached. (If a planted entry inside the root were
    // ever passed through, `create_new` is the second gate.)
    let text = err.to_string();
    assert!(
        text.contains("left the scope") || text.contains("exists") || text.contains("stage"),
        "the refusal names the cause: {text}"
    );
    assert_eq!(
        std::fs::read(&victim).expect("victim"),
        b"UNTOUCHED",
        "the planted link's target is untouched"
    );
    let meta = std::fs::symlink_metadata(&planted).expect("planted");
    assert!(
        meta.file_type().is_symlink(),
        "and the link is still a link"
    );
}

// Verifies: gh #344 acceptance 2 — the write is a capability-scope question.
// Without the `temp` grant, staging is refused outright (it used to succeed
// by writing to the system temp dir, bypassing every scope).
#[test]
fn staging_without_the_temp_grant_is_refused() {
    let (caps, temp) = engine("notemp", false);
    let err = caps
        .fs_write_new("temp", "server-0-blob.bin", b"payload")
        .expect_err("no grant, no staging");
    assert!(
        matches!(err, lca_protocol::CapabilityError::NotGranted(_))
            || matches!(err, lca_protocol::CapabilityError::Permission(_)),
        "the scope refuses it: {err}"
    );
    assert!(
        !temp.join("server-0-blob.bin").exists(),
        "and nothing was written"
    );
    assert!(
        caps.denials()
            .iter()
            .any(|denial| denial.parameter.contains("temp")),
        "the refusal is recorded (FR-PERM-5)"
    );
}

// Verifies: gh #344 acceptance 1 — the shared directory is gone from the
// code path, so nothing writes there even when it exists and is writable.
#[test]
fn nothing_is_written_to_the_shared_system_temp_directory() {
    let shared = std::env::temp_dir().join("lca-mcp-resources");
    let before = std::fs::read_dir(&shared).map(|d| d.count()).unwrap_or(0);
    let (caps, _temp) = engine("shared", true);
    caps.fs_write_new("temp", "server-0-blob.bin", b"payload")
        .expect("staging");
    let after = std::fs::read_dir(&shared).map(|d| d.count()).unwrap_or(0);
    assert_eq!(before, after, "the shared arena gained nothing");
}
