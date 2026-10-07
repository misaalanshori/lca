//! gh #29 (QA-004), headless rows: an env-configured endpoint host either
//! exits 4 naming the host and the fix, or passes on this run's say-so.
//!
//! The interactive half lives in `tests/regressions/gh29-env-host-grant-
//! prompt.rs`; this file drives the real binary, because the exit code and
//! the stderr wording are the contract a script reads.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;

// Verifies: gh #29's acceptance criterion 1 - `OPENAI_BASE_URL` at an
// ungranted host, run headless, exits 4 (the documented permission-denied
// code) with a message naming the host and the fix, and not one request
// leaves the process.
#[test]
fn an_ungranted_endpoint_host_exits_4_and_names_the_host_and_the_fix() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("never"))]));
    let box_ = sandbox("gh29-headless-ungranted");
    // A grant store with no net pattern: the loopback mock's host is
    // outside the manifest's declared default hosts, so it is ungranted.
    box_.write_grants(false);

    let output = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(
        output.status.code(),
        Some(lca_cli::exit::PERMISSION),
        "stderr: {}",
        stderr(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("127.0.0.1"),
        "the message names the host: {err}"
    );
    assert!(
        err.contains("--allow-host 127.0.0.1"),
        "the message names the flag: {err}"
    );
    assert!(
        err.contains("interactively"),
        "the message names the interactive fix: {err}"
    );
    assert_eq!(
        mock.request_count(),
        0,
        "not one request left the process: {}",
        stderr(&output)
    );
}

// Verifies: gh #29's `--allow-host` row - the same command with the flag
// completes, records a `permission` answer for the session, writes no
// grant, and a fresh run without the flag is back at exit 4.
#[test]
fn allow_host_passes_the_gate_records_once_and_persists_nothing() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text("Hello from mock")),
        Reply::Sse(sse_text("never")),
    ]));
    let box_ = sandbox("gh29-allow-host");
    box_.write_grants(false);

    let output = box_.run(Some(&mock), &["--allow-host", "127.0.0.1", "-p", "hi"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("Hello from mock"),
        "stdout: {}",
        stdout(&output)
    );
    assert_eq!(mock.request_count(), 1, "the turn reached the endpoint");

    let log = find_session_log(&box_.state_dir()).expect("a session log");
    let text = std::fs::read_to_string(log).expect("read log");
    assert!(
        text.contains("\"t\":\"permission\"")
            && text.contains("\"action\":\"connect to 127.0.0.1\"")
            && text.contains("\"decision\":\"once\""),
        "the flag records a once-shaped permission answer: {text}"
    );

    let grants = std::fs::read_to_string(box_.state_dir().join("grants.json")).expect("grants");
    assert!(
        !grants.contains("127.0.0.1"),
        "--allow-host never writes the grant store: {grants}"
    );

    // The process-scoped grant died with the run: without the flag the
    // next run is back at the permission gate.
    let again = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(
        again.status.code(),
        Some(lca_cli::exit::PERMISSION),
        "a run without the flag exits 4 again: {}",
        stderr(&again)
    );
    assert_eq!(
        mock.request_count(),
        1,
        "the second run stopped at the gate too"
    );
}
