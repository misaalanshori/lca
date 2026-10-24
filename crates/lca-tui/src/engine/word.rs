//! Word-motion segmentation (gh #383 gap 1): one classifier for `Alt+Left`,
//! `Alt+Right` and `Ctrl+W`, so the three cannot disagree about where a word
//! begins and ends.
//!
//! Split out of `text.rs` when that file crossed the 1,200-line ceiling.
//! pi's reference is `packages/tui/src/word-navigation.ts`; the conformance
//! table transcribed from its test lives in `editor_tests.rs`.

use super::text::is_punctuation_char;

/// What a word-motion run is made of (gh #383 gap 1).
///
/// pi segments with `Intl.Segmenter` at word granularity (`word-navigation.ts`) and
/// then treats a segment as one of three things. This is that classification, in Rust,
/// and it is the **one** place the rule lives: `cursor_word_left`, `cursor_word_right`
/// and `delete_word_backward` all call it, because a `Ctrl+W` that lands where
/// `Alt+Left` cannot reach is the same bug in a new shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordClass {
    /// A run of letters, digits and `_` - the thing a reader calls a word.
    Word,
    /// A run of [`PUNCTUATION`](super::text::PUNCTUATION) - `...`, `()->`, `===`. **One unit**, not one per
    /// character: treating punctuation as "anything non-alphanumeric" gives `(` then
    /// `)` then `-`, which feels worse than whitespace-only motion.
    Punctuation,
    /// Spaces and tabs.
    Space,
}

/// Classify one character for word motion (gh #383 gap 1).
///
/// Order matters: punctuation before word, because `_` is a word character but `-` is
/// punctuation and both are non-alphanumeric.
#[must_use]
pub fn word_class(c: char) -> WordClass {
    if c.is_whitespace() {
        WordClass::Space
    } else if is_punctuation_char(c) {
        WordClass::Punctuation
    } else {
        WordClass::Word
    }
}

/// The span of an atomic segment starting at `chars[col]`, if one starts there
/// (gh #383 gap 1): `Some(end_col)`.
///
/// pi's `isAtomicSegment` option (`word-navigation.ts`): a paste marker such as
/// `[paste #1 +3 lines]` is **one token**, so motion hops it whole and deletion removes
/// it whole. A caret landing inside a marker is a different class of bug - it would let
/// `Ctrl+W` leave `[paste #1 +3 lines` behind, which is not a marker and not text the
/// user typed.
///
/// The recogniser is deliberately the same shape `expand_paste_markers` uses: `[paste #`
/// then digits then `]`. Anything else is not atomic.
#[must_use]
pub fn atomic_span_at(chars: &[char], col: usize) -> Option<usize> {
    const OPEN: &[char] = &['[', 'p', 'a', 's', 't', 'e', ' ', '#'];
    if col >= chars.len() || !chars[col..].starts_with(OPEN) {
        return None;
    }
    let mut i = col + OPEN.len();
    let digits = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits {
        return None; // `[paste #` with no id is not a marker
    }
    while i < chars.len() {
        if chars[i] == ']' {
            return Some(i + 1);
        }
        // A marker never spans a bracket or a newline; if it hits one it is not a marker.
        if chars[i] == '[' || chars[i] == '\n' {
            return None;
        }
        i += 1;
    }
    None
}

/// The start of an atomic segment that **ends exactly at** `col`, if one does.
#[must_use]
pub fn atomic_start_ending_at(chars: &[char], col: usize) -> Option<usize> {
    if col > chars.len() {
        return None;
    }
    (0..col)
        .rev()
        .find(|&start| atomic_span_at(chars, start) == Some(col))
}

/// The word the caret would cross moving **backward** from `chars[..col]`, as the
/// column it lands on (gh #383 gap 1).
///
/// pi's `findWordBackward`: skip trailing whitespace, then consume one run of the class
/// found there. A punctuation run is consumed whole, and an atomic segment (a paste
/// marker) is consumed whole *before* the class rule is consulted.
#[must_use]
pub fn word_start_before(chars: &[char], col: usize) -> usize {
    let mut col = col.min(chars.len());
    while col > 0 && word_class(chars[col - 1]) == WordClass::Space {
        col -= 1;
    }
    if col == 0 {
        return 0;
    }
    // An atomic segment is one token (gh #383 gap 1): hop it whole.
    if let Some(start) = atomic_start_ending_at(chars, col) {
        return start;
    }
    let class = word_class(chars[col - 1]);
    while col > 0 && word_class(chars[col - 1]) == class {
        col -= 1;
    }
    col
}

/// The word the caret would cross moving **forward** from `col` (gh #383 gap 1).
///
/// pi's `findWordForward`: skip leading whitespace, then consume one run of the class
/// found there.
#[must_use]
pub fn word_end_after(chars: &[char], col: usize) -> usize {
    let mut col = col.min(chars.len());
    while col < chars.len() && word_class(chars[col]) == WordClass::Space {
        col += 1;
    }
    if col == chars.len() {
        return col;
    }
    // An atomic segment is one token going forward too.
    if let Some(end) = atomic_span_at(chars, col) {
        return end;
    }
    let class = word_class(chars[col]);
    while col < chars.len() && word_class(chars[col]) == class {
        col += 1;
    }
    col
}
