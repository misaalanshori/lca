//! Host-rendered dialog flows on a real terminal (gh #124, gh #172):
//! a tool's `ui.confirm` opens the host's own modal mid-turn and the
//! answer reaches the turn.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #124 on a real pane - a tool's `ui.confirm` opens the
// host's own modal mid-turn, `y` answers yes, and the verdict reaches
// the transcript as the tool result.
#[cfg(unix)]
#[test]
fn a_tool_dialog_confirms_on_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("conformance", r#"{"mode":"ask-confirm"}"#)),
        Reply::Sse(sse_text_with_usage("all done", 20, 0)),
    ]));
    let sandbox = sandbox("tui-dialog");
    sandbox.approve_loopback_net(serde_json::json!({}));
    sandbox.install_component(
        "conformance",
        include_str!("../../../extensions/conformance/extension.toml"),
        include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm"),
    );

    let session = Tmux::new("dialog");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    // Wall-clock margin (gh #172 cycle: debug WASM compiles stack up
    // when heavy tmux tests share few cores).
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(30),
    );

    // The turn asks its question through the host's chrome.
    session.send(&["ask the question", "Enter"]);
    session.wait_for("proceed?", std::time::Duration::from_secs(25));

    // Answering yes lets the turn run past the question to its reply
    // (the verdict travels to the model, not the screen: `all done`
    // only renders when the tool answered).
    session.send(&["y"]);
    session.wait_for("all done", std::time::Duration::from_secs(25));

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}
