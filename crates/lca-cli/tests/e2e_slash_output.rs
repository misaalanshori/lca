//! Slash routing + async catalog (gh #234, gh #232) end to end, in a
//! real terminal (tmux):
//!
//! - `/hotkeys` lands in the scrollback transcript (scrollable,
//!   dismissible by scroll) while the composer dock stays bounded.
//! - `/model` opens its picker frame promptly and the rows fill in.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

// The rows are Unix-only (tmux), so their imports are too: an ungated
// glob import is an unused import on Windows, which clippy denies.
#[cfg(unix)]
use common::*;

// Verifies: gh #234 - a tall informational output scrolls in the
// transcript instead of inflating the dock. The scrollback history
// carries the table (the transcript appends there); the visible pane
// still shows the session footer, so the dock never ate the screen.
#[cfg(unix)]
#[test]
fn hotkeys_lands_in_scrollback_while_the_dock_stays_bounded() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("slash-output-hotkeys");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("slash-output-hotkeys");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));

    session.send(&["/hotkeys", "Enter"]);
    // The table is taller than the pane: its head lives in scrollback,
    // so the wait polls the history, not the visible screen.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let history = loop {
        let history = session.capture_with_history();
        if history.contains("keys:") || std::time::Instant::now() > deadline {
            break history;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    };
    assert!(
        history.contains("keys:") && history.contains("enter"),
        "the table scrolls in the transcript:\n{history:.2000}"
    );
    // Let the repaint settle: the history poll fires on the first
    // appended row, possibly a frame before the dock repaints.
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let pane = session.capture();
    assert!(
        pane.contains("openai-compatible/test-model"),
        "the dock stays bounded (footer intact)"
    );
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: gh #232 - `/model` opens its picker frame promptly and the
// catalog rows fill in (the async journey, wired through the
// background discovery).
#[cfg(unix)]
#[test]
fn model_picker_opens_promptly_and_fills_in() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("slash-output-model");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("slash-output-model");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    // The frame opens promptly (the instant-open half).
    session.send(&["/model", "Enter"]);
    session.wait_for("search:", std::time::Duration::from_secs(10));
    // The rows fill in (the background-discovery half).
    let pane = session.wait_for("test-model", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("test-model"),
        "the catalog rows filled the picker:\n{pane:.1500}"
    );
    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(300));
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}
