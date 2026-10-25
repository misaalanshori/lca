//! Gh #380 gap 1 in a real terminal (tmux): the `@file` popup appears
//! wherever a word starts - and **not** after a mid-sentence `/`.
//!
//! Three frames, and the middle one is the point:
//!
//! 1. `Check @src/m` pops a file list.
//! 2. `See https://example.com/pa` pops **nothing** - the `/` asymmetry.
//!    This is the frame that proves the tokenizer was not generalised to
//!    "after any delimiter" for both markers.
//! 3. `请查看，@src/m` pops a file list (CJK punctuation, no ASCII space).
//!
//! The unit rows prove the token extraction; this row proves the editor in a
//! real pane actually opens and closes the popup on those inputs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// Type `text` into the composer and wait until `needle` appears in the pane.
///
/// Uses [`Tmux::wait_for`] rather than a single paint capture: the popup is
/// drawn by the keystroke's own refresh, so "the pane painted" (the paste echo)
/// is not "the popup arrived". On a loaded macOS runner those are two frames
/// apart, which is how this row failed CI once while passing locally.
#[cfg(unix)]
fn type_and_wait(session: &Tmux, text: &str, needle: &str) -> String {
    session.paste_text(text);
    session.wait_for(needle, std::time::Duration::from_secs(20))
}

/// Type `text` and return the pane after the popup had its chance to appear,
/// for the rows asserting that it did **not**.
#[cfg(unix)]
fn type_and_capture(session: &Tmux, text: &str) -> String {
    session.paste_text(text);
    session.wait_until_painted(std::time::Duration::from_secs(10))
}

// Verifies: gh #380 gap 1 - a mid-sentence `@` opens the file popup in a
// real pane, and a mid-sentence `/` does not open a command menu.
#[cfg(unix)]
#[test]
fn the_at_popup_follows_a_word_start_in_a_real_pane() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    // The popup needs no turn, so the mock never serves anything.
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("autocomplete-380");
    sandbox.approve_loopback_net(serde_json::json!({}));

    // The project the agent runs in, so `src/main.rs` exists to be found.
    let project = sandbox.root.join("project");
    std::fs::create_dir_all(project.join("src")).expect("mkdir src");
    std::fs::write(project.join("src/main.rs"), "fn main() {}\n").expect("write");

    let session = Tmux::new("autocomplete-380");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    session.wait_for_input_ready(std::time::Duration::from_secs(15));

    // Frame 1: a mid-sentence `@` proposes the file. Before the fix the popup
    // needed the `@` at column 0, so `Check @src/m` proposed nothing.
    let mid_sentence = type_and_wait(&session, "Check @src/m", "main.rs");
    assert!(
        mid_sentence.contains("main.rs"),
        "a mid-sentence `@` proposes `src/main.rs`:\n{mid_sentence}"
    );

    // Clear the composer between frames (Ctrl+U kills the line).
    session.send(&["C-u"]);

    // Frame 2: the asymmetry. A mid-sentence `/` must open no menu at all -
    // neither a command menu nor a file list.
    let slash = type_and_capture(&session, "See https://example.com/pa");
    assert!(
        !slash.contains("/model") && !slash.contains("main.rs"),
        "a mid-sentence `/` opens no popup:\n{slash}"
    );
    // The typed text is still there, so frame 2 is "no popup", not "the
    // keystrokes were lost" - which would pass for the wrong reason.
    assert!(
        slash.contains("example.com/pa"),
        "the url is still in the composer:\n{slash}"
    );

    session.send(&["C-u"]);

    // Frame 3: CJK prose before the `@`, with no ASCII space after the
    // fullwidth comma.
    let cjk = type_and_wait(&session, "请查看，@src/m", "main.rs");
    assert!(
        cjk.contains("main.rs"),
        "a CJK sentence completes the attachment:\n{cjk}"
    );

    session.send(&["C-u"]);
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
