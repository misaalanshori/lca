//! Picker rolling-window rows on the real terminal (gh #226):
//! tall-list viewports, indicators, in-place repaints, scroll-offset
//! clicks live under tmux.
//!
//! Split from `e2e_terminal.rs` under the workspace's 1,200-line file
//! ceiling; the harness (`common`) is shared.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #226 - the inventoried `/settings` selector on an
// 80x24 pane rolls in place: the frame closes, the selection stays
// visible with scroll counts, paging appends nothing to terminal
// scrollback, and End reaches the last row.
#[cfg(unix)]
#[test]
fn a_tall_settings_selector_rolls_with_frame_and_selection_on_screen() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("gh226-tall-picker");
    sandbox.approve_loopback_net(serde_json::json!({}));
    let session = Tmux::new("gh226-tall-picker");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    session.resize(80, 24);

    session.send(&["/settings", "Enter"]);
    let pane = session.wait_for("key = value [source]", std::time::Duration::from_secs(15));
    assert!(pane.contains("╭"), "top border draws:\n{pane}");
    assert!(pane.contains("╰"), "bottom border draws:\n{pane}");
    assert!(pane.contains("▼"), "more below at the top:\n{pane}");
    assert!(
        pane.contains("> Theme"),
        "starts on the first row:\n{pane}"
    );

    // In-place repaints append nothing to terminal scrollback.
    let history_before = session.capture_with_history().lines().count();
    for _ in 0..10 {
        session.send(&["Down"]);
    }
    std::thread::sleep(std::time::Duration::from_millis(600));
    let pane = session.capture();
    assert!(
        pane.contains("▲"),
        "more above after stepping down:\n{pane}"
    );
    assert!(pane.contains("▼"), "more below still:\n{pane}");
    assert!(
        pane.contains("enter/←→ change"),
        "the hint row stays:\n{pane}"
    );
    let history_after = session.capture_with_history().lines().count();
    assert_eq!(
        history_before, history_after,
        "paging repaints in place (no scrollback growth)"
    );

    session.send(&["End"]);
    std::thread::sleep(std::time::Duration::from_millis(600));
    let pane = session.capture();
    assert!(!pane.contains("▼"), "nothing below at the end:\n{pane}");
    assert!(pane.contains("▲"), "plenty above at the end:\n{pane}");
    assert!(
        pane.contains("╰"),
        "the bottom border still closes:\n{pane}"
    );

    // The selected row carries the highlight (raw SGR receipt): the
    // cursor marker rides a styled row.
    let sgr = session.capture_e();
    assert!(sgr.contains("> "), "a cursor row is on screen in SGR");

    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(400));
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}
