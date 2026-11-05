//! Gh #368: three ways `StdinBuffer` corrupts typed input.
//!
//! # What was already closed
//!
//! #366 (gap 3) built the reply-fragment machinery this file extends:
//! [`is_reply_prefix`] recognises the head of a reply we asked for, `flush` drops
//! such a head instead of delivering it, and a bounded **content** window
//! ([`REPLY_TAIL_MAX_CHARS`]) keeps the tail from being typed either. That closed
//! the `CSI ?` family and `OSC 11`/`OSC 7501` - the rows in
//! `reply_fragment_tests.rs` are still green here for the first of those.
//!
//! It did **not** cover `DCS` or `APC`, because [`ReplyKind`] had no members for
//! them: their heads were never recognised as replies at all, so they were
//! delivered as text and the tail was typed into the prompt (bug 1, family 2).
//!
//! # The three bugs
//!
//! 1. **A split reply's tail lands in the prompt.** For `DCS`/`APC` (and any
//!    `OSC` shape #366 did not name) the head is delivered as text and the tail
//!    arrives as ordinary characters.
//! 2. **An introducer with no terminator swallows every following keystroke.**
//!    `Alt+]` sends `ESC ]` in a legacy terminal; the buffer waits for a
//!    terminator that is never coming, so keystrokes accumulate into one glued
//!    event (`ESC ] abc`) that no parser can read - **the session appears
//!    frozen**. This is the worst of the three, which is why it comes first.
//! 3. **Kitty flag `4`'s colon sub-parameters are rejected**, so `ESC [ 97 :: 97 u`
//!    plus the raw `a` echo inserts the character **twice**.
//!
//! Every row drives a **split** arrival through the real buffer, because each bug
//! is a reassembly failure: sending the sequence whole would prove nothing.

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

/// Deliver `head`, let the sequence timeout elapse (`flush`), then deliver
/// `tail` - the **split reply** shape, where the pause falls between the two
/// halves of a sequence.
fn across_the_timeout(head: &str, tail: &str) -> Vec<String> {
    let mut buffer = StdinBuffer::new();
    let mut out = data(buffer.process(head));
    out.extend(data(buffer.flush()));
    out.extend(data(buffer.process(tail)));
    out
}

/// Deliver `chunks` back to back, then let the timeout elapse - the **typing**
/// shape, where the pause falls *after* the keystrokes rather than between two
/// halves of one sequence.
///
/// Both shapes are real and they are different bugs: a reply arrives in two
/// halves with a gap in the middle, a human types continuously and pauses after.
/// A helper that only modelled one of them would pass the other bug by
/// construction - which is exactly what the first version of this file did.
fn typed_then_flush(chunks: &[&str]) -> Vec<String> {
    let mut buffer = StdinBuffer::new();
    let mut out = Vec::new();
    for chunk in chunks {
        out.extend(data(buffer.process(chunk)));
    }
    out.extend(data(buffer.flush()));
    out
}

/// Run the events through the **real editor**, which is where "swallowed" and
/// "typed twice" are actually observable. A row that only counted events would
/// pass on a fix that emitted an event the editor still could not read.
fn typed(events: &[String]) -> String {
    let mut editor = crate::widgets::editor::Editor::new();
    for event in events {
        editor.handle_key(event);
    }
    editor.text()
}

// ===========================================================================
// Bug 2: an introducer with no terminator is a keystroke, not a sequence
// ===========================================================================

// Verifies: gh #368 bug 2 - **the worst one**: `Alt+]` in a legacy terminal sends
// the `OSC` introducer, and the buffer then waits for a terminator that never
// comes. Today the following keystrokes glue onto it (`ESC ] a b c` as one event)
// and no parser can read that, so the user's session looks frozen.
//
// The honest reading, and pi's (`stdin-buffer.ts`'s flush handler): an introducer
// with no terminator is **not a sequence**. On expiry the two-byte meta pair is
// the `Alt+]` keystroke and everything after it is what the user typed, so the
// remainder is split back into input rather than delivered glued.
#[test]
fn alt_bracket_then_typing_delivers_the_key_and_every_character() {
    // Typed continuously (`\x1b]` then `a` then `b` then `c`), then a pause: the
    // buffer holds all four and only flushes once the typing stops, so the glued
    // event is what the editor would receive today.
    let events = typed_then_flush(&["\x1b]", "a", "b", "c"]);
    assert_eq!(
        events.first().map(String::as_str),
        Some("\x1b]"),
        "the introducer is delivered as its own two-byte keystroke, not glued \
         onto the typing: {events:?}"
    );
    assert_eq!(
        crate::engine::keys::parse_key(&events[0]).as_deref(),
        Some("alt+]"),
        "and it parses as the key the user pressed"
    );
    assert_eq!(
        typed(&events),
        "abc",
        "every typed character lands (today the glued event loses all three): {events:?}"
    );
}

