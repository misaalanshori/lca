//! The escape-scanning layer: the closed-vocabulary sequence reader, the
//! byte-safe character accessor every scanner uses, and the ASCII fast path
//! (gh #378 gap 1).
//!
//! Split from the barrel for gate 11 (the 1,200-line ceiling); the code, its
//! comments and its rows are unchanged. The fast path lives here because it
//! answers with [`ansi_code_length`], whose closed vocabulary is the reason a
//! sequence this does not recognise makes the scanner **decline** rather than
//! guess a skip length.

/// Extract the supported ANSI/OSC/APC sequence at `pos`, if any. The CSI
/// final-byte set is deliberately narrow (`m G K H J`) — this is the closed
/// vocabulary every rendered line is assumed to contain, which is also what
/// keeps hostile escapes out (LCA threat model).
pub fn extract_ansi_code(s: &str, pos: usize) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if pos >= bytes.len() || bytes[pos] != 0x1b {
        return None;
    }
    let next = *bytes.get(pos + 1)?;
    match next {
        b'[' => {
            // `A B C D` are cursor movements, accepted for gh #324 gap 1's
            // multi-row iTerm2 shape (see `ansi_code_length`).
            let mut j = pos + 2;
            while j < bytes.len()
                && !matches!(
                    bytes[j],
                    b'm' | b'G' | b'K' | b'H' | b'J' | b'A' | b'B' | b'C' | b'D'
                )
            {
                j += 1;
            }
            if j < bytes.len() {
                Some((s[pos..=j].to_string(), j + 1 - pos))
            } else {
                None
            }
        }
        b']' | b'_' => {
            let mut j = pos + 2;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    return Some((s[pos..=j].to_string(), j + 1 - pos));
                }
                if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                    return Some((s[pos..=j + 1].to_string(), j + 2 - pos));
                }
                j += 1;
            }
            None
        }
        _ => None,
    }
}

/// The character starting at byte `index`, or `None` when `index` is out
/// of range or not a char boundary. The char-boundary-safe replacement for
/// `text[index..].chars().next().unwrap()` in the scanners below.
pub(crate) fn char_at(text: &str, index: usize) -> Option<char> {
    text.get(index..).and_then(|rest| rest.chars().next())
}

#[cfg(test)]
thread_local! {
    /// How many times the ASCII fast path answered (gh #378 gap 1).
    ///
    /// `cfg(test)`-only, so a release build has no counter, no atomic, and no shared
    /// cache line. The acceptance test for the optimisation is that a coloured row
    /// *takes* this path; a faster function nobody calls is not a fix.
    static FAST_PATH_HITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The fast-path hit count (gh #378 gap 1). Test builds only.
#[cfg(test)]
pub(crate) fn fast_path_hits() -> usize {
    FAST_PATH_HITS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(super) fn note_fast_path_hit() {
    FAST_PATH_HITS.with(|hits| hits.set(hits.get() + 1));
}

#[cfg(not(test))]
#[inline(always)]
pub(super) fn note_fast_path_hit() {}

/// Length of the escape sequence starting at `pos`, or `0` when there is none —
/// pi's `ansiCodeLength` (`packages/tui/src/utils.ts`), and deliberately the same
/// closed vocabulary as [`extract_ansi_code`]: CSI finals `m G K H J`, plus the
/// cursor movements `A B C D`.
///
/// **`A B C D` are here for gh #324 gap 1**, whose multi-row iTerm2 shape opens
/// with `\x1b[{n}A` to put the cursor back at the top of the block. The four are
/// not "supported" by pi - they are *unrecognised* there, so pi walks them as text
/// and mismeasures that one row. Recognising them is strictly closer to the
/// terminal's truth, which is all this function answers. **`E F G` stay out on
/// purpose**: `G` already means `CHA` in this vocabulary, and a final that two
/// meanings disagree about is exactly the ambiguity the closed list exists to
/// avoid. A sequence this does not recognise answers `0`, which makes the fast path
/// decline rather than guess: a wrong skip length would silently mismeasure every
/// row after it, and a mismeasurement is much worse than a missed optimisation.
fn ansi_code_length(s: &str, pos: usize) -> usize {
    let bytes = s.as_bytes();
    if pos >= bytes.len() || bytes[pos] != 0x1b {
        return 0;
    }
    let Some(&next) = bytes.get(pos + 1) else {
        return 0;
    };
    match next {
        b'[' => {
            let mut j = pos + 2;
            while j < bytes.len() {
                if matches!(
                    bytes[j],
                    b'm' | b'G' | b'K' | b'H' | b'J' | b'A' | b'B' | b'C' | b'D'
                ) {
                    return j + 1 - pos;
                }
                j += 1;
            }
            0
        }
        b']' | b'_' => {
            let mut j = pos + 2;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    return j + 1 - pos;
                }
                if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                    return j + 2 - pos;
                }
                j += 1;
            }
            0
        }
        _ => 0,
    }
}

/// Width of a string of printable ASCII, tabs and recognised escape sequences, or
/// `None` when it holds anything else (gh #378 gap 1).
///
/// pi's `asciiVisibleWidth`: printable ASCII is one column, a tab is three, an escape
/// sequence is zero. Everything else - a wide glyph, a combining mark, an emoji, a
/// control byte, a malformed escape - declines so `super::width::visible_width` falls back to the
/// cached path that does grapheme segmentation.
///
/// This is the difference between a re-render being linear and being allocation-bound:
/// **every rendered row carries SGR**, so before this the fast path's
/// `is_printable_ascii` check rejected essentially every line and each one cloned,
/// replaced tabs, stripped sequences, segmented graphemes and took the cache lock.
pub fn ascii_visible_width(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut width = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if (0x20..=0x7e).contains(&b) {
            width += 1;
            i += 1;
        } else if b == b'\t' {
            width += 3;
            i += 1;
        } else if b == 0x1b {
            let len = ansi_code_length(s, i);
            if len == 0 {
                return None;
            }
            i += len;
        } else {
            return None;
        }
    }
    Some(width)
}
