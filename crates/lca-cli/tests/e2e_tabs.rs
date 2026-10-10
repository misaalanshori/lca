//! Session tabs journey (gh #209, FR-UI-16): open, switch with
//! per-tab drafts, and close in a real terminal.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #209 - Ctrl+T opens a second tab with its own session;
// Alt+1/Alt+2 switch with drafts isolated per tab; clearing then
// Ctrl+W closes back to one tab, and the first session continues.
#[cfg(unix)]
#[test]
fn tabs_open_switch_isolated_and_close_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text_with_usage("answer one", 20, 0)),
        Reply::Sse(sse_text_with_usage("answer two", 20, 0)),
    ]));
    let sandbox = sandbox("tabs");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("tabs");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    // One turn on tab one, then a second tab beside it.
    session.send(&["alpha question", "Enter"]);
    session.wait_for("answer one", std::time::Duration::from_secs(25));
    session.wait_for("done", std::time::Duration::from_secs(25));
    session.send(&["C-t"]);
    let pane = session.wait_for("untitled", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("alpha question"),
        "the bar names both sessions:\n{pane}"
    );

    // A draft on tab two stays on tab two.
    session.send(&["draft text"]);
    session.wait_for("draft text", std::time::Duration::from_secs(10));
    session.send(&["M-1"]);
    std::thread::sleep(std::time::Duration::from_millis(800));
    let pane = session.capture();
    assert!(
        !pane.contains("draft text"),
        "tab one's composer is clean:\n{pane}"
    );
    session.send(&["M-2"]);
    session.wait_for("draft text", std::time::Duration::from_secs(10));

    // Clear, close, and the first session continues where it was.
    session.send(&["C-c"]);
    std::thread::sleep(std::time::Duration::from_millis(600));
    session.send(&["C-w"]);
    session.wait_for("answer one", std::time::Duration::from_secs(15));
    session.send(&["second question", "Enter"]);
    session.wait_for("answer two", std::time::Duration::from_secs(25));
    // Turn-end, not first-delta: /exit into a running turn never
    // lands (gh #231's lesson, same shape).
    session.wait_for("done", std::time::Duration::from_secs(25));
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}
