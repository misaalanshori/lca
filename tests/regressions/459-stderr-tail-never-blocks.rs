//! gh #459 - the error path must not hang before it can show the error.
//!
//! `native.rs` wraps every handshake failure with `stderr_tail(...)` so a chatty
//! crash explains itself, and `read_up_to` **blocks** on a live child with an empty
//! pipe - no EOF, no data. So the path that exists to surface an error hung before
//! it could surface one: the worker never landed a row, and the placeholder stayed
//! `Connecting` until the budget expired and then still said `Connecting`.
//!
//! The irony is the point, and it is why this guard is structural as well as
//! behavioural. Three things must stay true:
//!
//! 1. **The blocking read stays the ABI's contract.** `read-stderr` is a WIT import;
//!    an extension asking for output waits for it, and that must not change. The
//!    fix is a *second*, bounded path - not a quiet change of semantics under the
//!    first.
//! 2. **The tail read uses the bounded one.** A future edit that reaches for
//!    `process_read_stderr` "because it returns more" reintroduces the hang.
//! 3. **The doc says what it guarantees.** "Gives us its partial output" was true
//!    only once the child had written something; it is the sentence a future reader
//!    would trust, and it is corrected.
//!
//! The behaviour rows are in `crates/lca-mcp/tests/silent_failure.rs` (a silent live
//! server lands `Failed`; an empty live pipe returns without waiting; a partial line
//! comes back; a child that wrote and exited keeps its tail) and the unchanged gh
//! #418 rows in `crates/lca-mcp/tests/stdio_spawn.rs` pin the manager-level chatty
//! and no-stderr cases.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #459 - the bounded read exists, is exported, and maps the three
// states distinctly. `Some(empty)` ("nothing yet") is the state the blocking read
// cannot express, and it is the whole fix.
#[test]
fn there_is_a_bounded_read_beside_the_blocking_one() {
    let process = read("crates/lca-tools/src/process.rs");
    assert!(
        process.contains("pub fn read_buffered("),
        "a non-blocking read exists beside `read_up_to`"
    );
    // Both halves are present, because non-blocking I/O is platform-specific and a
    // Unix-only fix would leave the Windows leg hanging.
    assert_eq!(
        process.matches("pub fn read_buffered(").count(),
        2,
        "one implementation per platform (unix + windows)"
    );
    assert!(
        process.contains("#[cfg(unix)]") && process.contains("#[cfg(windows)]"),
        "and each is gated to its platform"
    );
    // The doc names the distinction that matters.
    let doc = process
        .split("/// Read whatever is **already buffered**")
        .nth(1)
        .expect("the documented bounded read");
    let doc = doc.split("pub fn read_buffered").next().unwrap_or(doc);
    assert!(
        doc.contains("`Ok(Some(empty))`"),
        "the doc names the empty-not-EOF state: {doc}"
    );
    assert!(
        doc.contains("blocks on a live child with an empty pipe"),
        "and records the hang it exists to prevent"
    );
    // The blocking one keeps its contract and says so, rather than looking like a
    // legacy duplicate a tidy-up could delete.
    let blocking = process
        .split("/// Read up to `max` bytes, **blocking**")
        .nth(1)
        .expect("the blocking read is still documented as blocking");
    let blocking = blocking
        .split("pub fn read_up_to")
        .next()
        .unwrap_or(blocking);
    assert!(
        blocking.contains("ABI's `read-stderr`"),
        "the blocking read is the ABI's contract, not an oversight: {blocking}"
    );
}

// Verifies: gh #459 - the ABI's `read-stderr` semantics are untouched.
//
// An extension that calls `read-stderr` must still wait for output. Turning that
// blocking call non-blocking would "fix" this bug by breaking every extension that
// reads a child's error output.
#[test]
fn the_abi_read_stderr_is_still_blocking() {
    // The methods live in their own module after the gate-11 split; the guard
    // follows them rather than pinning a path that a future split would move again.
    let capabilities = read("crates/lca-tools/src/capabilities/process_io.rs");
    let blocking = capabilities
        .split("pub fn process_read_stderr(")
        .nth(1)
        .expect("the ABI seam exists");
    let blocking = blocking.split("\n    }\n").next().unwrap_or(blocking);
    assert!(
        blocking.contains("read_up_to"),
        "`process_read_stderr` still uses the blocking read: {blocking}"
    );
    assert!(
        !blocking.contains("read_buffered"),
        "the ABI path did not become non-blocking"
    );
    // And the bounded variant is a separate method that says why.
    let bounded = capabilities
        .split("pub fn process_read_stderr_buffered(")
        .nth(1)
        .expect("the bounded seam exists");
    let bounded = bounded.split("\n    }\n").next().unwrap_or(bounded);
    assert!(
        bounded.contains("read_buffered"),
        "the bounded seam uses the bounded read"
    );
    assert!(
        capabilities.contains("gh #459"),
        "and the distinction records the defect that forced it"
    );
}

