//! gh #476 - a capability call from inside a runtime must not kill the process.
//!
//! `/mcp` → Sign in died, deterministically, taking the session with it:
//!
//! ```text
//! location: crates/lca-tools/src/capabilities/mod.rs:416:34
//! message: Cannot start a runtime from within a runtime.
//!   <Capabilities>::oauth_flow_net_request
//!   lca_mcp::remote::remember_unauthorized
//!   <McpManager>::begin_interactive_sign_in
//!   lca_cli::tui::Ui::hooks::{closure#23}
//!   lca_ui::chat::Chat::open_mcp_signin
//! ```
//!
//! # The seam, not the caller
//!
//! `Capabilities::drive` read the ambient runtime handle and called `block_on` on
//! it. That is legal from a worker thread and **panics** from inside a runtime
//! *poll* - and `Handle::try_current()` succeeds in both situations, so the old
//! code could not tell them apart. It documented the gap as an assumption
//! ("every current caller is inside a blocking region") rather than checking it,
//! and a TUI callback broke the assumption: `main` holds `Runtime::block_on` on a
//! multi-threaded runtime, so every UI closure runs inside that poll.
//!
//! A capability function that panics for one caller is a trap for every later one,
//! and `oauth_flow_net_request` is not the only method that blocks - `net_request`,
//! the `process` spawn path and the body readers all go through the same `drive`.
//! So the fix is at the seam, once, for the whole layer.
//!
//! # The shape
//!
//! `tokio::task::block_in_place` re-enters a blocking region on a **multi-threaded**
//! runtime, after which `block_on` is legal - tokio's own documented pairing for
//! this exact problem, and it works from a poll *and* from a worker thread, so
//! there is no case left to tell apart. Staying on the **ambient** runtime is what
//! makes it correct rather than merely non-panicking: a connection's background
//! task lives there, and handing the future to a different runtime would orphan it
//! - the bug the shared-runtime path exists for.
//!
//! Three things this file pins, because each is easy to undo:
//!
//! 1. **The seam is the only `block_on`.** A new `Handle::block_on` anywhere else
//!    in the capability layer reintroduces the trap for that method.
//! 2. **`block_in_place` is used, not a second runtime.** Handing the work to
//!    another runtime would return a value while breaking long-lived connections.
//! 3. **The shape is guarded in a real runtime.** The behaviour rows live in
//!    `crates/lca-tools/tests/network.rs`; this file is the structural half.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// The body of `fn drive`.
fn drive_body() -> String {
    let source = read("crates/lca-tools/src/capabilities/mod.rs");
    let start = source
        .find("pub(super) fn drive<")
        .expect("the drive seam exists");
    let rest = &source[start..];
    rest[..rest.find("\n    }\n").expect("drive has a body")].to_string()
}

// Verifies: gh #476 - **one `block_on` for the layer, and it goes through
// `block_in_place`.**
//
// A future edit that "simplifies" this back to `handle.block_on(future)` restores
// the exact panic; an edit that hands the future to another runtime fixes the
// panic but orphans long-lived connections, which is a silent, worse bug.
#[test]
fn the_seam_reenters_a_blocking_region_instead_of_panicking() {
    let body = drive_body();
    assert!(
        body.contains("block_in_place"),
        "the multi-threaded path re-enters a blocking region, which is what makes \
         `block_on` legal from a poll: {body}"
    );
    assert!(
        body.contains("handle.block_on(future)"),
        "and it blocks on the *ambient* handle, so the connection's background \
         task stays on the same runtime: {body}"
    );
}

// Verifies: gh #476 - **no other `block_on` in the capability layer.**
//
// `oauth_flow_net_request` is not special: `net_request`, the body reads and the
// `process` spawn path all funnel through `drive`. A new `Handle::block_on` added
// beside one of them would be the same trap for that method, and it would not
// show up in the row that covers this one.
#[test]
fn every_block_on_is_a_named_path_in_one_place() {
    let source = read("crates/lca-tools/src/capabilities/mod.rs");
    // Count real calls: `block_on(` with a `handle.` or `runtime.` receiver, and
    // skip doc-comment lines.
    let calls: Vec<&str> = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| line.contains(".block_on(") || line.contains("::block_on("))
        .collect();
    assert!(
        calls.len() >= 2 && calls.len() <= 4,
        "every `block_on` sits in `drive`/`shared_drive` - the multi-threaded \
         branch, the current-thread branch, and the no-runtime path: {calls:#?}"
    );
    // And `net.rs` — the file with the most capability I/O — has none of its own.
    let net = read("crates/lca-tools/src/capabilities/net.rs");
    assert!(
        !net.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .any(|l| l.contains(".block_on(")),
        "every network call drives through the seam"
    );
}

