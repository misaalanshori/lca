//! gh #405 gap 4: a codepoint split across reads is **held**, never replaced.
//!
//! The defect this file guards: every caller that decoded a pipe read with
//! `String::from_utf8_lossy` turned a codepoint straddling two reads into `U+FFFD`.
//! Measured on the shipped tool-output path, `"日本語の日本語の日本語の日本語"` in 8-byte
//! chunks became 10 replacement characters and the bash card painted them.
//!
//! The three states are pinned separately, because collapsing any two of them is how
//! this bug returns: **incomplete** (held, silent), **invalid** (replaced, once), and
//! **incomplete at end of stream** (replaced, once, because nothing can complete it).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code.

use lca_protocol::utf8::{REPLACEMENT, Utf8Stream};

/// Decode `bytes` in `chunk`-sized pieces, as a pipe read would.
fn streamed(bytes: &[u8], chunk: usize) -> String {
    let mut out = String::new();
    let mut stream = Utf8Stream::new();
    for piece in bytes.chunks(chunk.max(1)) {
        out.push_str(&stream.push(piece));
    }
    out.push_str(&stream.finish());
    out
}

// Verifies: gh #405 gap 4 - **every split of a document decodes to the document.**
//
// Every byte boundary, not a sample: the bug appeared at *specific* offsets (a
// codepoint's second or third byte), and a table of a few interesting splits is how
// a regression slips back at the one nobody listed. This walks every one.
#[test]
fn every_chunk_boundary_decodes_the_same_text() {
    let documents = [
        "output: 日本語の日本語の日本語の日本語",
        "emoji 🎉🎊 mixed with ascii and 漢字",
        "zwj family 👨‍👩‍👧‍👦 then plain",
        "combining é and e\u{0301} both",
        "ascii only",
        "",
    ];
    for doc in documents {
        let bytes = doc.as_bytes();
        for chunk in 1..=bytes.len().max(1) {
            let got = streamed(bytes, chunk);
            assert_eq!(got, doc, "chunk size {chunk} of {doc:?} decoded to {got:?}");
            assert!(
                !got.contains(REPLACEMENT),
                "no replacement character for a codepoint that merely arrived late: \
                 chunk {chunk} of {doc:?} -> {got:?}"
            );
        }
    }
}

// Verifies: gh #405 gap 4 - **incomplete is silent, and that is observable.**
//
// A lone lead byte is not text yet. It must produce *nothing* rather than a
// placeholder, and `has_pending` is what lets a caller (a spinner, a "waiting for
// output" state) tell "no bytes" from "mid-codepoint".
#[test]
fn a_partial_codepoint_is_held_and_reports_itself() {
    let mut stream = Utf8Stream::new();
    // First byte of "日" (E6 97 A5).
    assert_eq!(stream.push(&[0xE6]), "", "nothing decodable yet");
    assert!(stream.has_pending(), "and the decoder knows it is holding");
    assert_eq!(stream.push(&[0x97]), "", "still mid-codepoint");
    assert_eq!(stream.push(&[0xA5]), "日", "completed");
    assert!(!stream.has_pending(), "nothing left over");
    assert_eq!(stream.finish(), "", "a clean end has nothing to report");
}

// Verifies: gh #405 gap 4 - **invalid is replaced once, visibly, and does not
// swallow the rest.** A byte that can never be UTF-8 is a real transport fault;
// hiding it would be the opposite error from hiding a late codepoint.
#[test]
fn an_invalid_byte_is_one_replacement_and_the_text_continues() {
    let mut stream = Utf8Stream::new();
    // 0xFF can never appear in UTF-8.
    let out = stream.push(b"before\xffafter");
    assert_eq!(
        out.matches(REPLACEMENT).count(),
        1,
        "exactly one replacement, not a run: {out:?}"
    );
    assert!(
        out.starts_with("before") && out.ends_with("after"),
        "the text on both sides survives, so one bad byte cannot swallow the rest: {out:?}"
    );
    // A truncated multi-byte sequence in the *middle* (a lead byte then ASCII) is
    // invalid too, not "incomplete": the ASCII byte cannot continue it.
    let out = stream.push(b"\xe6?a");
    assert!(
        out.contains('?') && out.contains('a'),
        "the ASCII after an impossible continuation is text, not payload: {out:?}"
    );
}

// Verifies: gh #405 gap 4 - **incomplete at end of stream is reported once.**
//
// The transfer was cut mid-character, so nothing can ever complete it. Dropping it
// would silently lose the last character of an answer, which is worse than a visible
// marker; holding it forever would lose it too, and quietly.
#[test]
fn an_unfinished_sequence_at_end_of_stream_is_reported_once() {
    let mut stream = Utf8Stream::new();
    let out = stream.push(b"tail \xe6\x97");
    assert_eq!(
        out, "tail ",
        "the valid prefix decodes, the split one waits"
    );
    assert_eq!(
        stream.finish(),
        REPLACEMENT.to_string(),
        "the cut character is reported once"
    );
    assert_eq!(stream.finish(), "", "and only once");
}

// Verifies: gh #405 gap 4 - **the same bytes in one chunk and in N chunks are the
// same text.** This is the equality the brief asks for, at the decoder that
// produces the text: everything above the decoder then has nothing to repair.
#[test]
fn one_chunk_and_many_chunks_agree() {
    let doc = "streamed \x1b[38;5;196mred\x1b[0m + 日本語 + 🎉 + link \x1b]8;;https://example.com\x1b\\x\x1b]8;;\x1b\\ end";
    let whole = streamed(doc.as_bytes(), doc.len());
    assert_eq!(whole, doc, "the whole document decodes to itself");
    for chunk in [1, 2, 3, 5, 7, 11, 64, 4096] {
        assert_eq!(
            streamed(doc.as_bytes(), chunk),
            doc,
            "chunk size {chunk} must decode identically"
        );
    }
}
