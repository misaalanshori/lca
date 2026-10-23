//! GitHub #403 gap 1: a transcript replacement must not keep the previous
//! transcript's selection.
//!
//! The bug: `clear_selection` fired on a picker press only, so every path
//! that replaces the transcript - a `/resume` switch, a `/tree` branch jump,
//! a fork, a clone, an import - left the previous transcript's selection
//! rectangles painted over rows that now hold different text. A selection is
//! in *content* coordinates, so a copy after the replacement handed back
//! bytes from a transcript that no longer existed: a wrong-data path, not
//! just a stale highlight.
//!
//! The fix is a seam rather than five call sites: `Transcript::generation`
//! moves only when the transcript is *cleared*, and the run loop resets the
//! selection whenever it sees a new value. Appends (streaming deltas),
//! resizes, and marker repaints leave the generation alone, so a selection
//! over unchanged rows survives - the mirror-image bug the brief warns about.
//!
//! This file pins the mechanism from outside the crates: the seam exists, it
//! is the only thing that clears, and the reset clears the multi-click run as
//! well as the rectangles. The behavioural bite-proof lives beside the logic
//! in `crates/lca-ui/src/run_selection_tests.rs` (removing the reset fails
//! five of seven rows by name); this one guards against the fix being
//! refactored away while those rows are moved or renamed.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

#[test]
fn the_only_transcript_replacement_bumps_a_generation() {
    let transcript = read("crates/lca-ui/src/transcript/mod.rs");

    // `clear` is the replacement, and it is the only thing that bumps.
    let clear_start = transcript
        .find("pub fn clear(&mut self) {")
        .expect("Transcript::clear exists");
    let clear_body = &transcript[clear_start..];
    let clear_end = clear_body.find("\n    }\n").expect("clear has a body");
    assert!(
        clear_body[..clear_end].contains("self.generation = self.generation"),
        "Transcript::clear must bump the generation: that bump is what tells \
         the run loop a selection no longer addresses live rows (gh #403)"
    );

    // And the accessor the loop reads exists.
    assert!(
        transcript.contains("pub fn generation(&self) -> u64"),
        "Transcript::generation() is the seam's reader"
    );

    // Appends must NOT bump it: a streaming delta keeps the earlier rows
    // exactly where they were, so the user's selection over them stays
    // valid. This is the mirror-image half of the contract.
    for append in ["fn push_notice", "fn push_raw", "fn push_user"] {
        if let Some(at) = transcript.find(&format!("pub fn {append}"))
            && let Some(end) = transcript[at..].find("\n    }\n")
            && transcript[at..at + end].contains("self.generation =")
        {
            panic!("{append} must not bump the generation: an append does not replace rows");
        }
    }
}

#[test]
fn the_reset_clears_the_click_run_too() {
    let alt = read("crates/lca-tui/src/engine/alt_screen.rs");
    let start = alt
        .find("pub fn reset_text_selection(&mut self) {")
        .expect("AltScreenRenderer::reset_text_selection exists (gh #403 gap 1)");
    let body = &alt[start..];
    let end = body.find("\n    }\n").expect("the reset has a body");
    let reset = &body[..end];

    // pi's `resetTextSelection()` drops the selection *and* `lastClick`.
    // Leaving `click_count` at 2 or 3 lets the next single click continue the
    // old run and select a word or a whole line the user never asked for.
    for required in [
        "self.selection.clear()",
        "self.click_count = 0",
        "self.last_click = None",
    ] {
        assert!(
            reset.contains(required),
            "the reset must include `{required}`; a selection is more than \
             rectangles, and a stale click run corrupts the next gesture"
        );
    }
}

#[test]
fn the_run_loop_resets_at_one_seam_not_at_call_sites() {
    let run = read("crates/lca-ui/src/run.rs");
    let selection = read("crates/lca-ui/src/run_selection.rs");

    // The decision is a function, so the loop and the tests answer it the
    // same way, and the comparison is not buried in the loop body.
    assert!(
        selection.contains("fn reset_selection_if_replaced("),
        "the seam decision must be one function (gh #403 gap 1)"
    );
    assert!(
        run.contains("reset_selection_if_replaced("),
        "the run loop must call the seam"
    );

    // The seam compares a generation rather than being sprinkled over the
    // five replacement paths: four call sites is how the fifth gets missed.
    assert!(
        selection.contains("chat.transcript.generation()"),
        "the seam decides on the transcript generation, not on which command ran"
    );
}

// ---------------------------------------------------------------------------
// gh #403 gap 2: every exit tells you how to come back.
//
// The fullscreen-exit path already named the session id (gh #82), but a user who
// quit any other way got nothing - so "how do I get back to this?" had no answer
// in the transcript. The fix prints a pasteable resume command on exit.
//
// The whole difficulty is **conditional honesty**: a hint shown when the command
// would fail is worse than no hint, because it teaches the user the tool lies. So
// the formatter is one function with three gates, and these rows pin the gates
// rather than the wording:
//
//   1. a TTY (a piped/headless run must gain zero bytes - scripted users parse it)
//   2. a session that was actually saved (a run that died before `session-start`
//      has no id to resume)
//   3. a non-default session dir gets shell-quoted, so the printed line is
//      pasteable on a path with spaces
//
// One formatter, not two: the fullscreen hint and the exit hint call the same
// function, which is the #456 one-copy rule.
// ---------------------------------------------------------------------------

