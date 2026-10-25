//! Gh #366 gap 3: a terminal reply split across the flush timeout must never
//! be typed into the user's prompt.
//!
//! Split out of `stdin_buffer.rs`'s own `tests` module for the workspace's
//! 1,200-line ceiling (gate 11); that module keeps the reassembly and paste
//! rows, this one holds the reply-fragment rows.
//!
//! # What these rows are for
//!
//! Over a congested SSH link or a multiplexer a terminal's answer can straddle
//! two packets by more than the 50 ms flush timeout. `process` then emits
//! nothing, `flush` delivers the fragment as **text**, and `parse_negotiation`
//! recognises neither half - so literal `[?62;1;2c` lands in the composer on
//! startup.
//!
//! Every row here drives a **split** arrival through the real buffer, because
//! the bug is a reassembly failure: sending the whole sequence would prove
//! nothing.

use super::*;

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

/// Deliver `head`, let the sequence timeout elapse (the reader loop's flush),
/// then deliver `tail`. Returns everything the consumer would receive as data.
fn split(build: impl Fn() -> StdinBuffer, head: &str, tail: &str) -> Vec<String> {
    let mut buffer = build();
    let mut out = data(buffer.process(head));
    // Stand in for the reader loop: the timeout elapsed with no more data,
    // so it flushes.
    assert!(
        buffer.pending_timeout_ms().is_some(),
        "the head must leave a remainder for this row to test a split"
    );
    out.extend(data(buffer.flush()));
    out.extend(data(buffer.process(tail)));
    out
}

/// The exact sanitised form: every emitted data string, concatenated.
fn split_lenient(head: &str, tail: &str) -> String {
    split(StdinBuffer::new, head, tail).concat()
}

// ---------------------------------------------------------------------------
// The injected-fragment rows (red before the fix)
// ---------------------------------------------------------------------------

// A Kitty flags reply (`ESC [ ? 7 u`) split after `ESC [ ?`: the classic
// shape the issue names.
#[test]
fn a_kitty_reply_split_across_the_timeout_never_reaches_the_prompt() {
    let out = split_lenient("\x1b[?", "7u");
    assert_eq!(
        out, "",
        "the fragment is discarded, not delivered as text (got {out:?})"
    );
}

// A DA1 reply (`ESC [ ? 62 ; 1 ; 2 c`) split in two.
#[test]
fn a_da1_reply_split_in_two_never_reaches_the_prompt() {
    let out = split_lenient("\x1b[?62", ";1;2c");
    assert_eq!(out, "", "the fragment is discarded (got {out:?})");
}

// The same reply split in three, with a flush between each piece.
#[test]
fn a_da1_reply_split_in_three_never_reaches_the_prompt() {
    let mut buffer = StdinBuffer::new();
    let mut out = data(buffer.process("\x1b[?62"));
    out.extend(data(buffer.flush()));
    out.extend(data(buffer.process(";1")));
    out.extend(data(buffer.flush()));
    out.extend(data(buffer.process(";2c")));
    let out = out.concat();
    assert_eq!(out, "", "every fragment is discarded (got {out:?})");
}

// An `OSC 11` colour reply split after `ESC ] 11 ;`. The `]` family is the
// one that could forge a title or a clipboard write, so this row matters
// most.
#[test]
fn an_osc11_reply_split_never_reaches_the_prompt() {
    let out = split_lenient("\x1b]11;rgb:1e1e", "/1e1e/1e1e\x07");
    assert_eq!(out, "", "the fragment is discarded (got {out:?})");
    // The `ST`-terminated dialect too.
    let out = split_lenient("\x1b]11;#1e1e2e", "\x1b\\");
    assert_eq!(out, "", "the ST dialect is discarded (got {out:?})");
}

// The colour-scheme report (`ESC [ ? 996 n`) split.
#[test]
fn a_color_scheme_report_split_never_reaches_the_prompt() {
    let out = split_lenient("\x1b[?996", ";1n");
    assert_eq!(out, "", "the fragment is discarded (got {out:?})");
}

