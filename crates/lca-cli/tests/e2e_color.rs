//! Real-pane color receipts (R5): the user band, the tool cards' state
//! backgrounds, and the separator's spinner row, read back from a live
//! pane with `capture-pane -e`.
//!
//! Split from `e2e_terminal.rs` under the same 1,200-line ceiling rule.
//! The tmux harness (private socket, `Drop`) is `tests/common`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

// The rows are Unix-only (tmux), so the glob import is too.
#[cfg(unix)]
use common::*;

// The pi-dark roles under test, as tmux reports them (`48;2;` = a
// 24-bit background): userMessageBg #343541, toolPendingBg #282832,
// toolSuccessBg #283228, toolErrorBg #3c2828, accent #8abeb7.
// The syntax roles (`38;2;` foregrounds): syntaxComment #6a9955,
// syntaxKeyword #569cd6, syntaxString #ce9178, syntaxFunction #dcdcaa.
#[cfg(unix)]
const USER_BAND: &str = "48;2;52;53;65";
#[cfg(unix)]
const TOOL_PENDING: &str = "48;2;40;40;50";
#[cfg(unix)]
const TOOL_SUCCESS: &str = "48;2;40;50;40";
#[cfg(unix)]
const TOOL_ERROR: &str = "48;2;60;40;40";
#[cfg(unix)]
const SYNTAX_COMMENT: &str = "38;2;106;153;85";
#[cfg(unix)]
const SYNTAX_KEYWORD: &str = "38;2;86;156;214";
#[cfg(unix)]
const SYNTAX_STRING: &str = "38;2;206;145;120";

/// One assistant reply that is a fenced rust block (the JSON escaping the
/// SSE delta needs; the frame is what R5's receipt asserts).
#[cfg(unix)]
fn sse_code_block() -> String {
    let content =
        "```rust\n// greet\nfn greet(name: &str) -> String {\n    format!(\"hi {name}\")\n}\n```";
    let escaped = content
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{escaped}\"}}}}]}}\n\ndata: [DONE]\n\n"
    )
}

// Verifies: R1/R2 on a real terminal - the band, both settled card states,
// and the separator's working row with its spinner, then the idle form
// after the turn. This is the receipt the report puts beside pi's.
#[cfg(unix)]
#[test]
fn bands_card_states_and_the_separator_render_on_a_real_pane() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        // Turn 1: a slow command, so the pending card and the working
        // separator are on screen together.
        Reply::Sse(sse_tool_call("shell", r#"{"command":"sleep 5"}"#)),
        Reply::Sse(sse_text_with_usage("first turn done", 20, 0)),
        // Turn 2: a read that cannot succeed, for the error tint.
        Reply::Sse(sse_tool_call(
            "read",
            r#"{"path":"/nonexistent-lca-color"}"#,
        )),
        Reply::Sse(sse_text_with_usage("second turn done", 20, 0)),
        // Turn 3: a fenced rust block, for the syntax roles.
        Reply::Sse(sse_code_block()),
    ]));
    let sandbox = sandbox("tui-color");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("color");
    session.spawn(&sandbox, Some(&mock), true, &[], &["--yolo"]);
    session.wait_for("YOLO MODE", std::time::Duration::from_secs(20));

    session.send(&["run the sleeper", "Enter"]);
    // While the command runs: the user band, the pending card, and the
    // spinner row, all at once.
    let working = session.wait_for_e(TOOL_PENDING, std::time::Duration::from_secs(25));
    assert!(
        working.contains(USER_BAND),
        "the user band is painted:\n{working}"
    );
    assert!(
        working.contains("Working"),
        "the separator says it:\n{working}"
    );
    assert!(
        working.contains("38;2;138;190;183"),
        "the spinner carries the accent role (#8abeb7):\n{working}"
    );
    assert!(
        !working.contains(TOOL_SUCCESS) && !working.contains(TOOL_ERROR),
        "a running card is neither settled state:\n{working}"
    );

    // After the turn: the success tint and the rest-state separator.
    session.wait_for("first turn done", std::time::Duration::from_secs(30));
    let done = session.capture_e();
    assert!(
        done.contains(TOOL_SUCCESS),
        "the settled card is quiet green:\n{done}"
    );
    assert!(!done.contains("Working"), "the indicator clears:\n{done}");
    assert!(
        done.lines()
            .any(|line| line.chars().filter(|c| *c == '─').count() > 100),
        "the idle separator is a full row of border dashes:\n{done}"
    );

    // Turn 2: a failing read tints the card error-red.
    session.send(&["read a missing file", "Enter"]);
    session.wait_for("second turn done", std::time::Duration::from_secs(30));
    let failed = session.capture_e();
    assert!(
        failed.contains(TOOL_ERROR),
        "the failed card is error-tinted:\n{failed}"
    );
    assert!(
        failed.contains(USER_BAND),
        "the band is still there:\n{failed}"
    );

    // Turn 3: a fenced block highlights per the RE's classes (R3).
    session.send(&["show me a code block", "Enter"]);
    session.wait_for("fn greet", std::time::Duration::from_secs(30));
    let code = session.capture_e();
    assert!(code.contains(SYNTAX_KEYWORD), "keyword class:\n{code}");
    assert!(code.contains(SYNTAX_STRING), "string class:\n{code}");
    assert!(code.contains(SYNTAX_COMMENT), "comment class:\n{code}");
    assert!(
        code.contains("38;2;220;220;170"),
        "function class (#dcdcaa):\n{code}"
    );

    session.send(&["/exit", "Enter"]);
}
