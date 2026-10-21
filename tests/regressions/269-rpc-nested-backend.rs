//! gh #269 (NFR-24): `--mode rpc` installs the nested completion backend.
//!
//! `headless.rs` and `tui/runner.rs` both build a `ProviderBackend` and call
//! `agent.set_nested_backend(nested)`; `rpc.rs` built the `Agent` and never
//! did. In RPC mode that left LLM compaction with no backend, so it silently
//! degraded to the mechanical excerpt summary — and any extension asking for
//! the `completion` capability had nothing to call, in the entry point a
//! daemon client uses.
//!
//! The guard is source-level on purpose. The behavioural route (a turn that
//! compacts) needs a *second* turn, and RPC's multi-turn path drops `turn-end`
//! under load — the pre-existing gh #195 defect that already has the project's
//! one RPC happy-path row `#[ignore]`d. A guard built on it would inherit that
//! flake, which is worse than a precise structural assertion: this row pins
//! exactly the call the bug omitted, in the order the two working entry points
//! make it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// Verifies: gh #269 - the RPC entry point constructs and installs the same
// nested backend the other two front ends do, before it runs the turn.
#[test]
fn the_rpc_entry_point_wires_the_nested_completion_backend() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-cli/src/rpc.rs"),
    )
    .expect("read rpc.rs");

    let build = source
        .find("ProviderBackend::new(")
        .expect("rpc.rs builds the nested adapter (gh #269)");
    let install = source
        .find("set_nested_backend")
        .expect("rpc.rs installs it on the agent (gh #269)");
    let run = source
        .find("run_turn_queued")
        .expect("rpc.rs runs the turn");

    assert!(
        build < install,
        "the adapter is built before it is installed ({build} < {install})"
    );
    assert!(
        install < run,
        "the backend is installed before the turn runs ({install} < {run})"
    );
    // The three inputs the other entry points pass.
    for needle in ["agent_config.model.clone()", "session_ref.id().to_string()"] {
        assert!(
            source.contains(needle),
            "the nested backend carries `{needle}` like the other entry points"
        );
    }
}

// Verifies: gh #269 - the two entry points that already worked still do, so a
// refactor cannot fix RPC by breaking them.
#[test]
fn the_other_two_entry_points_still_install_it() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in [
        "crates/lca-cli/src/headless.rs",
        "crates/lca-cli/src/tui/runner.rs",
    ] {
        let source = std::fs::read_to_string(root.join(file)).expect("read entry point");
        assert!(
            source.contains("set_nested_backend"),
            "{file} installs the nested backend"
        );
    }
}