// Verifies: gh #368 bug 2 - the same for the other two control-string
// introducers, `DCS` (`Alt+P`'s uppercase form) and `APC` (`Alt+_`).
//
// One row per introducer because the three are three separate `starts_with`
// branches in `is_complete_sequence`, and a fix that special-cased `ESC ]` would
// leave two thirds of the bug in place.
#[test]
fn the_other_introducers_recover_their_keystrokes_too() {
    for intro in ["\x1bP", "\x1b_"] {
        let events = typed_then_flush(&[intro, "abc"]);
        assert_eq!(
            events.first().map(String::as_str),
            Some(intro),
            "{intro:?} is its own keystroke: {events:?}"
        );
        assert_eq!(
            typed(&events),
            "abc",
            "{intro:?} must not swallow the typing that follows it: {events:?}"
        );
    }
}

// Verifies: gh #368 bug 2 - **the mirror**: a genuine reply prefix still drops,
// and this is the row that keeps bug 2's fix from undoing #366.
//
// The two are distinguished by the **prefix set** (`ESC ]` alone is not a reply
// head; `ESC ] 1 1 ;` is) and by the timeout behaviour (a reply head is dropped
// and opens the tail window; an introducer-only remainder is delivered). A fix
// that delivered *every* control-string remainder would revive #366's prompt
// corruption; a fix that dropped every one would keep the swallowed keys.
#[test]
fn a_reply_head_still_drops_and_an_introducer_still_delivers() {
    // Dropped: the head committed to a reply we asked for.
    let dropped = across_the_timeout("\x1b]11;rgb:1e1e/", "1e1e/2e2e\x07");
    assert!(
        dropped.is_empty(),
        "a reply head and its tail never reach the consumer: {dropped:?}"
    );
    // Delivered: the introducer alone has committed to nothing.
    let delivered = typed_then_flush(&["\x1b]", "abc"]);
    assert!(
        !delivered.is_empty() && typed(&delivered) == "abc",
        "an introducer with no terminator is input: {delivered:?}"
    );
    // And the prefix set is what says which is which.
    assert!(
        !is_reply_prefix("\x1b]"),
        "the bare introducer is not a reply"
    );
    assert!(is_reply_prefix("\x1b]11;"), "`OSC 11 ;` is");
}

// ===========================================================================
// Bug 1: a split reply's tail must not land in the prompt
// ===========================================================================

// Verifies: gh #368 bug 1 for the family #366 **did** close, restated here so the
// three families read as one table: an `OSC 11` colour reply split mid-body is
// dropped head and tail. This row is the control for the two below it.
#[test]
fn an_osc11_reply_split_mid_body_is_dropped_head_and_tail() {
    let out = across_the_timeout("\x1b]11;rgb:1e1e/", "1e1e/2e2e\x07");
    assert_eq!(
        out.concat(),
        "",
        "the issue's exact split, for the family #366 closed: {out:?}"
    );
}

// Verifies: gh #368 bug 1 - **`DCS`**: an XTVersion reply (`ESC P > | XTerm (390)
// ESC \`) is a control string we can elicit, and `ReplyKind` had no member for
// it, so the head was delivered as text and `(390)` was **typed into the prompt**.
//
// The terminator is `ST` for both `DCS` and `APC` (design call 4), and the body
// of a delimited string is arbitrary printable text - `>`, `|`, `(` and `)` are
// not in the `CSI ?` parameter alphabet, which is why the family needs its own
// tail rule rather than a wider shared one.
#[test]
fn a_dcs_reply_split_across_the_timeout_never_reaches_the_prompt() {
    let out = across_the_timeout("\x1bP>|XTerm", "(390)\x1b\\");
    assert_eq!(
        out.concat(),
        "",
        "no part of the version reply is typing: {out:?}"
    );
    assert_eq!(typed(&out), "", "and the editor stays empty: {out:?}");
}

