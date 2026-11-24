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

#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use lca_tools::PtyChild;

/// A scratch directory this row may spawn in.
#[cfg(unix)]
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-477-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// What one read-to-end run measured, per call as well as in total.
///
/// Gh #612: the cycle that restated the no-spin row needed the *distribution*,
/// not the rate, because the two readings of a fast loop ("the box is loaded" and
/// "the loop is spinning") differ in the shape of the calls, not in their count:
/// a waiting call answers empty after its readiness window (8 ms, more under
/// load), a spinning call answers empty in microseconds. So the run records how
/// many empty answers came back **too fast to have waited**, and that counter -
/// not the rate - is the row's primary assertion.
#[cfg(unix)]
struct Reading {
    out: Vec<u8>,
    /// Every `read` the guest-shaped loop made.
    calls: usize,
    /// Answers that carried no bytes (`Some(empty)`), i.e. "nothing yet".
    empty: usize,
    /// ...of those, the ones that returned in under a millisecond. A call that
    /// waited cannot land here: the shortest wait is a poll with a millisecond
    /// timeout.
    fast_empty: usize,
    /// The slowest empty answer, so a failure can show how far apart the two
    /// shapes were.
    empty_max: Duration,
    elapsed: Duration,
    /// The session's drain facts, when the loop reached the end (gh #606).
    drain: Option<lca_tools::DrainDiagnostic>,
}

#[cfg(unix)]
impl Reading {
    /// One line a future red run can be read from, without re-instrumenting.
    fn describe(&self) -> String {
        let per_second = self.calls as f64 / self.elapsed.as_secs_f64();
        format!(
            "{} calls in {:?} ({per_second:.0} calls/s): {} empty answers, {} of them \
             faster than 1 ms (the shortest waited {:?}); drain was {:?}",
            self.calls, self.elapsed, self.empty, self.fast_empty, self.empty_max, self.drain
        )
    }
}

/// Read until the session ends, the way the conformance guest does.
///
/// This is deliberately the *guest's* shape - loop until `None` - because that
/// is the caller the defect was found in. If the host's contract is right, a
/// caller written this naively must still get the whole output.
#[cfg(unix)]
fn read_to_end(pty: &mut PtyChild, budget: Duration) -> Reading {
    let mut out = Vec::new();
    let mut calls = 0usize;
    let mut empty = 0usize;
    let mut fast_empty = 0usize;
    let mut empty_max = Duration::ZERO;
    let start = Instant::now();
    while start.elapsed() < budget {
        calls += 1;
        let call_started = Instant::now();
        match pty.read(4096).expect("read") {
            Some(chunk) => {
                if chunk.is_empty() {
                    empty += 1;
                    let cost = call_started.elapsed();
                    empty_max = empty_max.max(cost);
                    if cost < Duration::from_millis(1) {
                        fast_empty += 1;
                    }
                } else {
                    out.extend_from_slice(&chunk);
                }
            }
            None => break,
        }
    }
    let elapsed = start.elapsed();
    let drain = pty.drain_diagnostic();
    Reading {
        out,
        calls,
        empty,
        fast_empty,
        empty_max,
        elapsed,
        drain,
    }
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
        let out = read_to_end(&mut pty, Duration::from_secs(10)).out;
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
// rather than return an empty chunk immediately.
//
// **Gh #612 restated which assertion carries the claim.** The rate was the
// primary one from gh #477 to #612, and it went red once in a full workspace run:
// 1,594 calls in 635 ms (2,508/s) against a 2,000/s bound, on a box carrying
// every suite at once. That was not the box lying - it was a **real spin**, and
// the distribution said so before the mechanism did: 214 of the empty answers
// came back in **4-18 microseconds**, where a call that waits cannot come back in
// less than its poll's millisecond timeout. The product path was `EIO` from the
// master (the kernel naming the end) while the child had not been reaped yet:
// `poll` answered `POLLHUP` with no `POLLIN`, `read_upstream` had nothing, and the
// loop returned "nothing yet" at syscall speed - 203 such polls in one 500 ms
// window. Fixed in `crates/lca-tools/src/pty.rs`: a conclusive end now enters the
// drain window instead of waiting for `try_wait`, so the call costs one window
// and the session ends.
//
// So the row asserts the **shape of the calls**, which load cannot fake: no empty
// answer may return in under a millisecond, because the only way to answer empty
// is to have waited out a poll window. A loaded box makes a waiter *slower*
// (measured: 125/s idle, 38-110/s under twelve CPU hogs); only a spin makes it
// faster, and a spin is exactly what this row exists for. The rate is kept as a
// coarse second opinion with the measured numbers on both sides named.
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
    let reading = read_to_end(&mut pty, Duration::from_secs(30));
    let code = pty.wait().expect("wait");
    pty.kill();
    assert!(
        String::from_utf8_lossy(&reading.out).contains("done"),
        "{:?}",
        reading.out
    );
    assert_eq!(code, 0);
    assert!(
        reading.elapsed >= Duration::from_millis(400),
        "the read returned before the child had written anything, so it did not wait \
         (elapsed {:?})",
        reading.elapsed
    );
    // The primary assertion: a call that waits takes at least its poll timeout,
    // so an empty answer in under a millisecond is a call that did not wait.
    assert_eq!(
        reading.fast_empty,
        0,
        "the read loop answered \"nothing yet\" without waiting (gh #612's spin shape); {}",
        reading.describe()
    );
    // The coarse one, kept for the numbers it names: a waiter is ~125/s idle and
    // ~40-110/s under twelve hogs, the pre-#477 spin was 614,104/s, and the #612
    // spin measured 2,508-2,989/s. Load only ever moves this down.
    let per_second = reading.calls as f64 / reading.elapsed.as_secs_f64();
    assert!(
        per_second < 2_000.0,
        "the read loop ran at {per_second:.0} calls/s, which a waiting read cannot do \
         (measured: ~125/s idle, 38-110/s under load); {}",
        reading.describe()
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
    let reading = read_to_end(&mut pty, Duration::from_millis(600));
    let (out, elapsed) = (reading.out, reading.elapsed);
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
    let out = read_to_end(&mut pty, Duration::from_secs(30)).out;
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