// Verifies: gh #403 gap 2 - **one formatter.** The fullscreen-exit hint (gh #82)
// and the new exit hint must come from the same function; two `format!`s drift.
#[test]
fn the_resume_hint_has_exactly_one_formatter() {
    let ui_run = read("crates/lca-ui/src/run.rs");

    // The formatter lives in the interface (its own module, split for the file
    // ceiling), which both crates can reach.
    let formatter = read("crates/lca-ui/src/resume_command.rs");
    assert!(
        formatter.contains("pub fn resume_command("),
        "the one formatter is `lca_ui::resume_command::resume_command`"
    );
    // The gh #82 fullscreen hint must call it rather than spell its own line.
    let fullscreen = ui_run
        .find("fn exit_hint(")
        .expect("the gh #82 hint exists");
    let body = &ui_run[fullscreen..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    assert!(
        body[..end].contains("resume_command("),
        "the fullscreen hint reuses the one formatter"
    );
    // Neither call site builds the line itself.
    let cli_close = read("crates/lca-cli/src/tui/close.rs");
    for (name, source) in [
        ("lca-ui/src/run.rs", &ui_run),
        ("lca-cli/src/tui/close.rs", &cli_close),
    ] {
        assert!(
            !source.contains("resume with: lca"),
            "{name} must not spell the hint itself - it calls the one formatter"
        );
    }
    assert!(
        cli_close.contains("exit_resume_hint("),
        "the CLI exit path calls the shared formatter"
    );
}

// Verifies: gh #403 gap 2 - **gate 1: a TTY.** A piped run must gain no bytes.
// The CLI exit path asks `IsTerminal` on stdout before printing anything.
#[test]
fn the_exit_hint_requires_a_tty_stdout() {
    let close = read("crates/lca-cli/src/tui/close.rs");
    assert!(
        close.contains("is_terminal()"),
        "the exit path checks that stdout is a terminal before building the hint"
    );
    // The check and the formatter call are in the same function, so the guard
    // cannot drift away from the print it protects.
    let seam = close
        .find("fn exit_resume_hint(")
        .expect("the hint builder exists");
    let body = &close[seam..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    assert!(
        body[..end].contains("is_terminal()"),
        "the TTY check is inside the builder, beside the call it guards"
    );
    // Gate 2 is a filesystem question, not "does an id exist": a lazy session
    // has an id from the moment the interface opens but no log until the first
    // record, and `SessionStore::close` uses this same test.
    assert!(
        body[..end].contains("log_path().is_file()"),
        "the hint asks whether the session was actually written"
    );
}

// Verifies: gh #403 gap 2 - **gate 3: shell quoting.** The formatter quotes a
// non-default session dir, and leaves the default alone so the common command
// stays clean.
#[test]
fn a_non_default_session_dir_is_shell_quoted() {
    let source = read("crates/lca-ui/src/resume_command.rs");
    assert!(
        source.contains("fn shell_quote("),
        "the quoting helper exists"
    );
    let start = source
        .find("pub fn resume_command(")
        .expect("the formatter exists");
    let body = &source[start..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("shell_quote"),
        "the formatter routes the dir through a quoting helper: {body}"
    );
}

// Verifies: gh #403 gap 2 - **gate 2: the session was really saved.** The exit
// hint is printed after `close()`, which is what actually persists; printing
// before it would advertise sessions that a failed close never wrote.
#[test]
fn the_hint_is_printed_after_the_session_is_closed() {
    let close = read("crates/lca-cli/src/tui/close.rs");
    let body = close
        .find("pub(super) fn close(&self)")
        .expect("the close-out exists");
    let tail = &close[body..];
    let written = tail
        .find("self.store.close(&session)")
        .expect("close persists each opened session");
    let hint = tail
        .find("exit_resume_hint()")
        .expect("the hint is built in this seam");
    assert!(
        written < hint,
        "the hint follows the save: a session that failed to write must not be advertised"
    );
    // And nothing else prints it, so the seam cannot be bypassed by a call site.
    let mod_rs = read("crates/lca-cli/src/tui/mod.rs");
    assert!(
        !mod_rs.contains("println!(\"{hint}\")"),
        "the print lives in close(), not at the exit call sites"
    );
}

// Verifies: gh #403 gap 2 - **the id comes from the save, not the launcher.** The
// hint names the session that was actually written, reached through the opened
// set, so a `/tree` branch switch or a `/resume` inside the session names the
// session the user was last in rather than the one they started in.
#[test]
fn the_hint_names_the_session_that_was_saved() {
    let source = read("crates/lca-cli/src/tui/close.rs");
    let seam = source
        .find("fn exit_resume_hint(")
        .expect("the hint builder exists");
    let body = &source[seam..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    // `self` and the field are on separate lines (rustfmt splits a long
    // method chain), so match the field name rather than one spelling of the
    // access.
    assert!(
        body[..end].contains("current_session"),
        "the exit hint reads the session the run ended in, not the launch argument"
    );
    assert!(
        !body[..end].contains("resume:"),
        "and not the launch argument, which may name a session already left"
    );
}
