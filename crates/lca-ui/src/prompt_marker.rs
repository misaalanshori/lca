//! The in-band prompt marker (gh #375 gap 3): one `OSC 133;A` prefix on the first
//! row of every user message, and the exact strip that keeps it off the screen.
//!
//! # Why in-band
//!
//! A prompt jump used to store a **document row** computed at the width and theme in
//! force when the jump was requested, then apply it on a later frame under whatever
//! width and theme were in force then. A row is a *derived coordinate*: resize the
//! pane and the wrap changes, change the theme and a row's height changes, and the
//! stored number now points at some other line. The fix is to keep the *fact* in the
//! data — pi's `OSC 133;A`, which the terminal also uses as a shell-prompt zone — so
//! the row is found by scanning the rows that are about to be painted.
//!
//! The marker lives inside the rendered line, so it reflows with it. That is the whole
//! trick: a marker that rode beside the line would need the same width bookkeeping the
//! bug is about.
//!
//! # Why the strip must be exact
//!
//! The bytes are an escape sequence. A partial strip (`OSC 133` without its `BEL`, or
//! a `\x1b\\` terminator left behind) puts raw control bytes into the frame, which is
//! #366's class of defect one layer up: the terminal eats them and the row loses a
//! cell, or worse it prints nothing and the frame is silently one column short.
//! [`strip_markers`] therefore removes whole sequences only, and both terminators are
//! handled because pi's own renderer accepts both: `BEL` (`\x07`) and the
//! string-terminator form (`ESC \`).
//!
//! # Why it must not disturb the width
//!
//! `visible_width` (gh #378) has an ASCII fast path that walks escape sequences via
//! `ansi_code_length`, which already understands `ESC ]` … `BEL`/ST. So a marked line
//! measures exactly as its unmarked text does — asserted as a row rather than assumed,
//! because a marker that changed the width would silently undo that optimisation.

/// The `OSC 133;A` prompt-start marker, in the `BEL`-terminated spelling pi emits.
pub(crate) const PROMPT_START: &str = "\x1b]133;A\x07";

/// Whether `line` begins a user prompt.
///
/// A prefix test rather than a `contains`, because the marker only ever opens a line:
/// a message whose *text* mentioned the sequence would otherwise register as a prompt,
/// and the scan would jump to the wrong row.
pub(crate) fn starts_prompt(line: &str) -> bool {
    line.starts_with(PROMPT_START)
}

/// Remove every prompt marker from `line`, leaving all other styling untouched.
///
/// Not `strip_terminal_sequences`: that would remove the row's own SGR as well, which
/// is the colour the painter just put there. This removes the marker's bytes and
/// nothing else, whichever terminator the emitting side used.
pub(crate) fn strip_markers(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains("\x1b]133;") {
        return std::borrow::Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find("\x1b]133;") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + "\x1b]133;".len()..];
        // The terminator decides the length: `BEL`, or `ESC \` (ST). A sequence with
        // neither is malformed and left in place rather than eaten up to end-of-line,
        // so a stray introducer cannot swallow the rest of a row.
        let end = tail
            .find('\x07')
            .map(|bel| bel + 1)
            .or_else(|| tail.find("\x1b\\").map(|st| st + 2));
        match end {
            Some(len) => rest = &tail[len..],
            None => {
                out.push_str("\x1b]133;");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// Remove every prompt marker from `lines`, in place.
///
/// The row-list form of [`strip_markers`], for a place that owns the whole document:
/// it visits each row once and leaves untouched rows completely alone.
pub(crate) fn strip_markers_in_place(lines: &mut [String]) {
    for line in lines.iter_mut() {
        if line.contains("\x1b]133;") {
            *line = strip_markers(line).into_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #375 gap 3 - the marker is recognised only as a **prefix**, so a
    // message that merely contains the bytes is not mistaken for a prompt.
    #[test]
    fn only_a_leading_marker_marks_a_prompt() {
        assert!(starts_prompt(&format!("{PROMPT_START}hello")));
        assert!(!starts_prompt(&format!("  {PROMPT_START}hello")));
        assert!(!starts_prompt(&format!("\x1b[1m{PROMPT_START}hello")));
        assert!(!starts_prompt("hello"));
    }

    // Verifies: gh #375 gap 3 - **the strip is exact**, for both terminators.
    //
    // A partial strip is #366's class: escape bytes reach the screen and the row is
    // short a cell. Both spellings are stripped because pi's renderer accepts both.
    #[test]
    fn the_strip_removes_whole_sequences_only() {
        assert_eq!(strip_markers(&format!("{PROMPT_START}hi")), "hi");
        assert_eq!(strip_markers("\x1b]133;A\x1b\\hi"), "hi");
        // No marker, no allocation, no change.
        assert_eq!(strip_markers("plain \x1b[1mbold"), "plain \x1b[1mbold");
        // A malformed introducer is left alone rather than eating the rest of the row.
        assert_eq!(strip_markers("\x1b]133;Ahoy"), "\x1b]133;Ahoy");
        // Every marker on the row goes, and the styling between them survives.
        assert_eq!(
            strip_markers(&format!("\x1b[1m{PROMPT_START}a{PROMPT_START}b\x1b[0m")),
            "\x1b[1mab\x1b[0m"
        );
    }

    // Verifies: gh #375 gap 3 - **the marker does not change the measured width.**
    //
    // The #378 interaction the brief calls out: `visible_width`'s ASCII fast path
    // walks `ESC ]` sequences via `ansi_code_length`, so a marked row must measure
    // exactly as its unmarked text. If this ever fails, a marker silently costs every
    // row a column and the whole optimisation is undone.
    #[test]
    fn a_marker_does_not_change_the_visible_width() {
        use lca_tui::engine::text::{ascii_visible_width, visible_width};
        let plain = "> hello world";
        let marked = format!("{PROMPT_START}{plain}");
        assert_eq!(visible_width(&marked), visible_width(plain));
        assert_eq!(
            ascii_visible_width(&marked),
            ascii_visible_width(plain),
            "the fast path handles the sequence, it does not bail out of it"
        );
    }
}