// `OSC 7501` (the program-status support reply) split.
#[test]
fn a_program_status_reply_split_never_reaches_the_prompt() {
    let out = split_lenient("\x1b]7501", ";?\x1b\\");
    assert_eq!(out, "", "the fragment is discarded (got {out:?})");
}

// ---------------------------------------------------------------------------
// The narrowness rows (these must be GREEN before and after)
// ---------------------------------------------------------------------------

// A complete reply is unchanged: the happy path still parses. This is the
// no-regression row for every other row here.
#[test]
fn a_complete_reply_still_arrives_whole() {
    for reply in [
        "\x1b[?7u",
        "\x1b[?62;1;2c",
        "\x1b]11;rgb:1e1e/1e1e/1e1e\x07",
        "\x1b[?996;1n",
        "\x1b]7501;?\x1b\\",
    ] {
        let mut buffer = StdinBuffer::new();
        assert_eq!(
            data(buffer.process(reply)),
            vec![reply.to_string()],
            "a complete reply arrives as one sequence"
        );
        assert_eq!(
            buffer.pending_timeout_ms(),
            None,
            "and leaves no remainder behind"
        );
    }
}

// The reason the timeout cannot simply grow: a bare `Escape` must still
// reach the editor promptly. It is a legitimate one-character prefix, and a
// 500 ms window would delay the most-used key in the app.
#[test]
fn a_bare_escape_still_flushes_promptly() {
    let mut buffer = StdinBuffer::new();
    assert!(
        data(buffer.process("\x1b")).is_empty(),
        "held for reassembly"
    );
    assert_eq!(
        buffer.pending_timeout_ms(),
        Some(DEFAULT_ESCAPE_TIMEOUT_MS),
        "a lone ESC keeps the short window, not the sequence one"
    );
    assert_eq!(
        data(buffer.flush()),
        vec!["\x1b".to_string()],
        "and it is delivered on the timeout, not discarded"
    );
}

// Esc then a real key inside the window: the Alt+key path still works.
#[test]
fn a_bare_escape_followed_by_a_key_still_works() {
    let mut buffer = StdinBuffer::new();
    assert!(buffer.process("\x1b").is_empty());
    assert_eq!(
        data(buffer.process("x")),
        vec!["\x1b x".replace(' ', "").to_string()],
        "ESC + key reassembles as the meta pair"
    );
}

// **The narrowness rows.** A fix that ate arbitrary `ESC [` prefixes would
// break pasted content, so the recogniser must commit before it discards.
//
// Three cases, because the honest answer differs:
//
// 1. A pasted sequence that arrives **whole** and is not one of our replies is
//    text - the buffer emits it without ever consulting the recogniser.
// 2. A pasted `ESC [` that splits is text too: `ESC [` alone has not committed
//    to any reply family (`\x1b[H`, `\x1b[1;2H`, `\x1b[M…` all start so).
// 3. A pasted `ESC [ ?` that splits at exactly that point **is** discarded, and
//    that is the documented trade: at a flush boundary the buffer cannot tell
//    it from the head of a Kitty/DA1 answer, and the brief's design point 3
//    picks corruption-of-a-prompt over a visible annoyance. The next row pins
//    that cost rather than leaving it implicit.
#[test]
fn a_pasted_sequence_that_is_not_a_reply_is_still_delivered() {
    // 1. Arrives whole: not our reply, so not our business.
    let mut buffer = StdinBuffer::new();
    assert_eq!(
        data(buffer.process("\x1b[?99z")).concat(),
        "\x1b[?99z",
        "an unknown complete CSI-? sequence is text"
    );
    // 2. Splits before committing: `ESC [` is not a reply prefix.
    let out = split_lenient("\x1b[", "1;2H");
    assert!(
        out.contains("1;2H"),
        "a split cursor-position CSI is text (got {out:?})"
    );
    // 3. Splits *inside* the `CSI ?` introducer: discarded, by design.
    let out = split_lenient("\x1b[?", "99z");
    assert_eq!(
        out, "",
        "an ambiguous `ESC [ ?` head is discarded rather than typed (got {out:?})"
    );
}

