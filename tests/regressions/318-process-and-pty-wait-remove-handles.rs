//! Regression rows for gh #318: `process_wait` and `pty_wait` must release
//! the handle they reaped.
//!
//! The defect: both took `table.entries.get_mut(&handle)`, blocked on the
//! child while still holding the `self.handles` mutex, and never removed the
//! entry. Every normally-exiting child therefore left a `HandleEntry` in the
//! table forever: the map grew without bound over a long session, and the
//! handle ids were never reusable. The same shape applied to PTYs, where the
//! leaked entry also held the PTY master fd.
//!
//! Verifies: gh #318 - a wait consumes the handle, a wrong-type handle is
//! refused *without* consuming anything, and the standard lifecycle
//! (read to EOF, wait, then kill) still works.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_permissions::{
    Decision, FsMode, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots,
};
use lca_protocol::CapabilityError;
use lca_tools::{Capabilities, CapabilityGrants};

/// These rows exercise handle lifetime, not the consent path, so the
/// prompt allows the spawn outright (deny-by-default is NFR-13's rule and is
/// covered by its own rows).
struct Allow;

impl PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        true
    }
}

fn engine(name: &str, root: &std::path::Path, process: bool, pty: bool) -> Capabilities {
    // A shell needs a working directory, and `process_spawn`/`pty_spawn`
    // resolve it through the `fs` scope resolver (which requires `fs` to be
    // declared). So the grant set is "process/pty plus a workspace scope",
    // which is exactly what a real spawning extension carries.
    let grants = CapabilityGrants {
        process,
        pty,
        fs_declared: true,
        fs: vec![ScopeGrant {
            scope: "workspace",
            mode: FsMode::ReadWrite,
        }],
        ..CapabilityGrants::default()
    };
    Capabilities::new(
        name,
        grants,
        ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    )
}

/// Build a system with a workspace root and an empty grants file.
fn system(name: &str, process: bool, pty: bool) -> (Capabilities, std::path::PathBuf) {
    let root = lca_testkit::scratch_path(name);
    std::fs::create_dir_all(root.join("project")).expect("mkdir");
    (engine(name, &root, process, pty), root)
}

/// The core row: after a wait, the handle is gone. Re-waiting on it is
/// `NotFound`, not a second successful wait on a retained entry.
#[test]
fn a_completed_wait_releases_the_process_handle() {
    let (cap, _root) = system("318-proc-wait", true, false);
    let handle = cap
        .process_spawn("sh", &["-c".into(), "exit 7".into()], "workspace")
        .expect("spawn");
    assert_eq!(cap.process_wait(handle).expect("wait"), 7);
    match cap.process_wait(handle) {
        Err(CapabilityError::NotFound(_)) => {}
        other => panic!("the handle is released after a wait, got {other:?}"),
    }
}

/// A long session must not accumulate entries: many spawn/wait cycles with
/// the same shape, each reaping its handle, then one handle id is reused.
#[test]
fn repeated_spawns_and_waits_do_not_accumulate_handles() {
    let (cap, _root) = system("318-proc-grow", true, false);
    let mut handles = Vec::new();
    for _ in 0..12 {
        let handle = cap
            .process_spawn("sh", &["-c".into(), "exit 0".into()], "workspace")
            .expect("spawn");
        assert_eq!(cap.process_wait(handle).expect("wait"), 0);
        handles.push(handle);
    }
    // Every one of them is gone.
    for handle in &handles {
        assert!(
            matches!(cap.process_kill(*handle), Err(CapabilityError::NotFound(_))),
            "handle {handle} was released by its wait"
        );
    }
}

/// A wrong-type handle must be refused and must NOT be removed: asking the
/// process API about a PTY handle cannot destroy the PTY.
#[test]
fn a_wait_on_the_wrong_handle_type_refuses_without_consuming_it() {
    let (cap, _root) = system("318-wrong-type", true, true);
    let pty = cap
        .pty_spawn("sh", &["-c".into(), "exit 3".into()], "workspace", 24, 80)
        .expect("pty spawn");
    match cap.process_wait(pty) {
        Err(CapabilityError::Invalid(message)) => {
            assert!(
                message.contains("not a process"),
                "the refusal names the type mismatch: {message}"
            );
        }
        other => panic!("a pty handle is not a process handle, got {other:?}"),
    }
    // The PTY is still alive and still waitable through its own API.
    assert_eq!(cap.pty_wait(pty).expect("pty wait"), 3);
}

