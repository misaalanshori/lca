//! Gh #366 gap 3: a terminal reply split across `StdinBuffer`'s flush timeout
//! was typed into the user's prompt.
//!
//! Over a congested SSH link or a multiplexer a terminal's answer can straddle
//! two packets by more than the 50 ms sequence timeout. `process` then emits
//! nothing, `flush` delivered the head as **text**, and neither half matched
//! what `parse_negotiation` recognises - so literal `[?62;1;2c` appeared in
//! the composer on startup.
//!
//! The decision moved **into the buffer**, not into the editor: by the time a
//! fragment reaches `handle_key` it is indistinguishable from typing, so the
//! only place that can still tell is the reassembly seam. `flush` now drops a
//! remainder that is a prefix of a reply we asked for, and a short window
//! suppresses the tail that follows it.
//!
//! Gaps 1 (the `OSC 7501`/`DA1` ordering plus the dropped `Idle`), 2 (unbounded
//! `DA1` consumption across `$EDITOR` cycles) and 4–5 are out of scope.
//!
//! Verifies: FR-UI-24's input contract (defect gh #366 gap 3). The unit rows
//! live in `crates/lca-tui/src/engine/stdin_buffer/reply_fragment_tests.rs`.

use lca_tui::engine::stdin_buffer::{
    DEFAULT_ESCAPE_TIMEOUT_MS, DEFAULT_SEQUENCE_TIMEOUT_MS, REPLY_TAIL_MAX_CHARS, StdinBuffer,
    StdinEvent, is_reply_prefix,
};

/// The data strings a process produced, ignoring paste events.
fn data(events: Vec<StdinEvent>) -> Vec<String> {
    events
        .into_iter()
        .filter_map(|e| match e {
            StdinEvent::Data(s) => Some(s),
            StdinEvent::Paste(_) => None,
        })
        .collect()
}

/// Deliver `head`, flush as the reader loop does when the timeout elapses,
/// then deliver `tail`. Returns everything the consumer receives as data.
fn split(head: &str, tail: &str) -> String {
    let mut buffer = StdinBuffer::new();
    let mut out = data(buffer.process(head));
    out.extend(data(buffer.flush()));
    out.extend(data(buffer.process(tail)));
    out.concat()
}

/// The five replies the startup queries can elicit, each in a split shape that
/// crosses the flush boundary.
const SPLIT_REPLIES: &[(&str, &str, &str)] = &[
    ("kitty flags", "\x1b[?", "7u"),
    ("DA1", "\x1b[?62", ";1;2c"),
    ("DA1 (longer)", "\x1b[?62;1", ";2c"),
    ("colour scheme", "\x1b[?996", ";1n"),
    ("OSC 11 (BEL)", "\x1b]11;rgb:1e1e", "/1e1e/1e1e\x07"),
    ("OSC 11 (ST)", "\x1b]11;#1e1e2e", "\x1b\\"),
    ("OSC 7501", "\x1b]7501", ";?\x1b\\"),
];

#[test]
fn a_split_terminal_reply_never_becomes_prompt_text() {
    for (label, head, tail) in SPLIT_REPLIES {
        let out = split(head, tail);
        assert_eq!(
            out, "",
            "{label} split across the timeout must not be typed (got {out:?})"
        );
    }
}

/// The three-piece split, with a flush between every piece: the same property
/// one boundary further in.
#[test]
fn a_reply_split_three_ways_never_becomes_prompt_text() {
    let mut buffer = StdinBuffer::new();
    let mut out = data(buffer.process("\x1b[?62"));
    out.extend(data(buffer.flush()));
    out.extend(data(buffer.process(";1")));
    out.extend(data(buffer.flush()));
    out.extend(data(buffer.process(";2c")));
    assert_eq!(out.concat(), "", "no piece is typed (got {out:?})");
}

/// The discard is a **decision the caller can see**, not a silent difference
/// from "the terminal said nothing".
#[test]
fn the_discard_is_counted() {
    let mut buffer = StdinBuffer::new();
    assert_eq!(buffer.discarded_fragments(), 0);
    assert!(data(buffer.process("\x1b[?")).is_empty());
    let flushed = buffer.flush();
    assert!(flushed.is_empty(), "the head is not delivered: {flushed:?}");
    assert_eq!(buffer.discarded_fragments(), 1, "the drop is recorded");
    assert!(buffer.buffered().is_empty(), "and the buffer is cleared");
}

// ---------------------------------------------------------------------------
// The rows that must stay green: the fix must not eat real input
// ---------------------------------------------------------------------------

