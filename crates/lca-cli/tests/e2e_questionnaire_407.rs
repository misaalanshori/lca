//! Gh #407 on a real terminal: a three-question ask opens **one** modal with
//! three tabs, is answered partly by keyboard and partly by typing, refuses its
//! submit once, then submits - and the answers reach the tool result.
//!
//! The unit rows beside the widget (`crates/lca-ui/src/chat_questionnaire_tests.rs`)
//! pin the key contract on the fake terminal. This row is the other half: a mock
//! provider calls the conformance probe's `ask-questionnaire` mode **through the
//! WASM sandbox**, so the chain is the shipped one - mock → turn → guest →
//! `ui-dialogs.questionnaire` import → the session's dialog slot → the real
//! tmux pane - and back out as the tool's content.
//!
//! Verifies: gh #407's acceptance ("multi-question prompts complete in one turn
//! with tabbed progress, inline-other, and gated submit"), FR-UI-2, FR-UI-6.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// The three-question ask the mock hands the probe.
///
/// One line on purpose: `sse_tool_call` embeds the arguments in an SSE `data:`
/// payload, and a raw newline would end that event early - the tool then sees
/// empty arguments (`not valid JSON: EOF while parsing a value`), which is what
/// the first draft of this row failed with.
const ASK: &str = concat!(
    r#"{"mode":"ask-questionnaire","title":"setup","questions":["#,
    r#"{"header":"Scope","prompt":"Which scope?","options":["one","two","three"],"multiple":false},"#,
    r#"{"header":"Priority","prompt":"How soon?","options":["today","tomorrow"],"multiple":false},"#,
    r#"{"header":"Retries","prompt":"How many retries?","options":["none","three"],"multiple":false}"#,
    "]}"
);

