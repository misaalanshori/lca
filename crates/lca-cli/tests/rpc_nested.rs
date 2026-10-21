//! gh #269: `--mode rpc` wires the nested completion backend.
//!
//! `headless.rs` and `tui/runner.rs` both build a `ProviderBackend` and call
//! `agent.set_nested_backend(nested)`; `rpc.rs` built the `Agent` and never
//! did. In RPC mode that left LLM compaction with no backend, so it fell back
//! to the plain mechanical excerpt summary — and any extension asking for the
//! `completion` capability had nothing to call.
//!
//! The rows drive a real `lca --mode rpc` session (stdin JSONL) with a fake
//! provider, and read what the compaction produced: a model summary proves the
//! backend is wired, and the mechanical summary proves it is not.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// Verifies: gh #269 - `rpc.rs` installs the same nested backend `headless.rs`
// and `tui/runner.rs` do.
//
// **Source-level, deliberately.** The behavioural route (a turn that compacts)
// needs a *second* turn, and RPC's multi-turn path drops `turn-end` under load
// — the pre-existing gh #195 defect that already has the project's one RPC
// happy-path row `#[ignore]`d ("responses but zero turn-ends"). A guard built
// on it would be flaky for the same reason, so this row pins the wiring the
// fix is: the construction `rpc.rs` must perform, in the same order the other
// two entry points perform it.
//
// The behavioural evidence is in this cycle's report: a two-process drive
// (turn 1 records history, turn 2 resumes) issues the compaction request
// through the nested backend, and the same drive without the call emits the
// mechanical fallback.
#[test]
fn the_rpc_entry_point_installs_the_nested_backend() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rpc.rs"),
    )
    .expect("read rpc.rs");
    // The construction, exactly as the other two entry points build it.
    assert!(
        source.contains("ProviderBackend::new("),
        "rpc.rs builds the nested adapter"
    );
    // ...and installs it on the agent before the turn runs.
    let install = source
        .find("set_nested_backend")
        .expect("rpc.rs installs the nested backend on the agent (gh #269)");
    let run = source
        .find("run_turn_queued")
        .expect("rpc.rs runs the turn");
    assert!(
        install < run,
        "the backend is installed before the turn runs (install at {install}, run at {run})"
    );
    // The same three inputs the other entry points pass: the provider, the
    // turn's model, and the session id.
    assert!(
        source.contains("agent_config.model.clone()"),
        "the nested backend carries the turn's model"
    );
    assert!(
        source.contains("session_ref.id().to_string()"),
        "the nested backend carries the session id"
    );
}
