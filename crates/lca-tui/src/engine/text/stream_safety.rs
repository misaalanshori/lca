//! Drop an escape sequence that never completed (gh #405 gap 4).
//!
//! Split from `text.rs` for the 1,200-line ceiling (gate 11); behaviour unchanged.

use super::extract_ansi_code;

/// How far back from the end a trailing escape search will look (gh #405).
///
/// `extract_ansi_code`'s own vocabulary is short (a CSI has no length limit in the
/// standard, but LCA only ever *renders* `m G K H J`, and OSC here is a hyperlink
/// or a title). A cap means a stream that emits an ESC and then megabytes of text
/// cannot make this scan unbounded - the tail is dropped either way, and a
/// truncated sequence is dropped rather than printed by design.
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
/// # What it does
///
/// Finds the last `ESC` in `text`. If a complete sequence starts there, nothing is
/// incomplete and the input is returned unchanged. Otherwise everything from that
/// `ESC` onward is dropped: it is a sequence that never terminated, so there is no
/// safe way to render it.
///
/// The search only looks back a bounded window (4 KiB), so a pathological
/// unterminated sequence costs bounded work rather than an unbounded scan.
#[must_use]
pub fn sanitize_incomplete_escapes(text: &str) -> &str {
    let bytes = text.as_bytes();
    let window_start = bytes.len().saturating_sub(MAX_TRAILING_SEQUENCE);
    let mut pos = bytes.len();
    while pos > window_start {
        pos -= 1;
        if bytes[pos] != 0x1b {
            continue;
        }
        // A complete sequence at this position means nothing is unterminated.
        if extract_ansi_code(text, pos).is_some() {
            return text;
        }
        return &text[..pos];
    }
    // No `ESC` in the window: nothing incomplete to drop. (An `ESC` further back
    // than the cap with no terminator would have been dropped when it was still
    // near the end, so reaching here means the tail is genuinely plain.)
    text
}
