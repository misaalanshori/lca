//! GitHub #368: three ways `StdinBuffer` corrupts typed input. NFR-24's guard.
//!
//! The behavioural rows live beside the code they drive
//! (`crates/lca-tui/src/engine/stdin_buffer/recovery_tests.rs`, 12 rows, every one
//! driven through the real buffer and the last step through the real editor) and
//! the #366 reply-fragment rows next to them. This file pins the **shape** from
//! outside, because all three bugs were silent: a frozen session, text in a prompt,
//! and a duplicated character have no error to grep for.
//!
//! The three rules that must not drift:
//!
//! 1. an introducer with no terminator is a **keystroke plus typing**, not a
//!    sequence;
//! 2. a head that **committed** to a reply we asked for still **drops**, tail and
//!    all (#366's property, re-pinned against this change);
//! 3. `DCS`/`APC` end on `ST` and not on `BEL`, and the kitty printable grammar
//!    accepts the colon sub-parameters LCA itself negotiates.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The module's code, without its test modules - two `#[path]` blocks and one
/// inline `mod tests`, so the cut is the **first** `#[cfg(test)]`.
fn source() -> String {
    let text = read("crates/lca-tui/src/engine/stdin_buffer.rs");
    match text.find("\n#[cfg(test)]\n") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

// Verifies: gh #368 bug 2 - **the recovery is a split, not a delivery.**
//
// The bug was a glued event, so a fix that simply stopped dropping the remainder
// (delivering `\x1b]abc` as one string) would pass a row that only asserted "the
// characters arrived" and still leave the editor unable to read the key. The shape
// that matters is the split at the two introducer bytes, then the rest through the
// buffer again.
#[test]
fn the_alt_key_recovery_splits_the_introducer_and_reparses() {
    let source = source();
    let body = function_body(&source, "fn revoke_alt_key(");
    // The two introducer bytes are emitted **alone**, as their own event.
    assert!(
        body.contains("rest[..2].to_string()"),
        "the introducer is its own two-byte event: {body}"
    );
    assert!(
        body.contains("StdinBuffer::new()") && body.contains("tail.process("),
        "and the tail is reparsed through a buffer, so a `Ctrl+M` in it is still \
         Enter rather than two literal characters: {body}"
    );
    // A **loop**, not recursion: the tail is itself input and may be another
    // introducer. Recursion aborted the process on a burst (stack overflow) and a
    // fresh buffer per pass was O(n^2); both are recorded in the function's own
    // comment, and the loop is the shape that is neither.
    assert!(
        body.contains("loop {"),
        "the recovery iterates rather than recursing: {body}"
    );
    // The body includes the signature itself, so "no call" is "mentioned once".
    assert_eq!(
        body.matches("revoke_alt_key(").count(),
        1,
        "and never calls itself: {body}"
    );
    for intro in ["\\x1b]", "\\x1bP", "\\x1b_"] {
        assert!(
            body.contains(intro),
            "the recovery covers {intro:?}: {body}"
        );
    }
    assert!(
        !body.contains("\"\\x1b[\""),
        "and deliberately not `ESC [`: a bare one is `ctrl+alt+[` *and* the head of \
         a half-arrived arrow key, so splitting it trades one ambiguity for \
         another: {body}"
    );
}

// Verifies: gh #368 bug 1's second half - **`DCS` and `APC` are reply families.**
//
// Their absence from `ReplyKind` is what made the leak: the head was delivered as
// text and the body typed. A future refactor that prunes "unused" variants takes
// the leak back, so the variants are named and the commitment points pinned.
#[test]
fn the_delimited_string_replies_are_in_the_table() {
    let source = source();
    let kind = source
        .get(source.find("enum ReplyKind").expect("the enum exists")..)
        .expect("sliced");
    for variant in ["XtVersion", "KittyGraphics"] {
        assert!(
            kind.contains(variant),
            "{variant} is a reply we can elicit and must stay in the table"
        );
    }
    let commit = function_body(&source, "fn reply_matches_so_far(");
    assert!(
        commit.contains("rest.starts_with(\">|\")"),
        "`DCS` commits at `>|` (pi's header pattern), past the bare introducer, so \
         `Alt+P` stays recoverable: {commit}"
    );
    assert!(
        commit.contains("graphics_header_committed("),
        "and `APC` at its first `key=value`: {commit}"
    );
}

// Verifies: gh #368 design call 4 - **the terminator table.**
//
// `BEL` is an ordinary byte inside a `DCS`/`APC` body, so accepting it there would
// cut a version reply in half and type the rest. Asserted on the predicate rather
// than through a row, because the predicate is what a new family would be added to.
#[test]
fn only_the_osc_families_end_on_bel() {
    let source = source();
    let body = function_body(&source, "fn terminates_on_bel(");
    assert!(
        body.contains("Osc11") && body.contains("ProgramStatus"),
        "the two OSC families end on BEL: {body}"
    );
    for absent in [
        "XtVersion",
        "KittyGraphics",
        "DeviceAttributes",
        "KittyFlags",
    ] {
        assert!(
            !body.contains(absent),
            "{absent} must not be in the BEL set: {body}"
        );
    }
    // And completion says the same thing: `DCS`/`APC` wait for ST only.
    let complete = function_body(&source, "fn is_complete_sequence(");
    assert!(
        complete.contains("ends_with(\"\\x1b\\\\\")"),
        "both delimited strings end on ST: {complete}"
    );
}

// Verifies: gh #368 bug 3 - **the colon sub-parameters are part of the grammar.**
//
// Flag `4` is negotiated by LCA (`KITTY_QUERY`), so rejecting the format it
// produces is self-inflicted. The guard names the fields the parser must accept, so
// a future "simplification" back to digits-only fails here rather than shipping the
// duplicate again.
#[test]
fn the_kitty_printable_parser_accepts_the_colon_form() {
    let source = source();
    let body = function_body(&source, "fn parse_unmodified_kitty_printable_codepoint(");
    assert!(
        body.contains("for field in 0..2"),
        "two optional `:digits` fields, pi's grammar: {body}"
    );
    assert!(
        body.contains("b':'"),
        "which means looking for the colon: {body}"
    );
    // The codepoint still has to be printable, colon form or not.
    assert!(
        body.contains("codepoint >= 32"),
        "and the printable bound survives: {body}"
    );
    // Declining rather than guessing stays the rule for everything else.
    assert!(
        body.contains("return None"),
        "a malformed sub-parameter declines: {body}"
    );
}

// Verifies: gh #368 bug 2's fix did not undo #366 - **the two directions are one
// function apart and in that order.**
//
// `flush` asks `is_reply_prefix` *first*; only a remainder that has committed to
// nothing reaches the keystroke recovery. A refactor that swapped them would turn
// every split reply into typing - #366's bug, in the name of #368's fix.
#[test]
fn a_committed_reply_head_is_still_dropped_before_the_keystroke_recovery() {
    let source = source();
    let flush = function_body(&source, "pub fn flush(&mut self)");
    let drop = flush
        .find("is_reply_prefix(&seq)")
        .unwrap_or_else(|| panic!("flush consults the prefix predicate: {flush}"));
    let recover = flush
        .find("revoke_alt_key(")
        .unwrap_or_else(|| panic!("and falls through to the recovery: {flush}"));
    assert!(
        drop < recover,
        "the reply check must come first, or #366's leak returns: {flush}"
    );
    assert!(
        flush[drop..recover].contains("return Vec::new()"),
        "and a committed head returns without emitting anything: {flush}"
    );
}
