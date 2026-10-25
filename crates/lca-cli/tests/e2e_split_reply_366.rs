//! Gh #366 gap 3 in a real terminal (tmux): a terminal reply split across the
//! flush timeout must never be typed into the composer.
//!
//! The unit and regression rows drive `StdinBuffer` directly. This row drives a
//! **real pty** through tmux: it writes `\x1b[?`, sleeps past the 50 ms
//! sequence timeout (so the reader loop's flush really runs), writes the rest,
//! and then asserts the composer is **empty** - not holding `[?62;1;2c`.
//!
//! Then it types normally, to prove the fix did not eat real input.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// How long to wait between the two halves of the reply: well past the 50 ms
/// sequence timeout, so the reader loop's flush is guaranteed to have run.
#[cfg(unix)]
const PAST_THE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(400);

// Verifies: gh #366 gap 3 - a split DA1 reply leaves the composer empty, and
// typing afterwards still works.
#[cfg(unix)]
#[test]
fn a_split_terminal_reply_is_never_typed_into_the_composer() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("split-reply-366");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("split-reply-366");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    session.wait_for_input_ready(std::time::Duration::from_secs(15));

    // The split reply: head, a real pause past the timeout, then the tail.
    // `send_literal` writes raw bytes, so these are the actual escape bytes a
    // terminal would send, not key names.
    session.send_literal("\x1b[?");
    std::thread::sleep(PAST_THE_TIMEOUT);
    session.send_literal("62;1;2c");
    let pane = session.wait_until_painted(std::time::Duration::from_secs(10));

    // The composer must be empty. Before the fix the head was flushed to the
    // editor as text and the tail was typed after it, so the prompt held
    // `[?62;1;2c`.
    for fragment in ["[?62;1;2c", "[?", "62;1;2c", "62;1"] {
        assert!(
            !pane.contains(fragment),
            "the split reply fragment {fragment:?} reached the composer:\n{pane}"
        );
    }

    // Typing still works: the fix must not have eaten real input.
    session.paste_text("hello");
    let typed = session.wait_until_painted(std::time::Duration::from_secs(10));
    assert!(
        typed.contains("hello"),
        "ordinary typing still reaches the composer after a split reply:\n{typed}"
    );

    session.send(&["C-u"]);
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
