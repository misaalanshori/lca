//! The real-terminal harness's own contract (gh #413's class).
//!
//! Every `e2e_*` row drives a real tmux pane, and the harness had two ways to
//! turn a correct product into a red test under load: one shared socket name
//! (so two suites drove each other's servers) and a fixed wall-clock budget
//! that panicked even when the pane had never painted. Those cost this project
//! six ledger entries for one row and two false fix-up hunts.
//!
//! These rows pin the contract the harness now keeps, against fake commands —
//! no product code, no `lca` binary — because the property under test is the
//! *harness's* behaviour:
//!
//! - a socket that belongs to this binary alone;
//! - an empty pane is a terminal that did not come up, and the wait re-spawns
//!   once before giving up;
//! - a painted pane missing its needle is still a failure.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #413's class - the socket name carries this binary's pid, so two
// concurrent suites on one machine cannot share a tmux server. The row reads
// the name the harness actually uses rather than a constant, because the
// constant is precisely what changed.
#[cfg(unix)]
#[test]
fn the_harness_socket_belongs_to_this_binary() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let socket = common::tmux_socket_for_tests();
    assert!(
        socket.starts_with("lca-tui-test-"),
        "the per-run prefix survives: {socket}"
    );
    assert!(
        socket.ends_with(&std::process::id().to_string()),
        "the socket ends in this process's pid, so a second suite gets its own: {socket}"
    );
    assert_ne!(
        socket, "lca-tui-test",
        "the shared name that caused the class must be gone"
    );
    // Two sessions from this binary share the one server (the tests rely on
    // it: one `Tmux` per row, several rows per binary).
    let first = Tmux::new("socket-contract-a");
    let second = Tmux::new("socket-contract-b");
    assert_eq!(
        common::tmux_socket_for_tests(),
        socket,
        "the name is stable within a binary"
    );
    assert_ne!(first.name, second.name, "sessions stay distinct");
}

#[cfg(unix)]
#[test]
fn a_painted_pane_missing_its_needle_is_still_a_failure() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("painted-wrong");
    // A real paint with real content, but never the needle the row wants.
    session.spawn_raw("sh -c 'echo painted-content; sleep 30'");
    session.wait_for("painted-content", std::time::Duration::from_secs(20));

    // The panic is the contract: a screen with content that lacks the needle
    // must NOT be re-spawned, and must fail.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session.wait_for(
            "never-going-to-appear",
            std::time::Duration::from_millis(400),
        );
    }));
    let err = outcome.expect_err("a painted pane missing its needle must fail");
    let message = err
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "non-string panic".to_string());
    assert!(
        message.contains("never appeared in the pane"),
        "the failure names the needle and shows the frame: {message}"
    );
    assert!(
        !message.contains("re-spawn"),
        "a painted pane is never re-spawned - that would hide a real failure: {message}"
    );
}

// Verifies: the harness contract - an **empty** pane is a terminal that did not
// come up, so the wait re-spawns once and succeeds when the second paint lands.
//
// The fake command paints only on its second run: the marker file is the
// counter, so the first spawn exits blank (empty pane) and the re-spawn draws
// the needle. That is precisely the load shape the ledger recorded, made
// deterministic.
#[cfg(unix)]
#[test]
fn an_empty_pane_is_respawned_once_and_then_succeeds() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    // A plain file in a scratch directory (`scratch_path` itself is a
    // *directory*, so `[ -f ]` would be false even after the first run).
    let marker = lca_testkit::scratch_path("harness-empty-pane-marker").join("painted");
    let _ = std::fs::remove_file(&marker);
    let marker = marker.to_string_lossy().into_owned();
    let session = Tmux::new("empty-pane-retry");
    // First run: touch the marker and sit on a *blank* screen (no output at
    // all) without exiting — a command that exits takes its pane with it, and
    // the harness would then be re-spawning into a dead session rather than
    // healing a live-but-unpainted one. Second run: the marker exists, so
    // paint the needle and stay up.
    session.spawn_raw(&format!(
        "sh -c 'if [ -f {marker} ]; then echo recovered-after-respawn; sleep 30; \
         else touch {marker}; sleep 30; fi'"
    ));
    // The first spawn leaves a blank pane, so this call must re-spawn and then
    // find the needle the second paint draws.
    let pane = session.wait_for("recovered-after-respawn", std::time::Duration::from_secs(5));
    assert!(
        pane.contains("recovered-after-respawn"),
        "the re-spawned pane painted: {pane}"
    );
    assert!(
        std::fs::metadata(&marker).is_ok(),
        "the fake command ran at least once"
    );
}
