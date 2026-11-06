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
