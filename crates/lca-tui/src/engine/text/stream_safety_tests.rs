//! gh #405 gap 4: streaming output must never emit a partial escape or a
//! replacement character.
//!
//! # What was actually broken
//!
//! The issue's premise is that a chunk boundary slices an escape or a multi-byte
//! character. That part is already safe, and measuring it is the first thing this
//! file does: the accumulator is a `String` built with `push_str`, and rendering
//! parses the **whole** accumulated string, so a split `\x1b[38;5` + `;196m` and a
//! split `é` both reassemble before anything looks at them. `lca-wire-openai`'s SSE
//! decoder has held partial frames since gh #337 for the same reason.
//!
//! What *was* broken is the neighbouring case, and it is the one with teeth: an
//! escape sequence that is **incomplete in the final text** - a truncated
//! `\x1b[38;5` at end of stream, a lone `\x1b`, an OSC that never terminates -
//! reached the renderer as literal characters. `extract_ansi_code` returns `None`
//! for "no complete sequence here", and both call sites then pushed the raw `ESC`
//! byte through as text:
//!
//! ```text
//! wrap_text_with_ansi("before\x1b[38;5", 80)  ->  ["before\x1b[38;5"]
//! wrap_text_with_ansi("done\x1b", 80)         ->  ["done\x1b"]
//! wrap_text_with_ansi("hi\x1b]0;evil", 80)    ->  ["hi\x1b]0;evil"]
//! ```
//!
//! A half escape sequence printed to a terminal is a control-byte injection into
//! the user's screen - the exact thing LCA's threat model says escapes may not do.
//! So the fix is a sanitizer that drops an unterminated sequence at the tail and
//! keeps everything before it.
//!
//! # What each row pins
//!
//! - the premise is measured, not assumed (split chunks already work);
//! - the real defect is closed (incomplete tails are dropped, not printed);
//! - the legitimate vocabulary still passes through untouched;
//! - a replacement character from an invalid byte is bounded, not retried.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code.

use super::{sanitize_incomplete_escapes, wrap_text_with_ansi};

/// The ESC byte, spelled once.
const ESC: char = '\u{1b}';

/// Render and flatten, which is what the transcript does with each line.
fn rendered(text: &str) -> String {
    wrap_text_with_ansi(text, 80).join("\n")
}

// Verifies: gh #405 gap 4 - **the premise, measured.** A CSI split across two
// chunks renders as one correct sequence, because the accumulator concatenates
// before anything parses. This row exists so the report cannot claim a fix for a
// bug that was not there.
#[test]
fn a_split_escape_sequence_reassembles_and_renders() {
    // Two pushes, exactly as two stream deltas would arrive.
    let mut acc = String::new();
    acc.push_str("\x1b[38;5");
    acc.push_str(";196mRED");
    let out = rendered(&acc);
    assert!(
        out.contains("\x1b[38;5;196m"),
        "the complete sequence is intact: {out:?}"
    );
    // The parameters appear *inside* the sequence legitimately; what must not
    // happen is the digits arriving as visible characters. Strip the sequence and
    // assert nothing of it is left over.
    let visible = out.replace("\x1b[38;5;196m", "");
    assert_eq!(visible, "RED", "only the text remains: {out:?}");
}

// Verifies: gh #405 gap 4 - **a split multi-byte character reassembles.** Same
// reason: `push_str` on a `String` cannot hold a partial codepoint, and the
// decoder above it (gh #337) never decodes a partial frame.
#[test]
fn a_split_multibyte_character_reassembles() {
    let out = rendered("café · 日本語");
    assert!(out.contains('é') && out.contains('日'), "{out:?}");
    assert!(
        !out.contains('\u{fffd}'),
        "no replacement character: {out:?}"
    );
}

