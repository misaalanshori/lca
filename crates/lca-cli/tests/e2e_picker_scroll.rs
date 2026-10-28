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
    // gh #447: `[session in` is painted by the first render and says nothing
    // about the input thread. Make the reader's attachment a fact before the
    // scenario is typed at it, or the keys are simply lost.
    session.wait_for_input_ready(std::time::Duration::from_secs(15));
    session.resize(80, 24);

    session.send(&["/settings", "Enter"]);
    // gh #447: the frame the header appears in is not the frame the border is
    // painted in - the selector paints its rows over several frames, so the
    // row settled on macOS and read a `╭` that had not arrived yet. Settling
    // first changes *when* this row reads, not what it asserts: a frame that
    // still lacks the border after settling still fails.
    let pane = session.wait_settled_for("key = value [source]", std::time::Duration::from_secs(15));
    assert!(pane.contains("╭"), "top border draws:\n{pane}");
    assert!(pane.contains("╰"), "bottom border draws:\n{pane}");
    assert!(pane.contains("▼"), "more below at the top:\n{pane}");
    assert!(pane.contains("> Theme"), "starts on the first row:\n{pane}");

    // In-place repaints append nothing to terminal scrollback.
    let history_before = session.capture_with_history().lines().count();
    for _ in 0..10 {
        session.send(&["Down"]);
    }
    // Same class, same primitive: the indicators are repainted after the
    // selection moves, so read a settled frame rather than one a fixed sleep
    // hoped was late enough (gh #447).
    let pane = session.wait_until_painted(std::time::Duration::from_secs(10));
    assert!(
        pane.contains("▲"),
        "more above after stepping down:\n{pane}"
    );
    assert!(pane.contains("▼"), "more below still:\n{pane}");
    // Gh #386 gap 3 named `Space` in this hint, so the fixed string this row read
    // changed with it. The property is unchanged - the pinned hint row survives the
    // scroll - so the row now matches the substring the two spellings share.
    assert!(
        pane.contains("enter/space/←→ change") && pane.contains("q/esc close"),
        "the hint row stays:\n{pane}"
    );
    let history_after = session.capture_with_history().lines().count();
    assert_eq!(
        history_before, history_after,
        "paging repaints in place (no scrollback growth)"
    );

    session.send(&["End"]);
    let pane = session.wait_until_painted(std::time::Duration::from_secs(10));
    assert!(!pane.contains("▼"), "nothing below at the end:\n{pane}");
    assert!(pane.contains("▲"), "plenty above at the end:\n{pane}");
    assert!(
        pane.contains("╰"),
        "the bottom border still closes:\n{pane}"
    );

    // The selected row carries the highlight (raw SGR receipt): the
    // cursor marker rides a styled row. Settled in the SGR capture kind, so the
    // receipt cannot come from a frame older than the one that settled.
    let sgr = session.wait_until_painted_e(std::time::Duration::from_secs(10));
    assert!(sgr.contains("> "), "a cursor row is on screen in SGR");

    session.send(&["Escape"]);
    session.wait_until_painted(std::time::Duration::from_secs(10));
    session.send(&["/exit", "Enter"]);
    // No turns ran, so no log exists for a session-end marker (gh
    // #122): the dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
