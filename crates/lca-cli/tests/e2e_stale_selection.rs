//! gh #403 gap 1, end to end: a transcript replacement must not leave the
//! previous transcript's selection painted over the new rows.
//!
//! **What these rows are, and what they are not.** They drive a real tmux
//! pane with real SGR-1006 mouse events, because a unit row that passes while
//! the terminal misbehaves proves nothing (`docs/testing-plan.md` §14). What
//! they verify is that the journey works end to end - the drag selects, the
//! replacement lands, the pane settles with no stranded highlight, a resize
//! keeps one - and that no step panics or strands the interface.
//!
//! **What they do not do is prove the fix.** Measured, not assumed: with the
//! reset call removed from `run_selection.rs` these rows still pass, three
//! runs each. The reason is structural rather than a test bug. A replacement
//! replays records, which rewrites the very rows the selection covered, so
//! the stale highlight is painted over by new content and the *paint* symptom
//! is not observable once the replacement has settled. The wrong-data
//! symptom is also invisible from a pane: `capture-pane` cannot see the OSC
//! 52 payload, and a shorter replay leaves the old coordinates addressing
//! blank rows, which a copy reports the same way as "nothing selected".
//!
//! The bite-proof lives in the unit rows next to the logic
//! (`run_selection_tests.rs`), where removing the reset fails five of seven
//! by name. These rows are the real-terminal smoke test that the fix does not
//! break the pane, and they are labelled as such rather than dressed up as a
//! regression guard they are not.
//!
//! Both directions are pinned here too. Dropping the highlight is half the
//! contract; the other half is that a *non*-replacement (a resize) keeps it,
//! or the fix would train users out of selecting anything.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// `Duration` is only used by the unix-gated rows below, so the import carries
// the same gate; without it `windows-latest` clippy fails on an unused import
// (the sibling of the `Tmux` gate added in 972d39bc).
#[cfg(unix)]
use std::time::Duration;

mod common;

// Every row here drives a real tmux pane, and `Tmux` itself is
// `#[cfg(unix)]` in `common` (the harness is a Unix pty). Without this gate
// the whole module fails to compile on `windows-latest` - which is what
// happened: batch-15 landed this file ungated and the Windows clippy leg went
// red (`cannot find type Tmux in this scope`). gh #444's neighbour.
#[cfg(unix)]
use common::*;

