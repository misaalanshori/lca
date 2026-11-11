//! Gh #593: the notice routing `raw_mode_error` into the interface, and the
//! `TurnState` split that gh #575's ceiling needed.
//!
//! The behavioural row for the notice itself lives in
//! `run_terminal_cooked_tests.rs` (the function and its three inputs). What
//! cannot be seen from there is that **the loop actually calls it** - the
//! defect gh #323 left behind was precisely a value nobody consumed, so a row
//! that only drives the helper would pass on the shipped tree that had no
//! reader at all.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

/// The production code of one file, without its `#[cfg(test)]` blocks.
///
/// `module_text` (the workspace walk the regression crate uses) is not
/// available inside `lca-ui`, so this reads the file directly and cuts at the
/// first `#[cfg(test)]` - which is also the stricter assertion for this row:
/// the *barrel* itself must call the helper, not a sibling.
fn run_source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/run.rs");
    let source = std::fs::read_to_string(&path).expect("run.rs is readable");
    match source.find("\n#[cfg(test)]") {
        Some(at) => source[..at].to_string(),
        None => source,
    }
}

/// The production code of `run_turns.rs`.
fn turns_source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/run_turns.rs");
    std::fs::read_to_string(&path).expect("run_turns.rs is readable")
}

// Verifies: gh #593 - **the loop asks.** A helper with no caller is the defect
// itself: `Terminal::raw_mode_error` existed, documented, and read by nobody
// from gh #323 until this cycle.
#[test]
fn the_loop_reads_the_raw_mode_reason_it_was_given() {
    let source = run_source();
    let call = source
        .find("cooked_input_notice(&terminal)")
        .expect("run asks the terminal about raw mode");
    // It must be the *notice* path, not a dropped value: the answer goes into a
    // `chat.say`, which is the one startup-notice surface.
    let around = &source[call..];
    let end = around
        .find('\n')
        .map(|at| call + at)
        .unwrap_or(source.len() - 1);
    let line = &source[source[..call].rfind('\n').unwrap_or(0)..end];
    assert!(
        line.contains("if let Some(notice)")
            || source[..call].rfind("if let Some(notice)").is_some(),
        "the reason is consumed as a notice, not discarded: {line}"
    );
    let after = &source[call..];
    assert!(
        after[..after.find("\n\n").unwrap_or(after.len())].contains("chat.say("),
        "and the notice reaches the user through the startup notice channel"
    );
}

// Verifies: gh #575 - `TurnState` moved to `run_turns.rs`, and **every path a
// caller names is unchanged**: `run.rs` re-exported it, so `crate::run::TurnState`
// still resolves, and an inherent `impl` in a sibling file is legal Rust.
#[test]
fn the_turn_state_split_moved_code_not_a_path() {
    let run = run_source();
    assert!(
        run.contains("pub(crate) use crate::run_turns::TurnState;"),
        "run.rs re-exports the type it used to define"
    );
    assert!(
        !run.contains("pub(crate) struct TurnState {"),
        "and no longer defines a second copy"
    );
    let turns = turns_source();
    assert!(
        turns.contains("pub(crate) struct TurnState {") && turns.contains("impl TurnState {"),
        "the definition and its transitions live in run_turns.rs"
    );
    // The three rows' entry points are public within the crate, or the loop and
    // the tests could not call them from outside the module.
    for entry in [
        "pub(crate) fn new(",
        "pub(crate) fn reap(",
        "pub(crate) fn start(",
    ] {
        assert!(
            turns.contains(entry),
            "`{entry}` keeps its reach: a sibling calls it"
        );
    }
}
