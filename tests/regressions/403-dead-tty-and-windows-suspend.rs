//! GitHub #403 gap 4, first half: a dead tty hangs or lies, and `Ctrl+Z` is silent
//! on Windows. NFR-24's named guard.
//!
//! The behavioural rows live beside the code they drive:
//! `crates/lca-tui/src/engine/signals.rs` (the exit code, the flag both directions
//! write, the platform predicate), `crates/lca-tui/src/engine/sys.rs` (the `poll`
//! classification), `crates/lca-ui/src/run_terminal_tests.rs` (the hangup question,
//! the suspend refusal, the dispose-before-touch ordering, the restarted reader) and
//! `crates/lca-cli/tests/e2e_hangup.rs` (a **real pty** closed under a **real**
//! binary, plus the `SIGHUP` route with `SIGHUP` blocked so only the reader's own
//! detection can pass it).
//!
//! This file guards the shapes that would let either half come back silently.
//! "Silently" is the operative word for both: the pty bug hung rather than errored,
//! and the Windows key did nothing rather than refusing.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

// Verifies: gh #403 gap 4 - **`HUP` outranks `IN`.** A closed pty reports
// `POLLIN|POLLHUP` together, so a classification ordered the other way answers "a key
// is waiting" forever and the reader spins - the defect, one layer below where it was
// found.
//
// Anchored on the classifier rather than on `wait_stdin`, because that is where the
// rule lives: a `wait_stdin` that calls it in either order would pass a looser guard.
#[test]
fn the_hangup_flags_are_checked_before_readiness() {
    let source = read("crates/lca-tui/src/engine/sys.rs");
    let body = function_body(&source, "fn classify_poll_revents(");
    // Compared as the `revents & …` *tests*, not as the first mention of each flag:
    // the `GONE` constant names all three at the top of the function, so a text
    // search for "POLLHUP" would still come first after the branches were swapped.
    let gone = body
        .find("revents & GONE")
        .unwrap_or_else(|| panic!("the classifier tests the gone flags: {body}"));
    let readable = body
        .find("revents & libc::POLLIN")
        .unwrap_or_else(|| panic!("and readiness separately: {body}"));
    assert!(
        gone < readable,
        "the `gone` check must come first, or a hung-up fd reads as a permanent key: {body}"
    );
    for flag in ["POLLERR", "POLLNVAL"] {
        assert!(
            body.contains(flag),
            "a pty can also report {flag}, which is the same answer: {body}"
        );
    }
}

// Verifies: gh #403 gap 4 - **the reader treats end-of-file as a hangup, not as
// "nothing yet".**
//
// This is the other route to the same spin, and the one a `POLLIN`-only poll actually
// took: `read` returns 0, and a loop that ignores it waits again on a terminal that
// will answer 0 forever.
#[test]
fn end_of_file_is_a_hangup_in_the_reader() {
    // Gh #522 split the reader loop into its own module for the line ceiling, so
    // this row follows the code rather than the filename (the same rule #400's
    // guard took after its split).
    let source = read("crates/lca-tui/src/engine/reader_loop.rs");
    let body = function_body(&source, "pub(crate) fn reader_loop(");
    let eof = body
        .find("Ok(0) => {")
        .unwrap_or_else(|| panic!("the reader special-cases an empty read: {body}"));
    let window = &body[eof..(eof + 260).min(body.len())];
    assert!(
        window.contains("hangup"),
        "an end-of-file read must mark the terminal gone rather than loop: {window}"
    );
    assert!(
        window.contains("break"),
        "and leave the loop, or the spin remains: {window}"
    );
    assert!(
        body.contains("is_hangup()"),
        "and the poll outcome is consulted at all: {body}"
    );
}

// Verifies: gh #403 gap 4 - **the dispose-before-touch ordering.**
//
// Every terminal write passes one gate, and `stop` skips its own sequences on a
// hung-up terminal. The bug was not the exit code; it was a shutdown path that kept
// writing to an fd that was no longer a terminal, where each write either blocks or
// raises again.
#[test]
fn nothing_is_written_after_the_terminal_is_gone() {
    let source = read("crates/lca-tui/src/engine/terminal.rs");
    let write = function_body(&source, "fn raw_write(");
    assert!(
        write.contains("hangup.load("),
        "the one write gate checks the flag: {write}"
    );
    // The **body**, not the trait declaration of the same name (`terminal.rs` has
    // both, and the declaration comes first).
    let stop = function_body(&source, "fn stop(&mut self) {");
    let check = stop
        .find("hangup.load(")
        .unwrap_or_else(|| panic!("`stop` checks the flag: {stop}"));
    let restore = stop
        .find("restore_raw_mode(")
        .unwrap_or_else(|| panic!("and still restores mode on a live terminal: {stop}"));
    assert!(
        check < restore,
        "the check must precede the restore, and the restore live behind it: {stop}"
    );
    assert!(
        stop[check..restore].contains("return"),
        "a hung-up terminal returns before touching anything: {stop}"
    );
}

// Verifies: gh #403 gap 4 - **both directions reach one decision.**
//
// The reader sets a per-terminal flag; the signal handler can only set an atomic, so
// the loop copies that in. A rule written as `&&` would pass a row that only ever set
// both and would then hang on whichever shape set one - so the shape is `||`, and the
// loop asks exactly one question.
#[test]
fn both_hangup_directions_reach_one_question() {
    let source = read("crates/lca-ui/src/run_terminal.rs");
    let body = function_body(&source, "pub(crate) fn hangup_decision(");
    assert!(
        body.contains("terminal_hangup || signal_requested"),
        "either direction alone is enough: {body}"
    );
    let loop_source = read("crates/lca-ui/src/run.rs");
    assert!(
        loop_source.contains("crate::run_terminal::hangup_requested("),
        "and the loop asks the one question rather than either cell directly"
    );
    assert!(
        loop_source.contains("signals::EXIT_CODE"),
        "and exits with the documented code, not a literal"
    );
}

