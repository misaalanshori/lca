//! The `pty` capability's engine (ADR-0016, capability catalog): a program
//! spawned under a pseudo-terminal sees a real terminal, its output reaches the
//! reader, and what the caller writes reaches the program.
//!
//! Split out of `tools.rs` under the 1,200-line ceiling (gate 11) when gh #535
//! added two rows there. `PtyChild` is public via `lca_tools`, so the split costs
//! nothing but a local `scratch` helper - three lines, duplicated rather than
//! given a shared module of their own, because one helper is not a module.
//!
//! The two readings these rows care about are pinned in three places on purpose:
//! the marker (never a byte count), the exit code, and (for the echo row) that a
//! write reaches the child. What changed in gh #535 is *when* a row reads, never
//! what it asserts.

use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    lca_testkit::scratch_path(name)
}

// The `pty` capability's engine (ADR-0016): a program spawned under a
// pseudo-terminal sees a real terminal and its output reaches the reader.
#[test]
fn pty_spawn_delivers_program_output_and_exit_code() {
    let ws = scratch("pty-spawn");
    let mut pty = if cfg!(unix) {
        lca_tools::PtyChild::spawn("echo", &["hello-pty".into()], &ws, 24, 80, &[]).expect("spawn")
    } else {
        lca_tools::PtyChild::spawn(
            "cmd",
            &["/C".into(), "echo".into(), "hello-pty".into()],
            &ws,
            24,
            80,
            &[],
        )
        .expect("spawn")
    };
    // Gh #535: **wait for the marker, not for a byte count.**
    //
    // This used to stop at `out.len() >= 64`, which is a *sampling* condition: a
    // first read that returned a partial line could satisfy the length without
    // containing `hello-pty`, and a slow runner could spend the whole 5 s
    // deadline. Neither is a property of the product, so the row measured the
    // machine. It now reads until the marker arrives or the session ends, and
    // reports *which* of the two happened, because that difference is the whole
    // reason this row was ever ambiguous: a pty that closed without the marker is
    // a product answer, a deadline that expired is a runner one.
    let started = std::time::Instant::now();
    let deadline = started + std::time::Duration::from_secs(15);
    let mut out = String::new();
    let mut closed = false;
    while !out.contains("hello-pty") {
        if std::time::Instant::now() >= deadline {
            break;
        }
        match pty.read(1024).expect("read") {
            Some(chunk) => out.push_str(&String::from_utf8_lossy(&chunk)),
            None => {
                closed = true;
                break;
            }
        }
    }
    let code = pty.wait().expect("wait");
    assert!(
        out.contains("hello-pty"),
        "{}",
        missing_marker_message(closed, started.elapsed(), &out, code)
    );
    assert_eq!(code, 0, "output was {out:?}");
}

/// The failure message for a pty read that never produced the marker.
///
/// Gh #535: the whole reason this row was ambiguous for two cycles is that its
/// message could not say *which* side failed. Extracted so both branches can be
/// asserted without waiting out a real 15-second deadline.
fn missing_marker_message(
    closed: bool,
    elapsed: std::time::Duration,
    out: &str,
    code: i32,
) -> String {
    format!(
        "the pty {} after {elapsed:?} without the marker; output so far {out:?}, exit code {code}",
        if closed {
            "closed (product: the session ended without delivering it)"
        } else {
            "was still open (runner: the deadline expired before it arrived)"
        }
    )
}

// Verifies: gh #535 - **a failure names which side failed.** A pty that closed
// without the marker is a product answer; a deadline that expired is a runner one.
// Printing the difference is not politeness - it is what makes the next CI failure
// diagnosable in one read instead of a blind re-run, which is how this row sat open.
#[test]
fn the_failure_message_distinguishes_a_closed_pty_from_a_spent_deadline() {
    let closed = missing_marker_message(true, std::time::Duration::from_millis(40), "", 0);
    let expired = missing_marker_message(false, std::time::Duration::from_secs(15), "hel", -1);
    assert!(
        closed.contains("product: the session ended"),
        "the EOF side names the product: {closed}"
    );
    assert!(
        expired.contains("runner: the deadline expired"),
        "the timeout side names the runner: {expired}"
    );
    assert_ne!(closed, expired, "the two messages must differ");
    // And both carry the evidence a reader needs: what arrived, and the exit code.
    for message in [&closed, &expired] {
        assert!(
            message.contains("output so far") && message.contains("exit code"),
            "the message carries the evidence: {message}"
        );
    }
}