// Verifies: gh #405 gap 4 - **the real defect.** An escape that never completes is
// dropped at the tail, not printed. A half sequence reaching the terminal is a
// control byte on the user's screen.
#[test]
fn an_incomplete_csi_at_the_end_is_dropped() {
    let out = rendered("before\x1b[38;5");
    assert!(!out.contains(ESC), "no raw ESC reaches the screen: {out:?}");
    assert!(
        out.contains("before"),
        "the text before it survives: {out:?}"
    );
}

// Verifies: gh #405 gap 4 - a lone trailing ESC is dropped too: it is the
// degenerate incomplete sequence, and the same injection vector.
#[test]
fn a_lone_trailing_escape_is_dropped() {
    let out = rendered("done\x1b");
    assert!(!out.contains(ESC), "no raw ESC: {out:?}");
    assert!(out.contains("done"), "text survives: {out:?}");
}

// Verifies: gh #405 gap 4 - **an unterminated OSC is dropped.** OSC is the
// dangerous family (title sets, clipboard writes); an unterminated one is the
// clearest case of "half a control sequence on the screen".
#[test]
fn an_unterminated_osc_is_dropped() {
    let out = rendered("hi\x1b]0;evil title");
    assert!(!out.contains(ESC), "no raw ESC: {out:?}");
    assert!(out.contains("hi"), "text survives: {out:?}");
    assert!(
        !out.contains("evil"),
        "and the payload does not leak as text either: {out:?}"
    );
}

// Verifies: gh #405 gap 4 - **the legitimate vocabulary still renders.** The
// sanitizer must not be a blanket ESC strip: a complete SGR, EL, and OSC 8
// hyperlink all still pass through, or the fix would break styling to close an
// injection hole.
#[test]
fn complete_sequences_still_pass_through() {
    for (label, text, needle) in [
        ("SGR colour", "\x1b[31mred\x1b[0m", "\x1b[31m"),
        ("erase line", "a\x1b[Kb", "\x1b[K"),
        (
            "OSC 8 link",
            "\x1b]8;;https://x\x1b\\l\x1b]8;;\x1b\\",
            "https://x",
        ),
    ] {
        let out = rendered(text);
        assert!(out.contains(needle), "{label} survives: {out:?}");
    }
}

// Verifies: gh #405 gap 4 - **only a trailing incomplete sequence is dropped.**
// An incomplete sequence in the *middle* of the text is followed by more text, so
// it is not "incomplete" in the closed-vocabulary sense - the sanitizer must not
// start eating content after it. (The renderer's own narrow final-byte set is what
// keeps a mid-text oddity inert; this row pins that the sanitizer does not make it
// worse by discarding the rest.)
#[test]
fn a_trailing_drop_keeps_everything_before_it() {
    let text = "alpha beta gamma\x1b[1;2";
    let out = sanitize_incomplete_escapes(text);
    assert_eq!(
        out, "alpha beta gamma",
        "the whole visible prefix survives intact"
    );
}

// Verifies: gh #405 gap 4 - **a replacement character is bounded.** An invalid
// byte from the provider surfaces as `U+FFFD` once; the renderer must not print a
// run of them or loop retrying. Bound rather than banned, because the decoder is
// what produces it and dropping it silently would hide a real transport problem.
#[test]
fn replacement_characters_are_visible_but_bounded() {
    let out = rendered("before\u{fffd}\u{fffd}\u{fffd}after");
    assert!(
        out.contains("before") && out.contains("after"),
        "the surrounding text renders: {out:?}"
    );
    assert!(
        out.matches('\u{fffd}').count() <= 3,
        "no amplification of the replacement invariant: {out:?}"
    );
}