// The cost of row 3, stated so it is a decision rather than a surprise: a user
// who literally types `ESC [ ?` and pauses would lose it. Nobody conjures an
// ESC by typing, so the exposure is pasted escape data - and the alternative
// is `[?62;1;2c` appearing in a prompt mid-sentence on a slow link.
#[test]
fn the_ambiguous_discard_is_exactly_one_introducer_wide() {
    // Everything that has committed past the introducer is text.
    for fragment in ["\x1b[?99z", "\x1b[?6z", "\x1b]10;", "\x1b[1;2H"] {
        assert!(
            !is_reply_prefix(fragment),
            "{fragment:?} has committed to something we did not ask for"
        );
    }
    // And the introducers themselves are the candidates, because we did ask.
    for fragment in ["\x1b[?", "\x1b]1", "\x1b]11", "\x1b]7501", "\x1b[?996"] {
        assert!(
            is_reply_prefix(fragment),
            "{fragment:?} could be the head of a reply we asked for"
        );
    }
}

// A plain `ESC [` with nothing reply-shaped after it is still typing.
#[test]
fn a_split_plain_csi_is_still_delivered() {
    let out = split_lenient("\x1b[", "1;2H");
    assert!(
        out.contains("1;2H"),
        "a cursor-position CSI is not a reply (got {out:?})"
    );
}

// Ordinary text between flushes is untouched. (Plain characters never linger
// in the buffer, so there is no remainder to flush - `process` emits them
// immediately; the assertion is that they come out in order and complete.)
#[test]
fn ordinary_typing_across_a_flush_is_untouched() {
    let mut buffer = StdinBuffer::new();
    let mut out = data(buffer.process("hel"));
    assert_eq!(
        buffer.pending_timeout_ms(),
        None,
        "text leaves no remainder"
    );
    out.extend(data(buffer.flush()));
    out.extend(data(buffer.process("lo")));
    assert_eq!(out.concat(), "hello");
}

// ---------------------------------------------------------------------------
// The discard is *named*, not silent
// ---------------------------------------------------------------------------

// "Nothing happened" is not the assertion: the buffer must report that it
// dropped something, so the discard is a decision the caller can see rather
// than a behaviour that differs invisibly from "the terminal said nothing".
#[test]
fn a_discarded_fragment_is_reported_rather_than_silently_dropped() {
    let mut buffer = StdinBuffer::new();
    assert!(data(buffer.process("\x1b[?")).is_empty());
    let events = buffer.flush();
    assert!(
        events.is_empty(),
        "a reply fragment is not delivered as data (got {events:?})"
    );
    assert!(
        buffer.discarded_fragments() >= 1,
        "the drop is counted, so it is a named decision (got {})",
        buffer.discarded_fragments()
    );
    assert!(
        buffer.buffered().is_empty(),
        "and the fragment no longer sits in the buffer"
    );
}

// A fragment that completes after the flush is still recognised as a reply
// continuation and never typed - the second half of the bug.
#[test]
fn the_tail_of_a_flushed_reply_never_reaches_the_prompt() {
    let mut buffer = StdinBuffer::new();
    assert!(data(buffer.process("\x1b[?62")).is_empty());
    assert!(buffer.flush().is_empty(), "the head is discarded");
    let out = data(buffer.process(";1;2c")).concat();
    assert_eq!(
        out, "",
        "the tail is discarded too, not typed (got {out:?})"
    );
}

// The constants are named and shared, not literals duplicated here.
#[test]
fn the_timing_constants_are_named() {
    assert_eq!(DEFAULT_SEQUENCE_TIMEOUT_MS, 50);
    assert_eq!(DEFAULT_ESCAPE_TIMEOUT_MS, 10);
    let buffer = StdinBuffer::new();
    assert_eq!(buffer.sequence_timeout_ms(), DEFAULT_SEQUENCE_TIMEOUT_MS);
    assert_eq!(buffer.escape_timeout_ms(), DEFAULT_ESCAPE_TIMEOUT_MS);
}
