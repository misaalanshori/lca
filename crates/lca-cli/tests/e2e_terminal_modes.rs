//! Real-terminal rows for the permission matrix and the caret (TUI-8):
//! yolo's no-modal run with its recorded `permission` record, the
//! read-only auto-approve, and the caret advancing on a typed space.
//!
//! Split out of `e2e_terminal.rs` so both files stay under the
//! workspace-wide 1,200-line ceiling (cycle-4 standard). The tmux harness
//! (`Tmux`, its private socket, and `Drop`) moved to `tests/common`, which
//! both files share.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

// The rows are Unix-only (tmux), so the glob import is too: an ungated
// `use common::*` is an unused import on Windows, which clippy denies.
#[cfg(unix)]
use common::*;

// Verifies: FR-PERM-25 (ADR-0042; R10's permission matrix, yolo column) - `--yolo`
// approves a review-class tool call with no modal, and the session log
// carries the `permission` record a human "always" answer would have
// written: approve everything, forget nothing.
#[cfg(unix)]
#[test]
fn yolo_approves_without_a_modal_and_records_the_decision() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    // An out-of-workspace command: review-class, so ask mode would prompt.
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("shell", r#"{"command":"cat /etc/hostname"}"#)),
        Reply::Sse(sse_text_with_usage("turn complete", 20, 0)),
    ]));
    let sandbox = sandbox("tui-yolo");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("yolo");
    session.spawn(&sandbox, Some(&mock), true, &[], &["--yolo"]);

    // 1. The banner leads the transcript: hands-free is never invisible.
    session.wait_for("YOLO MODE", std::time::Duration::from_secs(20));

    // 2. The turn runs to completion with no modal in the way.
    session.send(&["do a thing", "Enter"]);
    session.wait_for("turn complete", std::time::Duration::from_secs(30));
    let pane = session.capture();
    assert!(
        !pane.contains("Allow this action?"),
        "yolo never shows the permission modal:\n{pane}"
    );

    // 3. The decision is in the log, exactly like a human "always" answer.
    session.send(&["/exit", "Enter"]);
    let log = wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
    assert!(log.contains("\"t\":\"permission\""), "recorded:\n{log}");
    assert!(log.contains("\"decision\":\"always\""), "always:\n{log}");
    assert!(
        log.contains("cat /etc/hostname"),
        "the exact action is named:\n{log}"
    );
}

// Verifies: FR-PERM-27 (ADR-0042; R10's permission matrix, read-only column) - a read
// outside the workspace does not prompt in the default mode, and leaves no
// permission record because no user decision was made.
#[cfg(unix)]
#[test]
fn a_read_outside_the_workspace_does_not_prompt() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("read", r#"{"path":"/etc/hostname"}"#)),
        Reply::Sse(sse_text_with_usage("turn complete", 20, 0)),
    ]));
    let sandbox = sandbox("tui-read-auto");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("read-auto");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );
    session.send(&["look at a file", "Enter"]);
    session.wait_for("turn complete", std::time::Duration::from_secs(30));
    let pane = session.capture();
    assert!(
        !pane.contains("Allow this action?"),
        "a read does not prompt:\n{pane}"
    );

    session.send(&["/exit", "Enter"]);
    let log = wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
    assert!(
        !log.contains("\"t\":\"permission\""),
        "no decision was made, so none is recorded:\n{log}"
    );
}

// Verifies: FR-UI-24 (R5) - a typed space advances the caret immediately:
// the pane's own cursor column moves on the space keystroke itself, before
// any letter follows. This is the receipt the owner's report asked for; on
// Linux the caret row's repaint is asserted too.
#[cfg(unix)]
#[test]
fn a_typed_space_advances_the_caret_immediately() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("caret-space");
    let session = Tmux::new("caret");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));

    session.send(&["a", "b"]);
    let after_letters = session.cursor_x();
    session.send(&[" "]);
    let after_first_space = session.cursor_x();
    assert_eq!(
        after_first_space,
        after_letters + 1,
        "the first space moved the caret"
    );
    session.send(&[" "]);
    assert_eq!(
        session.cursor_x(),
        after_first_space + 1,
        "the second space moved it again"
    );
    session.send(&["c"]);
    assert_eq!(
        session.cursor_x(),
        after_first_space + 2,
        "and the next letter keeps the column"
    );
}
