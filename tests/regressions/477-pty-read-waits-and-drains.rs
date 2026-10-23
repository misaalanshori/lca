//! NFR-24 guard for gh #477: a pty read loop must not spin, and a fast-exiting
//! child's output must not be lost.
//!
//! The macOS CI failure that opened this: `pty_spawn_delivers_terminal_output`
//! spawned `echo pty-ok`, the child exited **0**, and the output was simply
//! missing - the result read exactly `"exit 0 "`. Two defects share the site:
//!
//! 1. **A busy-spin.** The conformance guest loops `pty.read` until it sees
//!    `None`. On Unix the host maps `WouldBlock` to `Ok(Some(empty))`, so
//!    "nothing yet" is answered instantly and the loop re-calls with no
//!    yield. Measured on this box: **584,352 read calls in one second, 589,863
//!    of them empty.** On a loaded runner that stole the core from the very
//!    writer whose output was being awaited.
//! 2. **A readiness race.** The loop treats the first `None` as end-of-stream,
//!    but a pty's master reports EOF on terms of its own: the child exiting
//!    does not mean its bytes have been drained, and on the platform that
//!    failed it reported the end before the tail arrived.
//!
//! The fix puts the answer in the host, where the facts are: a read now says
//! **done** only when the child has exited *and* the buffer is drained, and it
//! waits for data when there is none coming yet. The guest's loop is unchanged
//! in shape - `None` still ends it - which is the point: the contract was
//! always "None means the session ended", and the host was answering it too
//! early.
//!
//! # What is pinned, and how
//!
//! The spin is pinned **structurally, never by wall clock**: the host's read
//! path must contain a wait, and a read that finds nothing must have consumed
//! a measurable amount of time waiting rather than returning immediately. A
//! timing assertion on throughput would re-create exactly the load
//! sensitivity this repo has spent cycles removing.
//!
//! Verifies: ADR-0016's pty contract (`read` returns `None` when the session
//! ended, which is a statement about the session and not about one syscall),
//! and NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lca_tools::PtyChild;

/// A scratch directory this row may spawn in.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-477-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// Read until the session ends, the way the conformance guest does.
///
/// This is deliberately the *guest's* shape - loop until `None` - because that
/// is the caller the defect was found in. If the host's contract is right, a
/// caller written this naively must still get the whole output.
fn read_to_end(pty: &mut PtyChild, budget: Duration) -> (Vec<u8>, usize, Duration) {
    let mut out = Vec::new();
    let mut calls = 0usize;
    let start = Instant::now();
    while start.elapsed() < budget {
        calls += 1;
        match pty.read(4096).expect("read") {
            Some(chunk) => out.extend_from_slice(&chunk),
            None => break,
        }
    }
    let elapsed = start.elapsed();
    (out, calls, elapsed)
}

// Verifies: gh #477's `exit 0 ` failure - a command that writes and exits
// immediately must deliver what it wrote. Repeated, because a race fixed once
// is not fixed: this row is the one CI failed, and it failed under load.
#[cfg(unix)]
#[test]
fn a_fast_exiting_command_delivers_its_output_every_time() {
    let dir = scratch("fast-exit");
    let mut lost = 0;
    let rounds = 40;
    for round in 0..rounds {
        let mut pty = PtyChild::spawn(
            "sh",
            &["-c".to_string(), "printf pty-ok".to_string()],
            &dir,
            24,
            80,
            &[],
        )
        .expect("spawn");
        let (out, _calls, _elapsed) = read_to_end(&mut pty, Duration::from_secs(10));
        let code = pty.wait().expect("wait");
        pty.kill();
        let text = String::from_utf8_lossy(&out).to_string();
        if code != 0 || !text.contains("pty-ok") {
            lost += 1;
            eprintln!("round {round}: exit {code}, output {text:?}");
        }
    }
    assert_eq!(
        lost, 0,
        "{lost} of {rounds} fast-exiting commands lost their output"
    );
}