// Verifies: gh #476 - **the flavour is checked, not guessed.**
//
// `block_in_place` closes the poll case on a multi-threaded runtime, and it
// **panics** on a current-thread one - where the plain `block_on` is exactly
// right, because that runtime's only thread is running the frame (not a poll).
// `tui/runner.rs` drives each turn on a current-thread runtime, so both branches
// are live paths. Applying `block_in_place` unconditionally broke two
// `e2e_model_consent`/`e2e_color` rows, which is why the gate is here.
#[test]
fn the_flavour_decides_which_affordance_is_used() {
    let body = drive_body();
    assert!(
        body.contains("RuntimeFlavor::MultiThread") && body.contains("block_in_place"),
        "the multi-threaded branch re-enters a blocking region: {body}"
    );
    // And the current-thread branch still blocks plainly, rather than being
    // routed to `block_in_place` (which panics) or to the shared runtime (which
    // would recurse, since entering it makes that runtime ambient).
    let after_gate = body
        .split("RuntimeFlavor::MultiThread")
        .nth(1)
        .unwrap_or_default();
    assert!(
        after_gate.contains("handle.block_on(future)"),
        "the current-thread branch uses the plain `block_on`, legal in a frame: \
         {after_gate}"
    );
}

// Verifies: gh #476 design point 5 - **the sweep, recorded.**
//
// Every other runtime built or blocked on in this workspace, and why it is not
// this bug:
///
/// - `lca_core::drive_blocking` - already the correct shape for this problem (a
///   dedicated thread with its own current-thread runtime), and it is what the
///   compaction and `/usage` callers use from the interface thread.
/// - `lca_core::ext_provider::complete` - has the *same* `handle.block_on`
///   assumption, but its only caller is compaction, which runs on the turn's own
///   worker thread (`tui/runner.rs` spawns `turn_worker`), so there is no poll to
///   block. Noted rather than changed: it is outside this brief's boundary, and
///   the pattern is worth fixing with the same one-liner if a caller ever moves.
/// - `lca-cli/src/tui/hub.rs::install_row` and `tui/runner.rs` - both build a
///   runtime on a **spawned** thread (`spawn_install`, `turn_worker`), so they are
///   outside any poll by construction.
/// - `lca-cli/src/main.rs` - the one place a runtime is *entered*, which is why
///   the TUI's callbacks are inside a poll at all.
///
/// The row asserts the two TUI runtime-builders really are on spawned threads,
/// because that is the property the sweep rests on and the one a refactor could
/// quietly break.
#[test]
fn the_tui_runtime_builders_run_on_spawned_threads() {
    let hub = read("crates/lca-cli/src/tui/hub.rs");
    let install = &hub[hub
        .find("pub fn spawn_install(")
        .expect("spawn_install exists")..];
    let install = &install[..install.find("\n}\n").unwrap_or(install.len())];
    assert!(
        install.contains("std::thread::spawn"),
        "install_row builds a runtime, so it must be inside a spawned thread: \
         {install}"
    );

    let runner = read("crates/lca-cli/src/tui/runner.rs");
    assert!(
        runner.contains("std::thread::spawn(move || turn_worker"),
        "the turn worker (which builds a runtime) is spawned, not polled"
    );
}

// Verifies: gh #476 - no new dependency; the fix uses tokio's own primitives.
#[test]
fn no_new_dependency_was_added() {
    let manifest = read("crates/lca-tools/Cargo.toml");
    for unwanted in ["threadpool", "rayon", "crossbeam"] {
        assert!(
            !manifest.contains(&format!("\n{unwanted} =")),
            "`block_in_place` is in tokio already; no {unwanted} for it"
        );
    }
}