// Verifies: gh #403 gap 1 - after a transcript replacement the selection is
// gone, and the pane shows no reverse-video span.
#[cfg(unix)]
#[test]
fn a_transcript_replacement_clears_the_highlighted_rows() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text("first answer here")),
        Reply::Sse(sse_text("second answer here")),
    ]));
    let sandbox = sandbox("stale-selection-cleared");
    sandbox.approve_loopback_net(serde_json::json!({}));
    let session = Tmux::new("stale-selection-cleared");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", Duration::from_secs(25));
    // `/fullscreen` is a command, not a flag: the alt screen (and with it
    // app-owned selection) is what these rows need. The alt screen is the
    // one that enables SGR mouse reporting.
    session.send(&["/fullscreen", "Enter"]);
    session.wait_for_e("\x1b[?1049h", Duration::from_secs(10));

    // A real turn, so the transcript has text worth selecting.
    session.send(&["say something", "Enter"]);
    session.wait_for_e("first answer here", Duration::from_secs(30));
    let plain = session.capture();
    let (row, _) =
        locate(&plain, &plain, "first answer here").expect("the answer row is on screen");
    // The turn must be over before a switch: `switch_or_announce` refuses
    // mid-turn (a live agent and its stream must not be swapped in flight),
    // so a `/clone` sent too early leaves the transcript in place - which
    // would make this row pass for the wrong reason.
    session.wait_for("done", Duration::from_secs(30));

    drag_over(&session, row, 1, 30);
    // The highlight is the next frame's business: poll for it rather than
    // sleeping a fixed amount.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let selected = loop {
        let sgr = session.capture_e();
        if has_highlight(&sgr) || std::time::Instant::now() >= deadline {
            break sgr;
        }
        std::thread::sleep(Duration::from_millis(80));
    };
    assert!(
        has_highlight(&selected),
        "the drag really selected something, or the rest of this row is vacuous"
    );

    // Replace the transcript with a SHORTER one. `/fork 0` forks at the
    // first user message and switches, so the replay has one exchange where
    // the original had two: the rows the selection covers now hold nothing
    // (or different text), which is exactly the state a stale selection
    // corrupts. `/clone` would replay the same text and prove nothing.
    //
    // The seam is `switch_or_announce` -> `replay_records` ->
    // `transcript.clear()`, shared by `/resume`, `/tree` jumps, `/fork`,
    // `/clone`, and `/import`; LCA has no `/new`.
    // Typed with a trailing space: the autocomplete popup matches on the
    // prefix, and a space ends the command name so Enter submits the line
    // instead of completing a suggestion (`/fork` -> `/format...`). Escape
    // is not an option here - it would be read by the editor with a live
    // selection, and the run loop copies on that gesture.
    session.send(&["/fork 0 ", "Enter"]);

    // Wait for the replacement to land, then read what a copy would take.
    //
    // **Why the copy, not the paint.** The fork's replay rewrites the rows
    // the selection covered, so the stale highlight is overwritten by new
    // content either way - the *paint* symptom is not observable on a
    // real pane once the replacement has repainted. The consequence that
    // survives is the dangerous one: the selection's anchor still addresses
    // the old rows, so a copy hands back text from the transcript that is
    // gone. That is what this asserts, and it is what the fix changes.
    session.wait_for("Forked from turn", Duration::from_secs(20));
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let after = loop {
        let sgr = session.capture_e();
        if !has_highlight(&sgr) || std::time::Instant::now() >= deadline {
            break sgr;
        }
        std::thread::sleep(Duration::from_millis(120));
    };
    assert!(
        !has_highlight(&after),
        "a replaced transcript must show no highlighted rows"
    );

    // The wrong-data receipt: a copy taken now must not carry the answer the
    // user selected from the replaced transcript.
    session.send(&["C-x"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let copied = loop {
        let pane = session.capture();
        if pane.contains("copied") || std::time::Instant::now() >= deadline {
            break pane;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // `Ctrl+X` with an empty selection falls back to the last assistant
    // message ("Chat names the target"), which after a fork is the forked
    // exchange - never the replaced one's answer.
    assert!(
        !copied.contains("second answer here"),
        "a copy after a replacement must not reach into the transcript that \
         was replaced:\n{copied}"
    );

    // The fork restores the forked message into the editor for immediate
    // re-prompting, so clear it before `/exit` (otherwise the text is typed
    // into a non-empty buffer and the command never reaches the router).
    session.send(&["C-u"]);
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, Duration::from_secs(15));
}

// Verifies: gh #403 gap 1, the mirror image - a resize does NOT replace the
// transcript, so the selection must still be painted afterwards. Without
// this row a "clear on every frame" fix would pass the row above.
#[cfg(unix)]
#[test]
fn a_resize_keeps_the_highlighted_rows() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text(
        "resizable answer here",
    ))]));
    let sandbox = sandbox("stale-selection-resize");
    sandbox.approve_loopback_net(serde_json::json!({}));
    let session = Tmux::new("stale-selection-resize");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", Duration::from_secs(25));
    // `/fullscreen` is a command, not a flag: the alt screen (and with it
    // app-owned selection) is what these rows need. The alt screen is the
    // one that enables SGR mouse reporting.
    session.send(&["/fullscreen", "Enter"]);
    session.wait_for_e("\x1b[?1049h", Duration::from_secs(10));

    session.send(&["say something", "Enter"]);
    session.wait_for_e("resizable answer here", Duration::from_secs(30));
    let plain = session.capture();
    let (row, _) =
        locate(&plain, &plain, "resizable answer here").expect("the answer row is on screen");
    drag_over(&session, row, 1, 30);

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let selected = loop {
        let sgr = session.capture_e();
        if has_highlight(&sgr) || std::time::Instant::now() >= deadline {
            break sgr;
        }
        std::thread::sleep(Duration::from_millis(80));
    };
    assert!(has_highlight(&selected), "the fixture selected text");

    // A resize: same transcript, new geometry. tmux resizes the pane, the
    // interface repaints, and the selection above must survive.
    //
    // **Height, not width** (gh #399). The property this row guards is "a resize
    // must not *clear* the selection" - the mirror-image of gh #403's
    // reset-fires-too-often bug - and only a height change tests it. A width
    // change *reflows*: text rewraps, rows move, and a selection held as screen
    // coordinates (which is what it is) legitimately no longer covers the same
    // sentences. Asserting a highlight survives a reflow would be asserting
    // something the architecture does not promise, and it only passed before
    // because a one-row header happened to reflow to the same height. The
    // startup header legitimately grows a row when its prose nudge rewraps.
    session.resize(140, 30);
    std::thread::sleep(Duration::from_millis(600));
    let after = session.capture_e();
    assert!(
        has_highlight(&after),
        "a resize must keep the user's selection, not silently drop it"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, Duration::from_secs(15));
}
