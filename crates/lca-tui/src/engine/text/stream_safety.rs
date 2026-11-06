//! Drop an escape sequence that never completed (gh #405 gap 4).
//!
//! Split from `text.rs` for the 1,200-line ceiling (gate 11).

use std::borrow::Cow;

use super::extract_ansi_code;

/// How far back from the end an unterminated sequence may start and still be
/// **held** (dropped whole) rather than made inert.
///
/// This is not pi's 256 copied over: that number is "how much of a payload pi holds
/// back while streaming", and pi can afford it because its own `stripAnsi` runs
/// afterwards and removes any control byte it did not hold. Here this scan is the
/// only guard on the render path, so the number has to cover the **longest
/// legitimate sequence in this codebase** with room to spare:
///
/// - a true-colour SGR is ~20 bytes (`\x1b[38;2;255;255;255m`); a 16-colour SGR
///   carrying several attributes is under 40;
/// - an OSC 8 hyperlink is `\x1b]8;;` + a URL + `\x1b\\`, and the renderer wraps
///   links to the row width — 4 KiB is two full 2,000-column rows, or forty
///   100-column ones.
///
/// Past that a "sequence" is not a sequence; see [`sanitize_incomplete_escapes`]
/// for what happens then and why.
const MAX_TRAILING_SEQUENCE: usize = 4096;

/// Drop an escape sequence that is **incomplete at the end of `text`** (gh #405).
///
/// # Why this exists
///
/// [`extract_ansi_code`] answers "is there a *complete* sequence here". Callers that
/// walk text with it must decide what to do when the answer is `None` at an `ESC`:
/// before this function, both render paths pushed the raw byte through as a
/// character, so a truncated `\x1b[38;5` at end of stream reached the terminal as
/// literal `ESC [ 3 8 ; 5`. A half escape sequence on the screen is a control-byte
/// injection, which is exactly what LCA's threat model forbids.
///
/// # What it does — a forward scan, and the two bugs the first version had
///
/// It walks the text with [`extract_ansi_code`], stepping over complete sequences,
/// and reacts to the first `ESC` that opens nothing. **A forward scan, because the
/// backward one was wrong**: the first version searched backwards from the end for
/// an `ESC`, found the `ESC` of an `ST` terminator, asked `extract_ansi_code` about
/// it — which answers `None` for `\` — and dropped everything from there. Measured
/// on the shipped tree:
///
/// ```text
/// "a\x1b]0;title\x1b\\b"  ->  "a\x1b]0;title"   // the trailing "b" is gone
/// "a\x1b\\b"              ->  "a"               // a bare ST eats the rest of the row
/// ```
///
/// A title or a link followed by more text on the same row lost that text. The old
/// code guarded only the case where the `ST` was the *last* thing in the string,
/// which is why the pair test above passed while both of these silently truncated.
///
/// # The two outcomes, and why the length decides
///
/// An `ESC` that opens nothing is either a **tail** or a **lie**:
///
/// - **Within the tail window (4 KiB, below) of the end** it is a tail — a chunk
///   boundary mid-sequence, or a stream that ended there. Nothing after it is
///   displayable, so it is dropped whole. This is the behaviour the function is named
///   for, and it is what makes an unterminated OSC inert instead of a title-set on the
///   user's terminal.
/// - **Further back** it cannot be a legitimate sequence: no SGR or link here is
///   4 KiB long. It is a malformed opener followed by real payload, so *dropping*
///   would eat kilobytes of the user's output and *printing* would inject the control
///   byte. The `ESC` alone is dropped and **everything after it is kept as text.**
///
/// That second case is the other bug this fixes: the first version returned the text
/// **unchanged** past the cap, so `"\x1b]8;;" + "x" * 5000` reached the terminal with
/// its raw `ESC` intact — the injection the function exists to prevent, at >4 KiB.
///
/// Returns a borrow whenever the text is untouched or merely truncated (the
/// overwhelming majority), and an owned string only when a mid-text control byte has
/// to be made inert.
#[must_use]
pub fn sanitize_incomplete_escapes(text: &str) -> Cow<'_, str> {
    if !text.contains('\x1b') {
        return Cow::Borrowed(text);
    }
    let at = match first_opener(text) {
        Some(at) => at,
        None => return Cow::Borrowed(text),
    };
    if text.len() - at <= MAX_TRAILING_SEQUENCE {
        // A tail: nothing after it can be shown.
        return Cow::Borrowed(&text[..at]);
    }
    // Too long to be a sequence. Drop the control byte, keep the payload, and keep
    // scanning the remainder in case it holds another opener (each pass removes at
    // least one byte, so this terminates).
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..at]);
    let mut rest = &text[at + 1..];
    loop {
        match first_opener(rest) {
            Some(next) if rest.len() - next > MAX_TRAILING_SEQUENCE => {
                out.push_str(&rest[..next]);
                rest = &rest[next + 1..];
            }
            Some(next) => {
                out.push_str(&rest[..next]);
                return Cow::Owned(out);
            }
            None => {
                out.push_str(rest);
                return Cow::Owned(out);
            }
        }
    }
}

/// The byte offset of the first `ESC` that opens nothing, or `None` when every
/// escape in `text` is complete.
///
/// `ESC \` is an `ST` — the terminator of an OSC/APC that opened earlier — so it is a
/// complete two-byte unit and is stepped over (gh #384 bug 1's upstream defect: a
/// backward scan mistook it for an unterminated opener).
fn first_opener(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos] != 0x1b {
            pos += 1;
            continue;
        }
        if let Some((_, len)) = extract_ansi_code(text, pos) {
            pos += len;
            continue;
        }
        if bytes.get(pos + 1) == Some(&b'\\') {
            pos += 2;
            continue;
        }
        return Some(pos);
    }
    None
}