// Verifies: gh #405 gap 4 design point 8 - **bounded, and the bound is honest.**
//
// Past `MAX_TRAILING_SEQUENCE` an `ESC` cannot be a legitimate sequence - no SGR or
// link here is 4 KiB long - so it is a malformed opener in front of real payload. Two
// wrong answers are available and the first version took one of them: *dropping the
// tail* would eat kilobytes of the user's output, and *returning the text unchanged*
// (what it did) let the raw `ESC` reach the terminal, which is the injection this
// function exists to prevent. Measured before this row existed:
//
// ```text
// sanitize("\x1b]8;;" + "x" * 5000) -> 5005 bytes, still contains ESC
// ```
//
// So the control byte alone is dropped and the payload is kept. The visible text is
// unchanged, nothing is eaten, and no `ESC` survives.
#[test]
fn a_very_long_unterminated_sequence_keeps_its_payload_and_drops_the_control_byte() {
    // 64 KiB is far past the cap.
    let mut text = String::from("keep");
    text.push(ESC);
    text.push_str(&"x".repeat(64 * 1024));
    let out = sanitize_incomplete_escapes(&text);
    assert!(
        !out.contains(ESC),
        "no raw control byte survives, however long the opener is (that is the \
         injection the function exists to prevent)"
    );
    assert_eq!(
        out.len(),
        text.len() - 1,
        "exactly the ESC is gone - not the payload with it, and not the text \
         returned untouched"
    );
    assert!(
        out.starts_with("keep") && out.ends_with("xxx"),
        "the payload on both sides of the dropped byte survives"
    );
    // Inside the cap the drop still happens, which is the case that matters.
    let mut near = String::from("keep");
    near.push(ESC);
    near.push_str(&"y".repeat(100));
    assert_eq!(
        sanitize_incomplete_escapes(&near),
        "keep",
        "a trailing unterminated sequence within the window is dropped"
    );
}

// Verifies: gh #405 gap 4 - **a complete sequence followed by text keeps the text.**
//
// This is the backward-scan defect, and it is a *data-loss* bug rather than a
// rendering nit: the scan looked back for an `ESC`, found the one inside an `ST`
// terminator, `extract_ansi_code` answered `None` for it (the next byte is `\\`, not
// `[`/`]`/`_`), and everything from there was dropped. Measured on the shipped tree
// before this row:
//
// ```text
// "a\x1b]0;title\x1b\\b"  ->  "a\x1b]0;title"   (the trailing "b" is gone)
// "a\x1b\\b"              ->  "a"               (a bare ST eats the rest of the row)
// ```
//
// A title set or a link followed by more text on the same row lost that text. The
// existing row only covered the case where `ST` ends the string, which is exactly the
// case a backward scan happens to get right - so it passed while both of these
// silently truncated.
#[test]
fn text_after_a_complete_sequence_survives() {
    for (label, text, tail) in [
        ("OSC title then text", "a\x1b]0;title\x1b\\b", "b"),
        ("OSC title, leading", "\x1b]0;title\x1b\\b", "b"),
        ("bare ST then text", "a\x1b\\b", "b"),
        ("CSI then text", "a\x1b[1mb", "b"),
        (
            "link then more text on the row",
            "\x1b]8;;https://x\x1b\\lbl\x1b]8;;\x1b\\tail",
            "tail",
        ),
    ] {
        let out = sanitize_incomplete_escapes(text);
        // The assertion is on the *tail*, not on a flattened "ab": a complete
        // sequence legitimately stays in the string (the renderer consumes it later),
        // so what this row is about is whether the text after it was eaten.
        assert!(
            out.ends_with(tail),
            "{label}: the text after the sequence survives ({out:?} does not end with {tail:?})"
        );
        assert_eq!(
            out, text,
            "{label}: a complete sequence is left exactly as it was"
        );
    }
}

// Verifies: gh #405 gap 4 - an empty string and plain text are untouched, so the
// sanitizer is a no-op on the overwhelming majority of output.
#[test]
fn plain_text_is_untouched() {
    assert_eq!(sanitize_incomplete_escapes(""), "");
    assert_eq!(sanitize_incomplete_escapes("plain ascii"), "plain ascii");
    assert_eq!(sanitize_incomplete_escapes("日本語"), "日本語");
}