// Verifies: gh #535 - **the EOF side happens for real.** A pty whose program exits
// without printing anything closes, and the loop must report that as the session
// ending rather than as a timeout.
#[test]
fn a_pty_that_exits_silently_reports_a_closed_session() {
    let ws = scratch("pty-silent");
    let mut pty = if cfg!(unix) {
        lca_tools::PtyChild::spawn("true", &[], &ws, 24, 80, &[]).expect("spawn")
    } else {
        lca_tools::PtyChild::spawn("cmd", &["/C".into(), "exit".into()], &ws, 24, 80, &[])
            .expect("spawn")
    };
    let started = std::time::Instant::now();
    let deadline = started + std::time::Duration::from_secs(15);
    let mut out = String::new();
    let mut closed = false;
    while !out.contains("hello-pty") {
        if std::time::Instant::now() >= deadline {
            break;
        }
        match pty.read(1024).expect("read") {
            Some(chunk) => out.push_str(&String::from_utf8_lossy(&chunk)),
            None => {
                closed = true;
                break;
            }
        }
    }
    assert!(
        closed,
        "an empty program closes its session: {}",
        missing_marker_message(closed, started.elapsed(), &out, 0)
    );
    let _ = pty.wait();
}

// Interactive echo: what we write reaches the program, what it writes
// reaches us (the keystroke-forwarding contract of the catalog).
#[cfg(unix)]
#[test]
fn pty_forwards_keystrokes_both_ways() {
    let ws = scratch("pty-keys");
    let mut pty = lca_tools::PtyChild::spawn("cat", &[], &ws, 24, 80, &[]).expect("spawn");
    pty.write(b"round-trip\n").expect("write");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut out = String::new();
    while !out.contains("round-trip") && std::time::Instant::now() < deadline {
        if let Some(chunk) = pty.read(1024).expect("read") {
            out.push_str(&String::from_utf8_lossy(&chunk));
        } else {
            break;
        }
    }
    assert!(out.contains("round-trip"), "got {out:?}");
    pty.kill();
}

// ---------------------------------------------------------------------------
// Gh #606: the drain's end-of-stream, instrumented
// ---------------------------------------------------------------------------
//
// `pty_spawn_delivers_terminal_output` (lca-ext-host's conformance row) has now failed
// five times on hosted macOS, every one `exit 0` with an empty payload: the host
// answered end-of-stream before the child's tail was readable. Two facts the drain
// window cannot separate are *(a)* a tail still in flight and *(b)* a master that is
// drained yet stays readable (macOS answers zero bytes forever). On macOS `POLLIN` is
// set in both, so elapsed time is the only separator and nothing recorded **how far**
// it got - which is why all five firings produced the same uninformative line.
//
// These rows pin the instrument, not a new platform behaviour: what can be driven here
// is Linux, where an EOF **is** conclusive, so `a_tail_after_the_child_exits_is_still_read`
// is green **before** and after this cycle. That is stated rather than claimed as
// coverage: it proves the drain phase does not discard a tail that arrives *before* the
// exit is observed, and it cannot prove anything about a macOS-shaped master, which is
// precisely why the diagnostic matters more than the row.

// Verifies: gh #606 - **a finished session's drain reports how far it got.**
//
// The three facts are the ones a sixth firing needs: the attempt count (was the window
// spent, and how far in?), the session's byte total (did anything arrive at all?), and
// whether the kernel named the end (`slave_closed`). Linux reaches this conclusion with
// `POLLHUP`, so `exhausted` is false and `attempts` is small - a macOS firing would
// show `exhausted=true` and `attempts=20`, which is the whole point of the field.
#[cfg(unix)]
#[test]
fn the_drain_reports_how_far_it_got() {
    let ws = scratch("pty-diagnostic");
    let mut pty = lca_tools::PtyChild::spawn(
        "sh",
        &["-c".into(), "printf tail-606".into()],
        &ws,
        24,
        80,
        &[],
    )
    .expect("spawn");
    let mut out = String::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut diagnostic = None;
    while std::time::Instant::now() < deadline {
        match pty.read(4096).expect("read") {
            Some(chunk) => out.push_str(&String::from_utf8_lossy(&chunk)),
            None => {
                diagnostic = pty.drain_diagnostic();
                break;
            }
        }
    }
    let _ = pty.wait();
    let diagnostic = diagnostic.expect(
        "the drain concluded (None) and recorded a diagnostic; without one a firing \
         cannot say how far the window got (gh #606)",
    );
    // The byte count is the session's **total**, which includes what this row read.
    assert!(
        diagnostic.bytes >= out.len() as u64 / 2,
        "the diagnostic counts the bytes this session delivered ({} read as {out:?}): {}",
        out.len(),
        diagnostic.describe()
    );
    assert!(
        out.contains("tail-606"),
        "the tail arrived, so this is the healthy shape: {out:?}"
    );
    // Linux names the end (`POLLHUP`/EIO), so the window is not what decided it.
    assert!(
        diagnostic.slave_closed,
        "on Linux the kernel names the end, which is the fact macOS cannot give: {}",
        diagnostic.describe()
    );
    assert!(
        !diagnostic.exhausted,
        "and the ceiling was therefore not spent: {}",
        diagnostic.describe()
    );
    // The message a row would print, so the shape is asserted and not only the fields.
    let described = diagnostic.describe();
    assert!(
        described.contains("attempts=") && described.contains("bytes="),
        "the description names the two numbers a firing needs: {described}"
    );
}