// Verifies: gh #459 design point 2 - the tail read is bounded **for a live child**.
//
// This is the one-line regression that would bring the hang back: reaching for the
// blocking read unconditionally because it "returns more output".
//
// The rule is not "never block", though - it is "only block when EOF is guaranteed".
// A child that has **exited** will close its pipe, so draining it blocking is both
// safe and complete; a live one may never close it, so only what is already
// buffered is available. Both halves are pinned below, because either one alone is
// a bug in the other direction (a hang, or lost last words).
#[test]
fn the_stderr_tail_read_blocks_only_when_eof_is_guaranteed() {
    let native = read("crates/lca-mcp/src/stdio_tail.rs");
    let tail = native
        .split("fn read_stderr_tail(")
        .nth(1)
        .expect("the tail reader exists");
    let tail = tail.split("\n}\n").next().unwrap_or(tail);

    // The exited check gates the choice.
    assert!(
        tail.contains("process_has_exited"),
        "the reader asks whether EOF is guaranteed: {tail}"
    );
    // A live child takes the bounded path.
    assert!(
        tail.contains("process_read_stderr_buffered"),
        "a live child's read is bounded, which is the fix"
    );
    // A dead one may drain blocking, and that branch must be the *exited* one.
    let exited_branch_starts = tail
        .find("if exited")
        .expect("the choice is made on `exited`");
    let blocking_at = tail
        .find("process_read_stderr(")
        .expect("the dead-child branch drains to EOF");
    assert!(
        exited_branch_starts < blocking_at,
        "the blocking read sits inside the `if exited` branch, where EOF is \
         guaranteed - not on the live path"
    );
    // And the reason is recorded, because "why is a blocking read allowed here" is
    // the question the next reader will have.
    assert!(
        tail.contains("EOF is guaranteed"),
        "the branch says why blocking is safe there"
    );
}

// Verifies: gh #459 design point 5 - the doc comment that made this possible is
// corrected, and says what the function guarantees now.
#[test]
fn the_doc_comment_no_longer_promises_partial_output() {
    let native = read("crates/lca-mcp/src/stdio_tail.rs");
    // Slice the whole doc block, from its first line to the `fn` it belongs to.
    let doc = native
        .split("/// Read what the child has **already** written to stderr")
        .nth(1)
        .expect("the corrected doc");
    let doc = doc.split("fn read_stderr_tail(").next().unwrap_or(doc);
    let doc = format!("/// Read what the child has **already** written to stderr{doc}");
    assert!(
        doc.contains("without waiting"),
        "the guarantee is stated: no waiting"
    );
    assert!(
        doc.contains("Some(empty)"),
        "and the empty-but-open state is named"
    );
    // The old sentence is recorded as the thing that was wrong, not left standing.
    assert!(
        doc.contains("true only once it") && doc.contains("has *written* something"),
        "the old claim is corrected where a reader will find it: {doc}"
    );
}

// Verifies: gh #459 design point 3 - `connecting` after the budget carries the
// deadline that passed.
//
// The issue's second ask: a status label that can persist past every deadline is a
// label that lies. The state stays `Connecting` (the worker may genuinely still be
// handshaking, and claiming otherwise would be its own lie), but the report says
// what happened - it did not answer in time.
#[test]
fn connecting_past_the_budget_says_so() {
    let cli = read("crates/lca-mcp/src/cli.rs");
    let connecting = cli
        .split("crate::ServerStateKind::Connecting =>")
        .nth(1)
        .expect("the connecting arm");
    let connecting = connecting
        .split("crate::ServerStateKind::NeedsSignIn")
        .next()
        .unwrap_or(connecting);
    assert!(
        connecting.contains("still_connecting.contains"),
        "the arm consults what the settle reported as still pending: {connecting}"
    );
    assert!(
        connecting.contains("did not answer within"),
        "and says the budget passed rather than printing a bare label: {connecting}"
    );
    assert!(
        cli.contains("let still_connecting = manager.wait_until_settled(budget);"),
        "the pending set is captured from the settle, not recomputed from a guess"
    );
}

// Verifies: gh #459 design point 6 - no new dependency. The platform primitives
// were already in the tree for the pty capability, and the fix reuses them.
#[test]
fn the_fix_adds_no_dependency() {
    let manifest = read("crates/lca-tools/Cargo.toml");
    // Both were already direct dependencies for `pty.rs`.
    assert!(manifest.contains("libc"), "libc was already present");
    assert!(
        manifest.contains("windows-sys"),
        "windows-sys was already present"
    );
    // The bounded read uses them, rather than a crate the tree did not have.
    let process = read("crates/lca-tools/src/process.rs");
    assert!(
        process.contains("libc::O_NONBLOCK") && process.contains("PeekNamedPipe"),
        "the fix uses the existing primitives on both platforms"
    );
}
