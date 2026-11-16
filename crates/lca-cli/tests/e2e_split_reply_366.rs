//! Gh #366 gap 3 in a real terminal (tmux): a terminal reply split across the
//! flush timeout must never be typed into the composer.
//!
//! The unit and regression rows drive `StdinBuffer` directly. This row drives a
//! **real pty** through tmux: it writes `\x1b[?`, sleeps past the 50 ms
//! sequence timeout (so the reader loop's flush really runs), writes the rest,
//! and then asserts the composer is **empty** - not holding `[?62;1;2c`.
//!
//! Then it types normally, to prove the fix did not eat real input.
//!
//! **The order of those two claims was wrong until gh #603.** The row read the
//! composer **first** (an absence: "the split reply never landed") and then pasted
//! and read **again** (a presence: "typing still works"), and the second reading
//! was a bare `wait_until_painted` - a **settle**, which knows only that the screen
//! stopped changing, not that the paste had reached it. On CI `43701559681` (macOS,
//! head `5c7ef3c6`) the settle returned a frame the paste had not landed in and the
//! row failed on `ordinary typing still reaches the composer after a split reply:`.
//! The frame *was* settled; it was simply a frame from before the input arrived.
//!
//! So the order is now **positive first, absence second, on one frame**: paste,
//! wait for the echo (that is the fact which makes the frame live - the harness
//! primitive for it), then settle and assert both claims against the settled
//! capture. The absence is not weakened by this; it is strengthened, because it is
//! now read from a frame that demonstrably carries typing, where a leaked reply
//! fragment would appear next to the word rather than on an empty composer
//! (`[?62;1;2chello`).

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

    // **The positive half first**, and it is what proves the frame is live: the
    // paste can only be echoed through a working input path, so waiting for `hello`
    // makes "the screen has settled" mean "the screen has settled *with this
    // input in it*". `wait_for` is the bounded, load-scaled wait (`readiness.rs`'s
    // own contract); a bare settle here is what fired on CI `43701559681`.
    session.paste_text("hello");
    // Typing still works: the fix must not have eaten real input.
    session.wait_for("hello", std::time::Duration::from_secs(10));
    // Read the composer **on the frame that carries the typing**, then assert both
    // halves there: absence of the reply fragment, presence of the word.
    let pane = session.wait_until_painted(std::time::Duration::from_secs(10));
    assert!(
        pane.contains("hello"),
        "the settled frame still carries the paste:\n{pane}"
    );

    // The composer must not hold the reply. Before the fix the head was flushed to
    // the editor as text and the tail was typed after it, so the prompt held
    // `[?62;1;2c` - and after the paste it would hold `[?62;1;2chello`, which is
    // why this absence is read from the pasted frame rather than an earlier one.
    for fragment in ["[?62;1;2c", "[?", "62;1;2c", "62;1"] {
        assert!(
            !pane.contains(fragment),
            "the split reply fragment {fragment:?} reached the composer:\n{pane}"
        );
    }

    session.send(&["C-u"]);
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
