//! Gh #426 (NFR-24): `docs/headless.md` documented `queue-queued` with a
//! `mode` field and nothing emitted it.
//!
//! A script driving `lca --mode json`/`--mode rpc` saw the drain
//! (`queue-flushed`) but never the accept, so "queued and later injected"
//! and "never queued at all" were indistinguishable - which is the one thing
//! the steering queue exists to distinguish during a headless run.
//!
//! The guard had disclosed the gap rather than fixed it: `queue-queued` sat
//! in the drift test's `PRODUCED_ELSEWHERE` list. The proof that doc and
//! code agree again is that the exception is gone and the guard is green,
//! so these rows pin exactly that - the exception list, the emitter, and the
//! one-vocabulary rule across both delivery modes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failing assertion.

fn headless_source() -> &'static str {
    include_str!("../../crates/lca-cli/src/headless.rs")
}

fn rpc_source() -> &'static str {
    include_str!("../../crates/lca-cli/src/rpc.rs")
}

fn doc() -> &'static str {
    include_str!("../../docs/headless.md")
}

/// The drift guard's exception list no longer carries `queue-queued`, and it
/// still carries the one kind that genuinely lives outside the sink. This is
/// the acceptance proof the manager pinned: fix the code, never narrow the
/// guard.
#[test]
fn the_drift_guard_no_longer_excuses_queue_queued() {
    let tests = include_str!("../../crates/lca-cli/src/headless_tests.rs");
    let line = tests
        .lines()
        .find(|line| line.contains("const PRODUCED_ELSEWHERE"))
        .expect("the drift guard's exception list exists");
    assert!(
        !line.contains("queue-queued"),
        "`queue-queued` must not be excused any more: {line}"
    );
    assert!(
        line.contains("session-start"),
        "`session-start` legitimately opens the stream outside a turn: {line}"
    );
}

/// The emitter exists once, on the sink both modes share, and it is the
/// documented shape: `type` plus `mode`, no text (the drain carries the
/// text).
#[test]
fn one_emitter_serves_both_modes() {
    let headless = headless_source();
    assert_eq!(
        headless.matches("pub fn emit_queue_queued").count(),
        1,
        "one emitter, on the shared sink"
    );
    assert!(
        headless.contains("\"type\": \"queue-queued\""),
        "the documented type string"
    );
    assert!(
        headless.contains(r#""mode": mode.marker()"#),
        "`mode` comes from the ADR-0038 SubmitMode, never a literal default"
    );
    // Both call sites drive that one emitter; neither forks vocabulary.
    let rpc = rpc_source();
    assert!(
        rpc.contains("emit_queue_queued"),
        "RPC emits the accept through the shared sink"
    );
    assert!(
        !rpc.contains("\"type\": \"queue-queued\""),
        "RPC must not spell the record itself"
    );
}

/// The doc row no longer claims the gap, and the pi-shape mapping keeps
/// pointing at the two kinds.
#[test]
fn the_doc_row_states_the_field_honestly() {
    let doc = doc();
    let row = doc
        .lines()
        .find(|line| line.trim_start().starts_with("| `queue-queued`"))
        .expect("the queue-queued row exists");
    assert!(
        !row.contains("not yet emitted"),
        "the doc must not still disclose the gap: {row}"
    );
    assert!(!row.contains("gh #264"), "the gap note is stale: {row}");
    assert!(row.contains("gh #426"), "the row names the fix: {row}");
}

/// The accept precedes the drain in the only code path that can produce
/// both: RPC's `enqueue!` emits before anything can drain, and the follow-up
/// start happens after the run ends.
#[test]
fn both_halves_go_through_one_sink_in_order() {
    let rpc = rpc_source();
    // The accept is written where the message is accepted...
    let enqueue = rpc
        .find("macro_rules! enqueue")
        .expect("the enqueue macro exists");
    let macro_body = &rpc[enqueue..];
    let emit = macro_body
        .find("emit_queue_queued")
        .expect("the macro emits the accept");
    let push = macro_body
        .find(".push(")
        .expect("the macro queues the message");
    assert!(
        emit > push,
        "the accept is emitted after the queue actually took the message \
         (so it can never announce an accept that did not happen)"
    );
    // ...and the drain comes from the turn loop, which runs later.
    assert!(
        rpc.contains("run_turn_queued"),
        "the drain rides the turn, which starts after the accept"
    );
}