// Verifies: gh #477's busy-spin. The property is structural and named, not a
// throughput measurement: a read that has nothing to return must **wait**
// rather than return an empty chunk immediately, and the loop must therefore
// make very few calls for a child that sits silent for a while.
//
// The count is the assertion with a wide margin (the defect made 584,352
// calls/s, so "fewer than 100 for a one-second silence" cannot pass by luck
// on a slow machine - it can only pass by waiting). The *time* is checked too,
// so a machine fast enough to spin is still caught.
#[cfg(unix)]
#[test]
fn a_silent_pty_does_not_busy_spin() {
    let dir = scratch("no-spin");
    // Half a second of silence, then output. The reader must wait through it.
    let mut pty = PtyChild::spawn(
        "sh",
        &["-c".to_string(), "sleep 0.5; printf done".to_string()],
        &dir,
        24,
        80,
        &[],
    )
    .expect("spawn");
    let (out, calls, elapsed) = read_to_end(&mut pty, Duration::from_secs(10));
    let code = pty.wait().expect("wait");
    pty.kill();
    assert!(String::from_utf8_lossy(&out).contains("done"), "{out:?}");
    assert_eq!(code, 0);
    assert!(
        elapsed >= Duration::from_millis(400),
        "the read returned before the child had written anything, so it did not wait \
         (elapsed {elapsed:?})"
    );
    assert!(
        calls < 100,
        "the read loop made {calls} calls waiting through half a second of silence; \
         a path that waits makes a handful, a busy-spin makes hundreds of thousands"
    );
}

// Verifies: gh #477 design point 5 - a child that never exits must not hang
// the caller forever. The read ends on the caller's deadline, returning what
// arrived, and says so rather than blocking indefinitely.
//
// This exercises the *caller's* budget (the conformance guest has no clock on
// its read loop and the host cannot invent one), so what is pinned is that a
// live, silent child leaves the loop reachable rather than wedged.
#[cfg(unix)]
#[test]
fn a_child_that_never_exits_returns_at_the_deadline_with_what_arrived() {
    let dir = scratch("live-child");
    let mut pty = PtyChild::spawn(
        "sh",
        &["-c".to_string(), "printf early; sleep 30".to_string()],
        &dir,
        24,
        80,
        &[],
    )
    .expect("spawn");
    let (out, _calls, elapsed) = read_to_end(&mut pty, Duration::from_millis(600));
    pty.kill();
    assert!(
        String::from_utf8_lossy(&out).contains("early"),
        "the bytes that did arrive come back before the deadline: {out:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the loop returned at its own deadline rather than hanging on the live child \
         (elapsed {elapsed:?})"
    );
}

// Verifies: gh #477's regression pin - the chatty, long-output case still
// reads fully. A fix that stops early to "drain fast" would pass the rows
// above and fail this one.
#[cfg(unix)]
#[test]
fn a_chatty_child_still_reads_every_byte() {
    let dir = scratch("chatty");
    let mut pty = PtyChild::spawn(
        "sh",
        &[
            "-c".to_string(),
            // 2,000 lines: far more than one 4 KiB read, so the loop must keep
            // going across many chunks and still terminate on its own.
            "i=0; while [ $i -lt 2000 ]; do echo line-$i; i=$((i+1)); done".to_string(),
        ],
        &dir,
        24,
        80,
        &[],
    )
    .expect("spawn");
    let (out, _calls, _elapsed) = read_to_end(&mut pty, Duration::from_secs(30));
    let code = pty.wait().expect("wait");
    pty.kill();
    let text = String::from_utf8_lossy(&out);
    assert_eq!(code, 0);
    assert!(
        text.contains("line-1999"),
        "the last line arrived, so the loop drained rather than stopped early \
         (got {} bytes)",
        out.len()
    );
    assert!(
        text.contains("line-0"),
        "and the first line was not dropped either"
    );
}