// Verifies: gh #368 bug 1 - **`APC`**, the Kitty graphics reply: the same
// delimited-string rule with the same `ST` terminator.
//
// Split inside the body rather than at the introducer, because a fix that only
// remembered the two-byte head would pass a head/tail split and still leak the
// first byte of the body.
#[test]
fn an_apc_reply_split_across_the_timeout_never_reaches_the_prompt() {
    let out = across_the_timeout("\x1b_Gi=1;O", "K\x1b\\");
    assert_eq!(
        out.concat(),
        "",
        "the graphics reply is not typing: {out:?}"
    );
    assert_eq!(typed(&out), "");
}

// Verifies: gh #368 bug 1 - **the terminator table**, asserted as a table
// because that is what the design call asks for: which family ends on which byte.
//
// `BEL` terminates an `OSC`; `ST` (`ESC \`) terminates `OSC` too (both dialects
// are legal) and is the **only** terminator for `DCS`/`APC`. A row asserting one
// family would let the others drift.
#[test]
fn each_control_string_family_ends_on_its_own_terminator() {
    // `OSC 11`, both dialects.
    for reply in [
        "\x1b]11;rgb:1e1e/1e1e/1e1e\x07",
        "\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\",
    ] {
        let mut buffer = StdinBuffer::new();
        assert_eq!(
            data(buffer.process(reply)),
            vec![reply.to_string()],
            "{reply:?} is one complete sequence"
        );
        assert_eq!(buffer.pending_timeout_ms(), None, "and leaves no remainder");
    }
    // `DCS` and `APC`: `ST` only. `BEL` is a *byte in the body* there, which is
    // why the completion rule cannot be the union of the two.
    for reply in ["\x1bP>|XTerm(390)\x1b\\", "\x1b_Gi=1;OK\x1b\\"] {
        let mut buffer = StdinBuffer::new();
        assert_eq!(
            data(buffer.process(reply)),
            vec![reply.to_string()],
            "{reply:?} is one complete sequence"
        );
        assert_eq!(buffer.pending_timeout_ms(), None);
    }
    let mut buffer = StdinBuffer::new();
    assert!(
        data(buffer.process("\x1bP>|XTerm(390)\x07")).is_empty(),
        "a BEL does not end a DCS - it is a byte inside it"
    );
    assert!(
        buffer.pending_timeout_ms().is_some(),
        "so the buffer is still waiting for ST"
    );
}

// ===========================================================================
// Bug 3: Kitty flag 4's colon sub-parameters
// ===========================================================================

// Verifies: gh #368 bug 3 - **the character appears once.**
//
// Flag `4` is *negotiated by LCA itself* (`KITTY_QUERY` pushes `>7u`), and it
// makes the terminal report an unmodified printable key **twice**: once as
// `ESC [ <cp> u` and once as the raw character. The buffer drops the echo by
// remembering the codepoint, and the recogniser rejected the colon form flag 4
// produces (`ESC [ 97 :: 97 u` for the plain `a`) - so the dedup never armed and
// both events were delivered.
//
// Asserted through the editor, because "twice" is a text outcome: the buffer's
// event list and the composer's contents are two different claims.
#[test]
fn a_kitty_printable_with_colon_sub_parameters_inserts_the_character_once() {
    for sequence in ["\x1b[97u", "\x1b[97::97u", "\x1b[97:97u", "\x1b[97:u"] {
        let mut buffer = StdinBuffer::new();
        let events = data(buffer.process(&format!("{sequence}a")));
        assert_eq!(
            events,
            vec![sequence.to_string()],
            "{sequence:?} arms the echo dedup, so the raw `a` is dropped: {events:?}"
        );
        assert_eq!(
            typed(&events),
            "a",
            "and the character lands exactly once: {events:?}"
        );
    }
}