// Verifies: gh #407 - the whole questionnaire flow on a real pane. Every step
// asserts a *painted* row (never an internal field), because the point of the
// slice is that a user can drive N questions without leaving the modal.
#[cfg(unix)]
#[test]
fn a_three_question_ask_is_answered_and_submitted_in_one_modal_on_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("conformance", ASK)),
        Reply::Sse(sse_text_with_usage("all set", 20, 0)),
    ]));
    let sandbox = sandbox("tui-questionnaire-407");
    sandbox.approve_loopback_net(serde_json::json!({}));
    sandbox.install_component(
        "conformance",
        include_str!("../../../extensions/conformance/extension.toml"),
        include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm"),
    );

    let session = Tmux::new("questionnaire");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(30),
    );
    // A wide pane so all three tabs and the end-cap fit one row (the twelve-
    // question elision is the unit row's job, not this one's).
    session.wait_for_input_ready(std::time::Duration::from_secs(15));
    session.resize(100, 30);

    session.send(&["ask me three things", "Enter"]);
    // The ask opens as one modal: the tab row names all three questions at
    // once, all pending, with the submit end-cap.
    let pane = session.wait_for("□ Scope", std::time::Duration::from_secs(30));
    assert!(pane.contains("□ Priority"), "three tabs, not one:\n{pane}");
    assert!(pane.contains("□ Retries"), "three tabs, not one:\n{pane}");
    assert!(pane.contains("Submit"), "the end-cap is painted:\n{pane}");
    assert!(
        pane.contains("Which scope?"),
        "the first question is the body:\n{pane}"
    );
    assert_eq!(
        pane.matches("╭─ setup").count(),
        1,
        "one modal, not one per question:\n{pane}"
    );

    // The tab row under SGR, quoted for the report: the active tab is on the
    // selected background, and the answered markers are plain until answered.
    let raw = session.capture_e();
    let tab_line = raw
        .lines()
        .find(|line| line.contains("Scope"))
        .unwrap_or_else(|| panic!("the tab row is painted:\n{raw}"));
    assert!(
        tab_line.contains("[48;5;") || tab_line.contains("[48;2;"),
        "the active tab carries its own background: {tab_line:?}"
    );
    assert!(
        !tab_line.contains("[0m") || tab_line.matches("[0m").count() <= 4,
        "no all-channel reset parade on the tab row: {tab_line:?}"
    );

    // **Question 1 by keyboard**: down to the second option, Enter confirms and
    // advances. The marker flips to `■` on the tab it answered.
    session.send(&["Down", "Enter"]);
    let pane = session.wait_for("■ Scope", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("How soon?"),
        "answering advanced to the next tab:\n{pane}"
    );

    // **The gate refuses once**: walk to the end-cap with two questions open.
    // Two tabs, because the answer already advanced to `Priority`; a third
    // would wrap back to the first question (the row's first draft did exactly
    // that, and the pane showed `■ Scope` still in the body).
    session.send(&["Tab", "Tab"]);
    let pane = session.wait_for("Unanswered:", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("Unanswered: Priority, Retries"),
        "the review names what is missing:\n{pane}"
    );
    session.send(&["Enter"]);
    let pane = session.wait_for("How soon?", std::time::Duration::from_secs(15));
    assert!(
        !pane.contains("all set"),
        "the refused submit did not end the turn:\n{pane}"
    );
    assert!(
        pane.contains("Unanswered:") || pane.contains("How soon?"),
        "the refusal landed on the first unanswered tab:\n{pane}"
    );

    // **Question 2 typed into the inline editor**: the custom row is last.
    session.send(&["Down", "Down", "Enter"]);
    session.wait_for("Type your own answer", std::time::Duration::from_secs(15));
    session.send(&["in a minute"]);
    let pane = session.wait_for("in a minute", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("✎"),
        "the editor holds the field with its marker:\n{pane}"
    );
    session.send(&["Enter"]);
    let pane = session.wait_for("How many retries?", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("■ Priority"),
        "the typed answer marked its tab:\n{pane}"
    );

    // **Question 3 by keyboard, then submit**: the last answer lands on the
    // review, and the second Enter fires it.
    session.send(&["Down", "Enter"]);
    let pane = session.wait_for("Ready to submit", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("Scope: two"),
        "the review lists the chosen answer:\n{pane}"
    );
    assert!(
        pane.contains("Priority: (wrote) in a minute"),
        "and marks the typed one:\n{pane}"
    );
    assert!(
        !pane.contains("Unanswered:"),
        "nothing is missing now:\n{pane}"
    );
    session.send(&["Enter"]);

    // The answers crossed back: the turn continues to its reply, and the card
    // **collapses** rather than spilling three answers into the transcript
    // (gh #407's "history collapses cleanly" acceptance).
    session.wait_for("all set", std::time::Duration::from_secs(30));
    // `all set` is painted while the turn is still settling (`⠧ Working` in the
    // same frame), so the collapsed card is read from a settled frame - the
    // gh #447 lesson: a needle's frame is not the frame the next claims need.
    let pane = session.wait_settled_for("done • conformance", std::time::Duration::from_secs(20));
    assert!(
        pane.contains("done • conformance"),
        "the tool call finished and its card collapsed:\n{pane}"
    );
    assert!(
        !pane.contains("Priority=in a minute"),
        "a collapsed card does not spill its content into the transcript:\n{pane}"
    );

    // What the tool *received* is the other half, and it lives in the session
    // log rather than the collapsed card. A chosen option reports its label and
    // a typed one verbatim - the same text the sequential path reports.
    let content = wait_for_tool_result(&sandbox.state_dir(), std::time::Duration::from_secs(30));
    assert_eq!(
        content, "questionnaire: Scope=two; Priority=in a minute (wrote); Retries=three",
        "every answer reached the tool result, in the asker's order"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

/// The content of the first `tool-result` record that reports a questionnaire,
/// polled until it lands (the turn finishes the call after the modal closes).
#[cfg(unix)]
fn wait_for_tool_result(state: &std::path::Path, timeout: std::time::Duration) -> String {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(path) = find_session_log(state)
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            for line in text.lines() {
                let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                if record["t"] != "tool-result" {
                    continue;
                }
                let content = record["content"].as_str().unwrap_or_default();
                if content.contains("questionnaire:") {
                    return content.to_string();
                }
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no questionnaire tool-result under {}",
            state.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}
