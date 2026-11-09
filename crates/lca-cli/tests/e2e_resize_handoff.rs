//! gh #522 in a real pane: a resize that lands **while a child owns the tty**.
//!
//! # The defect
//!
//! The `$EDITOR` / interactive-`!` / suspend handoff stops the terminal's reader
//! thread and starts a new one afterwards. While the child owns the tty nobody is
//! watching the window size, and the restart seeds the reader's baseline from the
//! size it *finds* - so a resize in that window is never "different from" anything,
//! no resize fires, and the next frame is composed at the width the interface
//! recorded **before** the handoff.
//!
//! Measured before the fix, three trials of three: a 140-column dock painted into a
//! 60-column terminal, its editor-frame cap soft-wrapped by tmux into `╭───` plus a
//! continuation row. After the fix, three trials of three clean, and the live screen
//! settles with no torn cap at all.
//!
//! # Why a torn cap and not a max width
//!
//! Every physical row of a wrapped frame still *fits* the pane - tmux wrapped it for
//! us - so a max-width check says "60, fine" while the layout is 140. The signature
//! that cannot be fooled is a box corner that never closes on its own row:
//! `╭` with no `╮`, `╰` with no `╯`. That is `wait_for_layout_width`'s rule too
//! (gh #522's second half), and this file asserts the frame directly so a future
//! harness change cannot weaken it.
//!
//! Unix-only: the fixture is a `sh` script, the handoff is a `$EDITOR`, and the rows
//! drive tmux. The frame-level half of the contract lives in `lca-ui`'s
//! `the_frame_is_laid_out_for_the_width_it_is_painted_into`, and the size-adoption
//! rule in `lca-tui`'s `a_resumed_terminal_adopts_the_size_the_tty_has_now`, so the
//! product behaviour is pinned on every platform even though the pane rows are not.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

#[cfg(unix)]
mod common;

#[cfg(unix)]
use common::*;

/// A `$EDITOR` that records the tty size it was handed, then occupies it long enough
/// for a resize to land while the app is **not** reading.
#[cfg(unix)]
fn slow_editor(dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("slow-editor.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nstty size </dev/tty > \"$M522_EDITOR_SAW\" 2>/dev/null || true\n\
         sleep \"${M522_EDITOR_NAP:-2}\"\nexit 0\n",
    )
    .expect("write the editor script");
    let mut mode = std::fs::metadata(&script).expect("stat").permissions();
    mode.set_mode(0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    script
}

/// Lines of a pane whose box cap never closes: the frame is laid out wider than the
/// pane, whatever padding each physical row has.
#[cfg(unix)]
fn torn_caps(pane: &str) -> Vec<String> {
    let mut torn = Vec::new();
    for line in pane.lines() {
        for (open, close) in [('╭', '╮'), ('╰', '╯')] {
            if line.contains(open) && !line.contains(close) {
                torn.push(line.trim_end().to_string());
            }
        }
    }
    torn
}

// Verifies: gh #522 - a resize that lands **while a child owns the tty** (the
// `$EDITOR` handoff) still reaches the layout when the interface resumes.
//
// The resize is issued only after the editor script has proved it is running with
// the tty (`stty size` from the child's own `/dev/tty`), so the sequence is a fact
// rather than a sleep: the app has stopped its reader, the child holds the terminal,
// and *then* the pane changes size. Reading happens after the child exits and the
// live screen has settled, so what is measured is the resumed frame, not tmux's
// rewrap of the old one.
#[cfg(unix)]
#[test]
fn a_resize_during_the_editor_handoff_reaches_the_layout_on_resume() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text_with_usage(
        "editor handoff turn",
        20,
        0,
    ))]));
    let sandbox = sandbox("resize-handoff");
    sandbox.approve_loopback_net(serde_json::json!({}));
    let sleeper = slow_editor(&sandbox.root);
    let saw = sandbox.root.join("editor-saw.txt");
    let saw_env = saw.to_string_lossy().into_owned();

    let session = Tmux::new("resize-handoff");
    session.spawn(
        &sandbox,
        Some(&mock),
        true,
        &[
            ("EDITOR", &sleeper.to_string_lossy()),
            ("VISUAL", &sleeper.to_string_lossy()),
            ("M522_EDITOR_SAW", &saw_env),
        ],
        &[],
    );
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(25),
    );
    // 140 columns at spawn; prove there is nothing stale before the handoff.
    assert!(
        torn_caps(&session.capture()).is_empty(),
        "the boot frame is laid out for its own pane"
    );

    // The prompt handoff: type something, then `Ctrl+X Ctrl+E`.
    session.send(&["draft for the editor"]);
    session.wait_for_input_ready(std::time::Duration::from_secs(15));
    session.send(&["C-x"]);
    session.send(&["C-e"]);

    // The child proves it holds the tty by reading its size from /dev/tty.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !saw.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the editor never started inside the timeout, so the handoff under test \
             never happened"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // **Now** the pane changes size - the app is not reading, the child is. Injected
    // without the harness's own wait on purpose (`send_resize_immediately`, gh #522):
    // the layout cannot move while nobody is reading, so `resize` would sit against
    // the defect instead of measuring the frame afterwards.
    session.send_resize_immediately(60, 28);

    // The claim, through the house primitive: the app re-lays-out for 60 columns.
    // This is the assertion that failed before the resume path adopted the tty's size,
    // and it is **bounded** rather than slept on - `wait_for_layout_width_within`
    // polls the layout, and its failure message names the width it is still painting.
    //
    // The editor's own nap bounds how long the resume can take, so the budget here is
    // generous relative to it and still far below the old 20 s default.
    let resumed = session.wait_for_layout_width_within(60, std::time::Duration::from_secs(12));
    let torn = torn_caps(&resumed);
    assert!(
        torn.is_empty(),
        "a resize during the handoff left the resumed frame laid out for the old \
         pane: {} cap row(s) never close - gh #522.\n--- frame ---\n{resumed}",
        torn.len()
    );
    // And the honest width question, through the house primitive: at 60 columns the
    // app's layout must fit 60. This is the assertion that failed on 3 of 3 trials
    // before the resume path adopted the tty's size.
    assert!(
        session.logical_width() <= 60,
        "the resumed layout is at most 60 cells wide (measured {})",
        session.logical_width()
    );
}
