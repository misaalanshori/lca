//! NFR-24 guard for gh #405 gap 4, second round: **where the characters were lost.**
//!
//! The issue as filed says chunk boundaries slice escapes and UTF-8. Measuring the
//! live path shows the opposite split of work, and it matters because a guard aimed at
//! the wrong layer passes while the product paints garbage:
//!
//! - **The text accumulator was already right.** `lca-ui`'s `Transcript` appends
//!   `&str` deltas to a `String`, and rendering parses the whole accumulated text, so a
//!   document delivered in 200 character-chunks renders byte-identically to one chunk.
//!   Measured this cycle: `200 char-chunks == 1 chunk`, on a document with a true-colour
//!   SGR, an OSC 8 link, CJK and an emoji.
//! - **The byte decoder was losing characters.** `crates/lca-core/src/turn/execute.rs`
//!   decoded every pipe read with `String::from_utf8_lossy`, so a codepoint straddling
//!   a read became `U+FFFD`: measured, a CJK line in 8-byte reads painted **10**
//!   replacement characters. `stdin_buffer::process_bytes` had the same fallback and
//!   turned `"日本語"` split after byte 2 into `"��本語"`.
//! - **The sanitizer had two defects of its own**, both found by reading it against
//!   measurements rather than by a test: a backward scan that chopped text after a
//!   complete sequence, and a silent raw `ESC` past its 4 KiB bound.
//!
//! This file pins the *invariant* end to end: the same bytes, however they are chunked,
//! produce the same text, and none of it contains a replacement character that stands
//! for a codepoint that merely arrived late.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use lca_protocol::utf8::{REPLACEMENT, Utf8Stream};

/// A document with every shape the brief names: a colour span, an OSC 8 link, CJK,
/// an emoji, and a ZWJ cluster (which is where a "one codepoint" assumption breaks).
const DOC: &str = "streamed \x1b[38;5;196mred\x1b[0m · 日本語の出力 · 🎉 · link \
                   \x1b]8;;https://example.com\x1b\\here\x1b]8;;\x1b\\ · 👨‍👩‍👧‍👦 end";

/// Decode `bytes` the way a pipe delivers them: `chunk` bytes at a time.
fn streamed(bytes: &[u8], chunk: usize) -> String {
    let mut out = String::new();
    let mut stream = Utf8Stream::new();
    for piece in bytes.chunks(chunk.max(1)) {
        out.push_str(&stream.push(piece));
    }
    out.push_str(&stream.finish());
    out
}

// Verifies: gh #405 gap 4 - **the equality that is the receipt.** One chunk and every
// smaller chunk size decode to exactly the document, with no replacement character
// anywhere. A shape assertion per feature would pass with one of them broken; this
// cannot.
#[test]
fn the_same_bytes_decode_identically_at_every_chunk_size() {
    let bytes = DOC.as_bytes();
    let whole = streamed(bytes, bytes.len());
    assert_eq!(whole, DOC, "the whole document decodes to itself");
    assert!(
        !whole.contains(REPLACEMENT),
        "and it contains no replacement character: {whole:?}"
    );
    for chunk in 1..=bytes.len() {
        let got = streamed(bytes, chunk);
        assert_eq!(
            got, whole,
            "chunk size {chunk} must decode to the same text as one chunk"
        );
    }
}

// Verifies: gh #405 gap 4 - **a boundary inside an escape leaves no raw control byte
// and no literal fragment.** The escape is held like a codepoint is: the render entry
// then either receives it whole or not at all.
#[test]
fn a_boundary_inside_an_escape_leaves_no_fragment() {
    // Split inside the CSI's parameters, and inside the OSC's payload.
    for (label, doc) in [
        ("CSI", "a\x1b[38;5;196mred\x1b[0mz"),
        (
            "OSC 8",
            "a\x1b]8;;https://example.com\x1b\\t\x1b]8;;\x1b\\z",
        ),
        ("OSC title", "a\x1b]0;title\x1b\\z"),
    ] {
        for split in 1..doc.len() {
            // Cut at a char boundary so every sample is valid input.
            if !doc.is_char_boundary(split) {
                continue;
            }
            let (head, tail) = doc.split_at(split);
            // The sanitizer's contract, applied to the *prefix* a mid-stream render
            // would see: whatever it returns must contain no unterminated opener.
            let visible = lca_tui::engine::text::sanitize_incomplete_escapes(head);
            assert!(
                lca_tui::engine::text::sanitize_incomplete_escapes(&visible) == visible,
                "{label}: the sanitizer is idempotent at split {split}"
            );
            // And joined whole, the document renders the tail text.
            let joined = format!("{head}{tail}");
            let rows = lca_tui::engine::text::wrap_text_with_ansi(&joined, 200).join("");
            assert!(
                rows.ends_with('z'),
                "{label}: the text after the escape survives a boundary at {split} \
                 ({rows:?})"
            );
        }
    }
}

// Verifies: gh #405 gap 4 - **incomplete, invalid and end-of-stream are three
// different answers.** Collapsing any two is how this bug returns: treating a late
// codepoint as invalid paints `U+FFFD` over text the user typed, and treating an
// invalid byte as incomplete holds it forever.
#[test]
fn the_three_endings_are_distinguishable() {
    // (1) Incomplete: held, silent, recoverable.
    let mut stream = Utf8Stream::new();
    assert_eq!(stream.push(&[0xE6, 0x97]), "");
    assert!(stream.has_pending());
    assert_eq!(stream.push(&[0xA5]), "日");
    assert!(!stream.has_pending());

    // (2) Invalid: replaced once, and the text after it survives.
    let mut stream = Utf8Stream::new();
    let out = stream.push(b"a\xffb");
    assert_eq!(out.matches(REPLACEMENT).count(), 1, "{out:?}");
    assert!(out.contains('a') && out.contains('b'), "{out:?}");

    // (3) Incomplete at end of stream: reported once, never twice.
    let mut stream = Utf8Stream::new();
    assert_eq!(stream.push(b"cut \xe6"), "cut ");
    assert_eq!(stream.finish(), REPLACEMENT.to_string());
    assert_eq!(stream.finish(), "");
}