/// A complete reply still arrives whole, so the happy path is untouched.
#[test]
fn a_complete_reply_still_arrives_whole() {
    for reply in [
        "\x1b[?7u",
        "\x1b[?62;1;2c",
        "\x1b[?996;1n",
        "\x1b]11;rgb:1e1e/1e1e/1e1e\x07",
        "\x1b]7501;?\x1b\\",
    ] {
        let mut buffer = StdinBuffer::new();
        assert_eq!(
            data(buffer.process(reply)),
            vec![reply.to_string()],
            "{reply:?} arrives as one sequence"
        );
        assert_eq!(buffer.pending_timeout_ms(), None, "no remainder left");
        assert_eq!(buffer.discarded_fragments(), 0, "nothing was discarded");
    }
}

/// **The reason the timeout cannot simply grow.** A bare `Escape` is a
/// legitimate one-character prefix, so it keeps the short window and is still
/// delivered on it - a 500 ms window would delay the most-used key in the app.
#[test]
fn a_bare_escape_is_still_delivered_promptly() {
    let mut buffer = StdinBuffer::new();
    assert!(
        data(buffer.process("\x1b")).is_empty(),
        "held to reassemble"
    );
    assert_eq!(
        buffer.pending_timeout_ms(),
        Some(DEFAULT_ESCAPE_TIMEOUT_MS),
        "a lone ESC keeps the 10 ms window"
    );
    assert_eq!(
        data(buffer.flush()),
        vec!["\x1b".to_string()],
        "and is delivered, never discarded"
    );
    assert_eq!(buffer.discarded_fragments(), 0);
}

/// Esc followed by a key inside the window: the Alt+key path still works, and
/// a real keystroke is never eaten.
#[test]
fn typing_is_never_eaten() {
    // Ordinary characters, including across a flush boundary inside a word.
    let mut buffer = StdinBuffer::new();
    let mut out = data(buffer.process("hel"));
    out.extend(data(buffer.flush()));
    out.extend(data(buffer.process("lo world")));
    assert_eq!(out.concat(), "hello world");

    // ESC + key.
    let mut buffer = StdinBuffer::new();
    assert!(buffer.process("\x1b").is_empty());
    assert_eq!(data(buffer.process("x")).concat(), "\x1bx");

    // A pasted sequence that arrives whole and is not our reply.
    let mut buffer = StdinBuffer::new();
    assert_eq!(
        data(buffer.process("\x1b[?99z")).concat(),
        "\x1b[?99z",
        "an unknown complete CSI-? sequence is text"
    );

    // A split that has not committed to a reply family: `ESC [` is not a
    // reply prefix (every other CSI starts the same way).
    assert_eq!(split("\x1b[", "1;2H"), "\x1b[1;2H");
    assert_eq!(split("\x1b", "[A"), "\x1b[A", "the up arrow survives");
}

/// The recognised introducers, named so the narrowness is a table rather than
/// a claim: what commits, and what does not.
#[test]
fn the_recogniser_commits_only_where_a_reply_could_be() {
    for fragment in [
        "\x1b[?",
        "\x1b[?7",
        "\x1b[?62",
        "\x1b[?996",
        "\x1b]1",
        "\x1b]11",
        "\x1b]11;rgb:",
        "\x1b]7501",
        "\x1b]7501;?",
    ] {
        assert!(
            is_reply_prefix(fragment),
            "{fragment:?} could be the head of a reply we asked for"
        );
    }
    for fragment in [
        "\x1b[",
        "\x1b[1;2H",
        "\x1b[A",
        "\x1b[?99z",
        "\x1b[?6z",
        "\x1b]10;",
        "\x1b]110",
        "\x1b]75012",
        "\x1b[?62;1;2c",
    ] {
        assert!(
            !is_reply_prefix(fragment),
            "{fragment:?} has committed to something we did not ask for"
        );
    }
}

/// The timing constants are named and shared, not duplicated by callers.
#[test]
fn the_timing_constants_are_named() {
    assert_eq!(DEFAULT_SEQUENCE_TIMEOUT_MS, 50);
    assert_eq!(DEFAULT_ESCAPE_TIMEOUT_MS, 10);
    // Room for the longest reply we ask for: `?62;1;2c` is 8, a realistic
    // `DA1` is under 30.
    const _: () = assert!(REPLY_TAIL_MAX_CHARS >= 32);
    let buffer = StdinBuffer::new();
    assert_eq!(buffer.sequence_timeout_ms(), DEFAULT_SEQUENCE_TIMEOUT_MS);
    assert_eq!(buffer.escape_timeout_ms(), DEFAULT_ESCAPE_TIMEOUT_MS);
}