// Verifies: gh #403 gap 4 - the exit code is the **documented** one, and the doc and
// the constant agree.
//
// A code invented at the call site ("7") would be indistinguishable from a usage
// error to every script that reads it, and a doc row that drifted from the constant
// would be worse than no row at all.
#[test]
fn the_dead_terminal_code_is_the_documented_one() {
    let source = read("crates/lca-tui/src/engine/signals.rs");
    let body = function_body(&source, "pub const EXIT_CODE");
    assert!(body.contains("129"), "the constant is 128 + SIGHUP: {body}");
    let doc = read("docs/headless.md");
    assert!(
        doc.contains("| 129 |"),
        "and the exit-code table has the row"
    );
    assert!(
        doc.contains("128 + SIGHUP"),
        "which says where the number comes from"
    );
}

// Verifies: gh #403 gap 4 - **a platform without job control refuses rather than
// tearing the frame down.**
//
// The refusal is a pure predicate whose platform answer is a parameter, because a
// `cfg(windows)` row cannot run on the machine this suite runs on. What the guard
// adds is the ordering it pins: the loop asks *before* it calls `suspend`, so a
// refusal cannot leave the alternate screen behind.
#[test]
fn the_suspend_refusal_is_checked_before_anything_is_torn_down() {
    let source = read("crates/lca-ui/src/run.rs");
    let body = function_body(&source, "Action::Suspend => {");
    let refusal = body
        .find("suspend_refusal(")
        .unwrap_or_else(|| panic!("the refusal is asked: {body}"));
    let suspend = body
        .find("run_terminal::suspend(")
        .unwrap_or_else(|| panic!("and the suspend is what runs otherwise: {body}"));
    assert!(
        refusal < suspend,
        "the refusal must be decided first, or an unsupported platform still tears down: {body}"
    );
    assert!(
        body.contains("chat.say("),
        "and it is said, not swallowed - silence is the defect: {body}"
    );
}

// Verifies: gh #403 gap 4 - the suspend is a **registered binding**, so it is listed
// by `/hotkeys` and an embedder can rebind it.
//
// A hard-coded byte test in the key handler would make the key work and leave it
// invisible and un-rebindable - the `key_text` lie gh #410 removed, in the other
// direction.
#[test]
fn the_suspend_key_is_in_the_registry() {
    let source = read("crates/lca-tui/src/engine/keybindings.rs");
    assert!(
        source.contains("\"app.suspend\""),
        "the action is in the table"
    );
    assert!(source.contains("def!(&[\"ctrl+z\"]"), "with pi's key");
    let handler = read("crates/lca-ui/src/chat.rs");
    assert!(
        handler.contains("self.keybindings.matches(data, \"app.suspend\")"),
        "and the handler asks the registry rather than comparing bytes"
    );
}

// Verifies: gh #403 gap 4 - **the fix is installed by the interactive host, not by a
// library constructor.**
//
// A disposition is process-wide and permanent. Installing it from the engine would
// make every headless run ignore `SIGHUP` - turning "the terminal left" into "keep
// going" for the mode a script uses, which is worse than the behaviour it replaces.
#[test]
fn only_the_interactive_entry_installs_the_signal_handler() {
    let host = read("crates/lca-cli/src/tui/startup.rs");
    assert!(
        host.contains("signals::install()"),
        "the interactive entry installs it"
    );
    for library in [
        "crates/lca-tui/src/engine/mod.rs",
        "crates/lca-tui/src/lib.rs",
    ] {
        assert!(
            !read(library).contains("signals::install()"),
            "{library} must not install a process-wide disposition as a side effect"
        );
    }
}

// Verifies: gh #403 gap 4 - **nothing is printed into a terminal that is gone.**
//
// This one came from CI rather than from review: the macOS leg failed the pty row
// with exit **101**, not 129. The close-out's resume hint writes to stdout, and on a
// hung-up pty that write fails and `println!` panics - so a clean dead-tty exit became
// a crash-shaped one, on the platform whose pty semantics differ. The hint now asks
// the same process-wide flag the loop sets.
//
// A guard rather than only the behavioural row, because the failure mode is a *panic*
// in a path three crates away from the detection: any future line printed on the way
// out re-opens it, and this scan is what notices.
#[test]
fn the_exit_path_prints_nothing_once_the_terminal_is_gone() {
    let host = read("crates/lca-cli/src/tui/close.rs");
    let hint = function_body(&host, "pub(super) fn exit_resume_hint(");
    let gate = hint
        .find("signals::requested()")
        .unwrap_or_else(|| panic!("the hint asks the hangup flag: {hint}"));
    let id = hint
        .find("session.id()")
        .unwrap_or_else(|| panic!("and only then builds the command: {hint}"));
    assert!(
        gate < id,
        "the gate must be ahead of everything that formats or prints: {hint}"
    );
    // And the loop is what records it for that caller, which holds no terminal.
    let runtime = read("crates/lca-ui/src/run_terminal.rs");
    let body = function_body(&runtime, "pub(crate) fn hangup_requested(");
    assert!(
        body.contains("signals::request()"),
        "the loop records the hangup process-wide so an outside caller can ask: {body}"
    );
}
