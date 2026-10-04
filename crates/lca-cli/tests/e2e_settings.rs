//! Real-pane regression row for issue #20 / V1: `/settings` during a
//! running turn must respond on the input thread instead of queueing
//! behind the turn's `tools` lock - the freeze the owner hit, where the
//! UI stopped repainting and Ctrl+C could not cancel either. Since gh #30
//! `/settings` opens the interactive selector, so the row waits for the
//! selector's own header instead of the dump's notice line; the dump
//! lives on as `lca config`.
//!
//! The harness (private socket, `Drop`) is `tests/common`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

// The rows are Unix-only (tmux), so the glob import is too.
#[cfg(unix)]
use common::*;

/// V1 (issue #20): the owner's freeze sequence, driven deterministically
/// with a mock provider - a slow tool call holds the turn (and its
/// `tools` lock) while `/settings` is submitted mid-turn. The widget must
/// appear while the turn is still running, and Ctrl+C must still cancel
/// it. Pre-fix both waited for the turn to end: the input thread was the
/// one blocked.
#[cfg(unix)]
#[test]
fn settings_during_a_running_turn_responds_and_ctrl_c_still_cancels() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        // A slow command: the turn holds `tools` for these eight seconds,
        // which is the window the owner's `/settings` landed in.
        Reply::Sse(sse_tool_call(
            "shell",
            r#"{"command":"sleep 8 && echo long-done"}"#,
        )),
        Reply::Sse(sse_text_with_usage("turn finished", 20, 0)),
    ]));
    let sandbox = sandbox("tui-settings");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("settings");
    session.spawn(&sandbox, Some(&mock), true, &[], &["--yolo"]);
    session.wait_for("YOLO MODE", std::time::Duration::from_secs(20));

    session.send(&["run the sleeper", "Enter"]);
    session.wait_for("running", std::time::Duration::from_secs(20));

    // Submit /settings mid-turn. The notice must be on screen WITH the
    // turn's `running` cue within five seconds - that simultaneity is the
    // whole regression: before the fix the notice only appeared after the
    // turn released the lock, by which time `running` was gone. Polled
    // rather than read off one capture: the status row is the last row
    // painted, and a torn frame failed this row once on a loaded runner
    // (2026-10-02) while the turn was demonstrably still running.
    session.send(&["/settings", "Enter"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mid = loop {
        let pane = session.capture();
        if pane.contains("key = value [source]") && pane.contains("running") {
            break pane;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "the settings selector did not share the screen with the running turn within 5s:\n{pane}"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    };
    assert!(
        mid.contains("running"),
        "the settings selector opened while the turn was still running:\n{mid}"
    );

    // Ctrl+C during that same window must reach the agent (the owner's
    // \"not even ctrl-c is able to close it\").
    session.send(&["C-c"]);
    let cancelled = session.wait_for("cancelled", std::time::Duration::from_secs(10));
    assert!(
        cancelled.contains("cancelled"),
        "the running turn cancels while the settings selector is up:\n{cancelled}"
    );

    session.send(&["/exit", "Enter"]);
}
