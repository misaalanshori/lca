//! NFR-24 guard for gh #569: a blocked pipe operation never holds the handle table.
//!
//! ## The defect
//!
//! `process_read_stdout` / `process_read_stderr` / `process_read_stderr_buffered` /
//! `process_write_stdin` each took `self.handles`, looked the entry up, and then did
//! the pipe I/O **with the lock still held**. A stdout read of a silent child blocks
//! until the child writes or closes - so the table was held for the child's whole
//! lifetime, and `process_kill` needs that table. Cancellation, a timeout and
//! `mcp`'s `Drop` teardown all reach for `process_kill`: a reader waiting for
//! output, a killer waiting for the lock, and a child waiting for stdin. Measured
//! through the shape itself, `process_kill` had not returned after **three seconds**
//! (`cat` mid-read); the stdin half measured the same with `sleep` and a 512 KiB
//! write.
//!
//! ## What this pins
//!
//! The behaviour is in `crates/lca-tools/tests/process_hold_gap.rs` - a kill
//! returns while a stdout read blocks, a kill returns while a stdin write blocks, a
//! killed child's tree is gone, bytes read before and after a kill are delivered
//! once in order, and a wrong-type handle is refused without being taken. Those rows
//! fail on the pre-fix tree (two of them at the 10 s hang guard, with the message
//! naming which side stalled) and pass now.
//!
//! What is left here is the one claim a behavioural row cannot state: **the lock is
//! not held across the I/O in the first place.** A future refactor could hold the
//! lock again and still pass every behaviour row on a fast machine - the deadlock
//! needs the child to stay silent long enough - so the discipline is pinned
//! structurally, by the order of the three steps in each method.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::Path;

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|err| panic!("read {rel}: {err}"))
}

const SOURCE: &str = "crates/lca-tools/src/capabilities/process_io.rs";

