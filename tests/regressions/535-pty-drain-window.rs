//! NFR-24 guard for gh #535: **the pty's post-exit drain must give a late tail
//! real time, and must not become gh #477's spin.**
//!
//! ## The defect the row was hiding
//!
//! `pty_spawn_delivers_terminal_output` (`lca-ext-host::capabilities`) is not a
//! sampling loop: the conformance guest's `pty` mode loops `pty.read` until the
//! host answers end-of-stream, and *then* reports. So when it failed on macOS CI it
//! was not reading early - the **host had declared the session drained while the
//! child's `pty-ok` was still in flight**:
//!
//! ```text
//! thread 'pty_spawn_delivers_terminal_output' panicked at capabilities.rs:421:
//! exit 0
//! ```
//!
//! Twice in a fortnight (runs 42069077201 and 42166657159), both inside a 3.5-4.7 s
//! run, both on the same quiet slice of the suite. gh #477 had already found this
//! shape and had answered it with **one** 4 ms window after a confirmed child exit -
//! a guess, and on macOS a *drained* master stays permanently readable (a read
//! answers zero bytes), so one 4 ms step is all the platform was ever given.
//!
//! ## What this pins
//!
//! 1. **A bounded drain phase with real elapsed time**, not a single sample: an
//!    attempt count times a step, so the ceiling is arithmetic rather than
//!    optimism (20 x 5 ms = 100 ms).
//! 2. **Every attempt waits.** gh #477's spin was a branch that re-called with no
//!    pause at all (measured 614,104 calls/second). The guard is that the drain
//!    branch contains a sleep, so it cannot be reached in a loop without time
//!    passing.
//! 3. **The platform's own answer is still respected**: where the kernel can say
//!    "nothing more is coming" (Linux reports `POLLHUP` and never `POLLIN` on a
//!    drained master), the phase exits at once instead of sleeping out its ceiling.
//!
//! The behavioural halves live beside the code they drive: the rows in
//! `crates/lca-tools/tests/tools.rs` (marker, not a byte count) and
//! `crates/lca-ext-host/tests/capabilities.rs` (the finished result, with a message
//! that says which world failed), plus `tests/regressions/477-pty-read-waits-and-drains.rs`
//! for the spin and the fast-exit delivery.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code.

use std::path::Path;

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|err| panic!("read {rel}: {err}"))
}

/// The unix `read_impl`, which is where the drain phase lives.
fn read_impl() -> String {
    let source = read("crates/lca-tools/src/pty.rs");
    let start = source
        .find("fn read_impl(")
        .expect("the unix read_impl exists");
    let rest = &source[start..];
    // The function ends at the next top-level `}` after the loop.
    let end = rest.find("\n}\n").unwrap_or(rest.len());
    rest[..end].to_string()
}

// Verifies: gh #535 - **the drain phase is bounded by a count, not by hope.**
//
// A single window is what #477 shipped and what macOS CI defeated twice. A bound
// that is a *number of attempts* can be checked against the step and stated as a
// ceiling; a single `const DRAIN_GRACE: 4ms` cannot be reasoned about at all.
#[test]
fn the_drain_phase_is_a_bounded_number_of_attempts() {
    let source = read("crates/lca-tools/src/pty.rs");
    for fact in [
        "const DRAIN_STEP: std::time::Duration",
        "const DRAIN_ATTEMPTS: u32",
    ] {
        assert!(source.contains(fact), "the drain phase names {fact}");
    }
    let body = read_impl();
    assert!(
        body.contains("drain_attempt >= DRAIN_ATTEMPTS"),
        "the loop stops at that count rather than at a single sample: {body}"
    );
    assert!(
        body.contains("drain_attempt += 1"),
        "and each attempt is counted, so the ceiling is arithmetic: {body}"
    );
    // The old single-shot flag must be gone: it is what made the window 4 ms wide.
    assert!(
        !body.contains("drain_retry_used"),
        "the one-shot grace is replaced, not merely widened: {body}"
    );
}

// Verifies: gh #535's other half - **every drain attempt waits.**
//
// This is the difference between #477's fix and #477's bug. The spin was a branch
// that re-called `read` immediately (614,104 calls/second measured), so the guard is
// structural: the drain branch must spend time, and it must spend it *before* it
// re-reads. A future "simplification" that drops the sleep turns this row red rather
// than turning CI into a busy loop.
#[test]
fn every_drain_attempt_waits_before_it_rereads() {
    let body = read_impl();
    let drain = body
        .split("drain_attempt >= DRAIN_ATTEMPTS")
        .nth(1)
        .unwrap_or_else(|| panic!("the drain branch exists: {body}"));
    let drain = drain.split("continue;").next().unwrap_or(drain);
    assert!(
        drain.contains("std::thread::sleep(DRAIN_STEP)"),
        "the drain branch sleeps, so it cannot spin: {drain}"
    );
    // And the sleep is not conditional on data arriving (that is #477's spin shape,
    // where a readable-but-empty master kept the loop hot).
    assert!(
        drain.contains("wait_readable(&inner.master, 0)?"),
        "and it probes with a zero timeout, so the sleep is the only source of \\
         elapsed time: {drain}"
    );
}

// Verifies: gh #535 - **the platforms still differ where they can.** Where the
// kernel can answer "nothing more is coming", the phase ends immediately: Linux sets
// `POLLHUP` and never `POLLIN` on a drained master, so the probe answers `false` and
// the loop returns without spending the ceiling. Without this branch every pty read
// on every platform would cost 100 ms at shutdown.
#[test]
fn a_conclusive_end_is_taken_immediately() {
    let body = read_impl();
    assert!(
        body.contains("return Ok(None);"),
        "a conclusive end returns without waiting out the ceiling: {body}"
    );
    // The `else` arm after the readable probe is that early exit.
    let probe = body
        .split("wait_readable(&inner.master, 0)?")
        .nth(1)
        .unwrap_or_else(|| panic!("the zero-timeout probe exists: {body}"));
    let immediate = probe.split("continue;").next().unwrap_or(probe);
    assert!(
        immediate.contains("return Ok(None)"),
        "the answer 'nothing more is coming' is not slept on: {immediate}"
    );
}