// Verifies: gh #606 - **a tail written after the child exits is still delivered.**
//
// The five macOS firings' shape: the child's output is *in flight* when `try_wait` first
// reports the exit. Here the child sleeps a moment and then writes, so the ordering the
// drain phase exists for really happens - and the bytes must arrive.
//
// **What this does not prove** (stated, not papered over): macOS's drained-but-readable
// master cannot be reproduced on Linux, where the kernel names the end and the loop exits
// at once. So this row proves the drain phase does not drop a tail it *can* still see, not
// that the macOS window is wide enough - that is what the diagnostic reports, and what a
// future macOS run must be read for.
#[cfg(unix)]
#[test]
fn a_tail_after_the_child_exits_is_still_read() {
    let ws = scratch("pty-late-tail");
    let mut pty = lca_tools::PtyChild::spawn(
        "sh",
        &["-c".into(), "sleep 0.05; printf late-tail-606".into()],
        &ws,
        24,
        80,
        &[],
    )
    .expect("spawn");
    let mut out = String::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !out.contains("late-tail-606") && std::time::Instant::now() < deadline {
        match pty.read(4096).expect("read") {
            Some(chunk) => out.push_str(&String::from_utf8_lossy(&chunk)),
            None => break,
        }
    }
    assert!(
        out.contains("late-tail-606"),
        "the drain phase lost a tail written after the child's exit: {out:?} \
         (diagnostic: {:?})",
        pty.drain_diagnostic().map(|d| d.describe())
    );
    let _ = pty.wait();
}

// Verifies: gh #477's property, at gh #606's new counter: **a finished session does not
// spin, and does not pay the window again on every call.**
//
// The attempt counter is session-scoped, so N calls against an exhausted session cost
// what one call cost. Asserting the counter rather than a stopwatch is the same rule the
// #535 rows use: a wall-clock bound measures the runner, a counter measures the code.
#[cfg(unix)]
#[test]
fn a_drained_master_does_not_spin() {
    let ws = scratch("pty-drained");
    let mut pty = lca_tools::PtyChild::spawn("true", &[], &ws, 24, 80, &[]).expect("spawn");
    // Drain it once.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if std::time::Instant::now() >= deadline {
            panic!("an empty program's session never concluded");
        }
        if pty.read(1024).expect("read").is_none() {
            break;
        }
    }
    let after_first = pty
        .drain_diagnostic()
        .expect("the first None records a diagnostic");
    assert!(
        after_first.attempts <= 20,
        "the drain cannot exceed DRAIN_ATTEMPTS: {}",
        after_first.describe()
    );
    // Twenty more reads: each must answer None at once, and none may add an attempt.
    for _ in 0..20 {
        assert_eq!(
            pty.read(1024).expect("read"),
            None,
            "a finished session keeps answering None"
        );
    }
    let after_twenty = pty.drain_diagnostic().expect("still recorded");
    assert_eq!(
        after_twenty.attempts,
        after_first.attempts,
        "the window is spent once per session, not once per call (gh #477): {} -> {}",
        after_first.describe(),
        after_twenty.describe()
    );
    assert_eq!(
        after_twenty.windows,
        1,
        "and the drain phase ran exactly once (`windows` is the field that catches a \
         per-call reset, which `attempts` cannot where a platform concludes at its first          attempt): {} -> {}",
        after_first.describe(),
        after_twenty.describe()
    );
    let _ = pty.wait();
}