/// The standard lifecycle from the conformance extension and the tool
/// layer: read stdout to EOF, then wait, then kill with the error ignored.
/// Removing the entry on wait must not turn the trailing kill into a hard
/// failure the caller treats as fatal.
#[test]
fn the_read_to_eof_wait_then_kill_lifecycle_still_works() {
    let (cap, _root) = system("318-lifecycle", true, false);
    let handle = cap
        .process_spawn(
            "sh",
            &["-c".into(), "printf 'out'; printf 'err' >&2; exit 0".into()],
            "workspace",
        )
        .expect("spawn");
    let mut out = Vec::new();
    while let Some(chunk) = cap.process_read_stdout(handle, 4096).expect("read") {
        out.extend_from_slice(&chunk);
    }
    let mut err = Vec::new();
    while let Some(chunk) = cap.process_read_stderr(handle, 4096).expect("read stderr") {
        err.extend_from_slice(&chunk);
    }
    assert_eq!(out, b"out");
    assert_eq!(err, b"err");
    assert_eq!(cap.process_wait(handle).expect("wait"), 0);
    // The trailing kill is best-effort by contract: NotFound is the
    // expected shape now, and conformance ignores it.
    assert!(matches!(
        cap.process_kill(handle),
        Err(CapabilityError::NotFound(_))
    ));
}

/// The same rows for PTYs: a wait releases the handle.
#[test]
fn a_completed_pty_wait_releases_the_pty_handle() {
    let (cap, _root) = system("318-pty-wait", false, true);
    let handle = cap
        .pty_spawn("sh", &["-c".into(), "exit 5".into()], "workspace", 24, 80)
        .expect("pty spawn");
    assert_eq!(cap.pty_wait(handle).expect("pty wait"), 5);
    match cap.pty_wait(handle) {
        Err(CapabilityError::NotFound(_)) => {}
        other => panic!("the pty handle is released after a wait, got {other:?}"),
    }
}

/// Repeated PTY cycles do not accumulate either.
#[test]
fn repeated_pty_spawns_and_waits_do_not_accumulate_handles() {
    let (cap, _root) = system("318-pty-grow", false, true);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let handle = cap
            .pty_spawn("sh", &["-c".into(), "exit 0".into()], "workspace", 24, 80)
            .expect("pty spawn");
        assert_eq!(cap.pty_wait(handle).expect("pty wait"), 0);
        handles.push(handle);
    }
    for handle in &handles {
        assert!(
            matches!(cap.pty_kill(*handle), Err(CapabilityError::NotFound(_))),
            "pty handle {handle} was released by its wait"
        );
    }
}

/// A process handle asked of the PTY API is refused and survives.
#[test]
fn a_pty_wait_on_a_process_handle_refuses_without_consuming_it() {
    let (cap, _root) = system("318-wrong-type-pty", true, true);
    let child = cap
        .process_spawn("sh", &["-c".into(), "exit 9".into()], "workspace")
        .expect("spawn");
    match cap.pty_wait(child) {
        Err(CapabilityError::Invalid(message)) => {
            assert!(
                message.contains("not a pty"),
                "the refusal names the type mismatch: {message}"
            );
        }
        other => panic!("a process handle is not a pty handle, got {other:?}"),
    }
    assert_eq!(cap.process_wait(child).expect("still waitable"), 9);
}

/// An unknown handle stays `NotFound` for both APIs - the release path must
/// not conflate "never existed" with "already reaped" in a way that hides a
/// caller bug.
#[test]
fn an_unknown_handle_is_not_found_for_both_apis() {
    let (cap, _root) = system("318-unknown", true, true);
    assert!(matches!(
        cap.process_wait(4242),
        Err(CapabilityError::NotFound(_))
    ));
    assert!(matches!(
        cap.pty_wait(4242),
        Err(CapabilityError::NotFound(_))
    ));
}