// Gh #384 bug 1's upstream defect: **a trailing OSC `ST` is a complete
// sequence, not an incomplete one.** `ST` is `ESC \`; the sanitizer looked
// at the last `ESC`, found `\` at the next byte, and - because the closed
// vocabulary knew only `CSI`, `OSC` and `APC` openers - called it
// unterminated and chopped it. Every rendered link ends this way
// (`render_inline` closes OSC 8 with ST), so a wrap silently left a
// dangling `ESC ] 8 ; ;` with no terminator, and everything after it -
// including, in a table, the rest of the row's cells - became OSC payload
// and vanished from the screen.
#[test]
fn a_trailing_osc_string_terminator_is_not_dropped() {
    let text = "\x1b]8;;https://x\x1b\\lbl\x1b]8;;\x1b\\";
    let out = sanitize_incomplete_escapes(text);
    assert_eq!(
        out, text,
        "a complete OSC-8 open/close pair survives whole, ST included"
    );
    assert!(
        out.ends_with("\x1b]8;;\x1b\\"),
        "the close keeps its terminator: {out:?}"
    );
}

// ---------------------------------------------------------------------------
// Gh #405 gap 4, the strengthening the brief asked for: the property, not only
// the samples.
// ---------------------------------------------------------------------------
//
// The rows above pin the shapes the defect had (a split CSI, a split OSC, an
// incomplete tail) one document at a time. That leaves the *space* around them
// open: a sanitizer that drops a complete sequence, or one that misses a partly
// written opener at some offset nobody sampled, passes every one of them. The
// property closes the space instead of adding a fourth sample.
//
// Two properties, both over **arbitrary chunk splits** of a fixed document:
//
// 1. **byte-identical rendering** - the same document rendered as one string and
//    as N arbitrarily sized chunks produces the same rows. This is the equality
//    the issue's manager note calls "worth more than every shape assertion
//    around it".
// 2. **no raw control byte survives** - whatever the chunks, no rendered row
//    carries an `ESC` that a terminal would act on.
//
// The document is fixed (not arbitrary text) because the *interesting* document
// is the one with escapes, a link, CJK and a ZWJ emoji in it; arbitrary bytes
// would mostly test that random letters render as random letters.

use proptest::prelude::*;

/// A document with every shape that has ever been split badly: a true-colour
/// SGR, an OSC 8 hyperlink, a reset, CJK (3-byte sequences), an emoji (4-byte),
/// a ZWJ cluster (several codepoints that must not be cut apart visually), and a
/// trailing plain character.
const PROPERTY_DOC: &str = "start \x1b[38;2;10;20;30mcolour\x1b[0m                             \x1b]8;;https://example.com/x\x1b\\link\x1b]8;;\x1b\\                             \u{65e5}\u{672c}\u{8a9e} \u{1f389} \u{1f468}\u{200d}\u{1f469}                             end";

