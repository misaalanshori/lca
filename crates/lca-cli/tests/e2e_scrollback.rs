//! Real terminal scrollback receipt tests under tmux (Unix only).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "common/mod.rs"]
mod common;

use common::*;

// Verifies: S1 - main-screen mode incrementally appends to real terminal
// scrollback so history survives above the active working area, and
// `capture-pane -S -` captures genuine prior turns in terminal history.
#[cfg(unix)]
#[test]
fn the_main_screen_retains_real_scrollback_history_in_tmux() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text_with_usage("first response is here", 10, 0)),
        Reply::Sse(sse_text_with_usage("second response is here", 10, 0)),
        Reply::Sse(sse_text_with_usage("third response is here", 10, 0)),
    ]));
    let sandbox = sandbox("tui-scrollback-receipt");
    sandbox.approve_loopback_net(serde_json::json!({}));

    // Start tmux pane with a small height (10 rows) so turns quickly push into scrollback.
    let session = Tmux::new("scrollback");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("test-model", std::time::Duration::from_secs(20));

    // Turn 1
    session.send(&["turn 1 question", "Enter"]);
    session.wait_for("first response is here", std::time::Duration::from_secs(25));

    // Turn 2
    session.send(&["turn 2 question", "Enter"]);
    session.wait_for(
        "second response is here",
        std::time::Duration::from_secs(25),
    );

    // Turn 3
    session.send(&["turn 3 question", "Enter"]);
    session.wait_for("third response is here", std::time::Duration::from_secs(25));

    // Now capture full pane scrollback history with `tmux capture-pane -S -`
    let history = session.capture_with_history();
    assert!(
        history.contains("turn 1 question") && history.contains("first response is here"),
        "scrollback history retains turn 1:\n{history}"
    );
    assert!(
        history.contains("turn 2 question") && history.contains("second response is here"),
        "scrollback history retains turn 2:\n{history}"
    );
    assert!(
        history.contains("third response is here"),
        "visible area contains latest turn:\n{history}"
    );

    // Clean exit
    session.send(&["/exit", "Enter"]);
    let log = wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
    assert!(log.contains("\"t\":\"session-end\""), "session-end written");
}