/// The body of one method, sliced from its `pub fn` to the closing `    }` that
/// ends it. Slicing by the next top-level `fn` would be wrong: the helpers that
/// follow these methods are not part of them.
fn method_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` exists in {SOURCE}"));
    let body = &source[start..];
    let end = body
        .find("\n    }\n")
        .unwrap_or_else(|| panic!("`{signature}` is a method body ending with an indented `}}`"));
    &body[..end]
}

// Verifies: gh #569 - every stream method takes the pipe, uses it, and puts it
// back, in that order, and none of them holds the table itself.
//
// The order is the whole discipline: a `read_up_to` between the two `take_pipe`
// halves is a read with no lock held; a read before the take, or after the
// return, would be a read of a pipe the table might have handed to someone else.
#[test]
fn the_stream_methods_take_use_and_return_the_pipe_outside_the_table() {
    let source = read(SOURCE);
    for (label, signature, use_call) in [
        (
            "process_read_stdout",
            "pub fn process_read_stdout(",
            "crate::process::read_up_to(",
        ),
        (
            "process_read_stderr",
            "pub fn process_read_stderr(",
            "crate::process::read_up_to(",
        ),
        (
            "process_read_stderr_buffered",
            "pub fn process_read_stderr_buffered(",
            "crate::process::read_buffered(",
        ),
        (
            "process_write_stdin",
            "pub fn process_write_stdin(",
            "crate::process::write_all(",
        ),
    ] {
        let body = method_body(&source, signature);
        assert!(
            !body.contains("lock(&self.handles)"),
            "`{label}` must not lock the handle table in its own body - the \
             table is taken and released inside the helpers, so the I/O runs \
             with no lock held (gh #569): {body}"
        );
        let take = body
            .find("self.take_pipe(")
            .unwrap_or_else(|| panic!("`{label}` takes the pipe out of the table: {body}"));
        let used = body
            .find(use_call)
            .unwrap_or_else(|| panic!("`{label}` performs its I/O: {body}"));
        let back = body.find("self.return_pipe(").unwrap_or_else(|| {
            panic!("`{label}` puts the pipe back (or leaves it absent): {body}")
        });
        assert!(
            take < used && used < back,
            "`{label}` runs its I/O **between** the take and the return, so no \
             lock is held across it (gh #569): take={take} io={used} back={back}"
        );
    }
}

// Verifies: gh #569 - the helpers are the only place the table is locked, and the
// return path tolerates a handle that is gone.
//
// "Put it back, or leave it absent": a concurrent destroy (`process_kill` or a
// reaping `process_wait`) is legal, so `return_pipe` must not report an error and
// must not resurrect a pipe into a handle nobody owns. The behavioural half is
// `bytes_read_before_and_after_a_kill_are_delivered_once` and the trailing
// `NotFound` assertions in `crates/lca-tools/tests/process_hold_gap.rs`.
#[test]
fn the_helpers_scope_the_lock_and_a_missing_handle_simply_gets_no_pipe() {
    let source = read(SOURCE);

    let take = method_body(&source, "fn take_pipe<T>(");
    assert!(
        take.contains("lock(&self.handles)"),
        "the take helper locks the table once, to move the pipe out"
    );
    assert!(
        take.trim_end().ends_with("})") || take.contains("fill(child).ok_or_else"),
        "and it returns the pipe it found (the guard drops at the end of the \
         helper, not at the end of the caller): {take}"
    );

    let back = method_body(&source, "fn return_pipe<T>(");
    assert!(
        back.contains("lock(&self.handles)"),
        "the return helper locks the table once, to put it back"
    );
    assert!(
        back.contains("if let Some(slot)"),
        "and a handle that is gone is a no-op, not an error (gh #569): {back}"
    );
    assert!(
        !back.contains("Err(") && !back.contains("return Err"),
        "the return path cannot fail: nothing is left to report to a caller \
         whose pipe has already been used: {back}"
    );
    // The type check happens before anything is taken, so a wrong-type handle is
    // never touched - the rule gh #318 established for the same entry type.
    assert!(
        take.contains("is not a process"),
        "the take helper refuses another handle kind before taking anything: {take}"
    );
}

// Verifies: gh #569 - the module says the rule where a future editor will read it,
// and the reason is the deadlock rather than "cleaner code".
//
// A refresh without the reason is how the lock comes back: the next reader sees a
// lock/unlock dance that looks like it can be collapsed.
#[test]
fn the_module_documents_the_rule_and_why_it_exists() {
    let source = read(SOURCE);
    let doc = source
        .split("# The table is never held across a blocking pipe operation (gh #569)")
        .nth(1)
        .expect("the rule is documented in the module header");
    let doc = doc.split("use super::").next().unwrap_or(doc);
    // Line-wrapped prose, so compare on one line: strip the `//!` prefixes and
    // collapse whitespace, or a phrase that straddles a wrap reads as missing.
    let doc = doc
        .lines()
        .map(|line| line.trim_start_matches("//!").trim())
        .collect::<Vec<_>>()
        .join(" ");
    for phrase in [
        "take the pipe out of the handle",
        "no lock held",
        "put the pipe back",
        "single owner",
        "process_kill",
    ] {
        assert!(
            doc.contains(phrase),
            "the module doc states `{phrase}`, which is the rule a future edit \
             would otherwise have to guess: {doc}"
        );
    }
    // And the neighbouring contracts are named, so a reader knows which rows sit on
    // top of this file without leaving it.
    assert!(
        source.contains("gh #459") && source.contains("gh #318"),
        "the file names the contracts it already carries"
    );
}

// Verifies: gh #569 - the two rows that pin the neighbouring contracts were not
// edited.
//
// `459` pins the blocking/non-blocking stderr distinction (an ABI contract), and
// `318` pins wait/kill releasing the handle. Both are adjacent to this change and
// neither may be quietly relaxed to make it pass; their text is compared to what
// the rule they encode says, not to a hash (a hash would fail on any comment
// typo and teach nobody anything).
#[test]
fn the_neighbouring_guards_still_state_their_contracts() {
    let tail = read("tests/regressions/459-stderr-tail-never-blocks.rs");
    // Two asserts rather than one `&& ... || ...`: that spelling passed even when
    // the first needle was missing, which is a guard that cannot fail (found by
    // reading it back, not by running it).
    assert!(
        tail.contains("pub fn process_read_stderr("),
        "459 still slices the ABI's blocking stderr reader"
    );
    assert!(
        tail.contains("read_up_to"),
        "and still requires that reader to be the blocking one"
    );
    assert!(
        tail.contains("read_buffered"),
        "and still requires the bounded variant to exist separately"
    );

    let wait = read("tests/regressions/318-process-and-pty-wait-remove-handles.rs");
    assert!(
        wait.contains("fn a_completed_wait_releases_the_process_handle()"),
        "318 still pins that a wait releases the handle"
    );
    assert!(
        wait.contains("fn the_read_to_eof_wait_then_kill_lifecycle_still_works()"),
        "and the read/wait/kill lifecycle this change rewires"
    );
}