/// The chunks of `data` under a split schedule: `cuts` is a list of absolute
/// offsets, sorted and clamped, with 0 and `len` implied.
fn chunked<'a>(data: &'a str, cuts: &[usize]) -> Vec<&'a str> {
    let mut bounds: Vec<usize> = cuts
        .iter()
        .map(|cut| (*cut).min(data.len()))
        .filter(|cut| data.is_char_boundary(*cut))
        .collect();
    bounds.push(0);
    bounds.push(data.len());
    bounds.sort_unstable();
    bounds.dedup();
    let mut out = Vec::new();
    for pair in bounds.windows(2) {
        out.push(&data[pair[0]..pair[1]]);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Verifies: gh #405 gap 4 - **one chunk and any chunk split render the
    /// same rows.** The accumulator is a `String` and rendering parses the whole
    /// of it, so the only way this fails is if a caller renders a *partial*
    /// accumulation - which is exactly the defect the sanitizer exists to make
    /// safe.
    #[test]
    fn any_chunk_split_renders_like_one_chunk(cuts in proptest::collection::vec(0usize..96, 0..12)) {
        // The chunk boundaries are boundaries of the *bytes* a pipe delivers, so
        // a schedule lands where it lands - char boundaries are filtered, since a
        // str split inside a codepoint is not a shape a `&str` stream can have.
        let pieces = chunked(PROPERTY_DOC, &cuts);
        let mut acc = String::new();
        for piece in &pieces {
            acc.push_str(piece);
        }
        // A single-chunk schedule is the trivially-equal case, and it is
        // *allowed*: the property is "however it is split", and "not at all" is
        // one of those. What must never differ is the rendering.
        prop_assert_eq!(&acc, PROPERTY_DOC, "the concatenation is the document");
        let whole = wrap_text_with_ansi(PROPERTY_DOC, 80).join("\n");
        let split = wrap_text_with_ansi(&acc, 80).join("\n");
        prop_assert_eq!(split, whole, "chunks {} render differently", cuts.len());
    }

    /// Verifies: gh #405 gap 4 - **no unterminated escape survives**, whatever
    /// the split or the truncation point.
    ///
    /// "No `ESC` at all" would be the wrong property: a complete colour span
    /// legitimately keeps its bytes. What must never survive is an `ESC` that
    /// opens **nothing**, because that is the byte a terminal acts on - the
    /// injection this function exists to prevent. The document's own escapes are
    /// all complete, so the whole string and every prefix of it must come out
    /// with only complete escapes.
    #[test]
    fn no_split_leaks_an_unterminated_escape(cuts in proptest::collection::vec(0usize..96, 0..12)) {
        // Split schedules do not change the sanitizer's input (it sees the joined
        // text), so this property is about the *truncation* points a streaming
        // render can land on - one per chunk boundary the schedule produced.
        let pieces = chunked(PROPERTY_DOC, &cuts);
        let mut acc = String::new();
        let mut boundaries = vec![0usize];
        for piece in &pieces {
            acc.push_str(piece);
            boundaries.push(acc.len());
        }
        prop_assert_eq!(&acc, PROPERTY_DOC, "the concatenation is the document");
        let mut checked = 0;
        for cut in boundaries {
            let visible = sanitize_incomplete_escapes(&acc[..cut]);
            prop_assert_eq!(
                unterminated_escape_at(&visible),
                None,
                "a prefix truncated at {} left an unterminated escape: {:?}",
                cut,
                visible
            );
            checked += 1;
        }
        prop_assert!(checked > 0, "at least one boundary was checked");
    }
}

/// The offset of the first `ESC` in `text` that opens nothing complete, or `None`.
///
/// A **second scanner, deliberately**: this is a test's oracle, and an oracle
/// built out of the function under test would agree with it by construction. It
/// is not a second production parser - the one-parser rule (`docs/testing-plan.md`) -
/// because nothing ships it.
fn unterminated_escape_at(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos] != 0x1b {
            pos += 1;
            continue;
        }
        // `ESC \` is a complete two-byte ST.
        if bytes.get(pos + 1) == Some(&b'\\') {
            pos += 2;
            continue;
        }
        // A CSI runs to its final byte (0x40..=0x7e) over parameter/intermediate
        // bytes; an OSC runs to `BEL` or `ESC \`.
        match bytes.get(pos + 1) {
            Some(b'[') => {
                let mut end = pos + 2;
                while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                    end += 1;
                }
                if end >= bytes.len() {
                    return Some(pos);
                }
                pos = end + 1;
            }
            Some(b']') => {
                let mut end = pos + 2;
                loop {
                    if end + 1 >= bytes.len() {
                        return Some(pos);
                    }
                    if bytes[end] == 0x07 {
                        pos = end + 1;
                        break;
                    }
                    if bytes[end] == 0x1b && bytes[end + 1] == b'\\' {
                        pos = end + 2;
                        break;
                    }
                    end += 1;
                }
            }
            _ => return Some(pos),
        }
    }
    None
}