// Verifies: gh #368 bug 3 - **a malformed sub-parameter declines rather than
// guessing**, like every other parser in this module.
//
// The failure mode of a permissive parser is worse than the duplicate it fixes:
// a sequence it half-understood would arm the dedup on the wrong codepoint and
// then **drop a real keystroke**. So anything outside pi's own grammar
// (`^ESC [ digits ( : digits? )? ( : digits )? u$`) is not a printable report.
#[test]
fn a_malformed_kitty_printable_declines() {
    for sequence in [
        "\x1b[:97u",     // no codepoint
        "\x1b[97::97:u", // a third colon group
        "\x1b[97;1:3u",  // the modified form: `;` is not the printable grammar
        "\x1b[97::97x",  // not the `u` final byte
        "\x1b[97::9x7u", // a non-digit inside a group
        "\x1b[97::97uu", // trailing junk
    ] {
        assert!(
            parse_unmodified_kitty_printable_codepoint(sequence).is_none(),
            "{sequence:?} must decline, not guess"
        );
        let mut buffer = StdinBuffer::new();
        let events = data(buffer.process(&format!("{sequence}a")));
        assert!(
            events.len() >= 2 && events.last().map(String::as_str) == Some("a"),
            "{sequence:?} must not arm the dedup and eat the `a`: {events:?}"
        );
    }
}

// Verifies: gh #368 bug 3 - the codepoint still has to be a printable one, colon
// form or not. `ESC [ 13 :: 13 u` is `Enter` in the CSI-u grammar, so treating it
// as a printable echo would drop the following character instead of the newline.
#[test]
fn a_kitty_printable_below_space_declines_in_the_colon_form_too() {
    for sequence in ["\x1b[13u", "\x1b[13::13u", "\x1b[0::0u"] {
        assert!(
            parse_unmodified_kitty_printable_codepoint(sequence).is_none(),
            "{sequence:?} is not a printable report"
        );
    }
}

// ===========================================================================
// The two costs this trade pays, stated rather than implied
// ===========================================================================

// Verifies: gh #368 bug 2's cost, and it is the mirror of #366's `ESC [ ?` row: a
// `DCS` reply split **before** its head commits (`ESC P` alone, then `>|XTerm…`)
// is indistinguishable from `Alt+P` followed by someone typing `>` and `|`.
//
// The trade is taken deliberately and matches pi (`TERMINAL_REPLY_HEADER_PATTERN`
// requires `P>\|`, not `P`), because the alternatives are both worse: dropping a
// bare `ESC P` freezes every keystroke after `Alt+P` (bug 2, the worst of the
// three), and holding it longer delays `Alt+O`/`Alt+P` for everyone. A version
// reply split on its *second byte* is a one-off startup annoyance; a frozen
// session is not.
#[test]
fn the_alt_key_recovery_costs_one_ambiguous_head_for_dcs() {
    // A timeout between the bare introducer and the rest: recovered as
    // keystrokes, so the remainder types.
    let events = across_the_timeout("\x1bP", ">|XTerm(390)\x1b\\");
    assert_eq!(
        typed(&events),
        ">|XTerm(390)",
        "the accepted cost, on the record: {events:?}"
    );
    // Split *after* it commits: dropped, which is the case the issue reports.
    let dropped = across_the_timeout("\x1bP>|XTerm", "(390)\x1b\\");
    assert_eq!(typed(&dropped), "", "and once it commits it never types");
}

// Verifies: gh #368 bug 1/2's shared boundary for `OSC`: a reply split at the
// **bare introducer** (`ESC ]`, then `11;rgb:…`) leaks, because those two bytes
// are exactly what `Alt+]` sends.
//
// Pinned for the same reason as the row above: the split the issue reports is
// mid-body (`ESC ] 11 ; rgb:1e1e/`), and that one is closed. This one cannot be
// closed without re-opening a frozen session on every `Alt+]`.
#[test]
fn an_osc_reply_split_at_the_bare_introducer_is_the_accepted_cost() {
    let events = across_the_timeout("\x1b]", "11;rgb:1e1e/1e1e/1e1e\x07");
    assert_eq!(
        events.first().map(String::as_str),
        Some("\x1b]"),
        "the introducer is the key"
    );
    assert!(
        typed(&events).contains("rgb"),
        "and the body after it types - the cost, stated: {events:?}"
    );
    // The reported split, one byte later, is closed.
    let dropped = across_the_timeout("\x1b]1", "1;rgb:1e1e/1e1e/1e1e\x07");
    assert_eq!(typed(&dropped), "", "one byte of commitment is enough");
}
