//! Width: how many terminal cells a piece of text occupies.
//!
//! The ported `graphemeWidth` rules (pi's `utils.ts`, RE doc §1), the measured
//! width cache, and the two entry points that answer with them
//! ([`visible_width`] and its slow path). Split from the barrel for gate 11;
//! the rules, their comments and their rows are unchanged.
//!
//! The cache is the engine's one `crate::engine::lru::LruCache` (gh #323/#579):
//! a hit refreshes recency, so the row on screen is not evicted by a diagram
//! that merely scrolled past.

use std::cell::RefCell;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

use super::scan::{ascii_visible_width, char_at, extract_ansi_code, note_fast_path_hit};

const WIDTH_CACHE_SIZE: usize = 512;

thread_local! {
    /// Measured widths, bounded and **least-recently-used** (gh #323).
    ///
    /// This used to be a bare `HashMap` evicting `keys().next()`, and `HashMap`
    /// iteration order is hash order - so the victim was arbitrary, and a hot
    /// width could be dropped by the next cold insert precisely when the cache was
    /// doing the most work. It is now the engine's one [`crate::engine::lru::LruCache`],
    /// so the policy is stated once for the codebase instead of re-decided per
    /// cache. LRU rather than insertion order because a frame re-measures the rows
    /// it already showed: recently used predicts used-soon.
    static WIDTH_CACHE: RefCell<crate::engine::lru::LruCache<String, usize>> =
        RefCell::new(crate::engine::lru::LruCache::new(WIDTH_CACHE_SIZE));
}

fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

fn could_be_emoji(segment: &str) -> bool {
    let Some(cp) = segment.chars().next().map(|c| c as u32) else {
        return false;
    };
    (0x1f000..=0x1fbff).contains(&cp)
        || (0x2300..=0x23ff).contains(&cp)
        || (0x2600..=0x27bf).contains(&cp)
        || (0x2b50..=0x2b55).contains(&cp)
        || segment.contains('\u{FE0F}')
        || segment.chars().count() > 2
}

fn is_regional_indicator(c: char) -> bool {
    (0x1f1e6..=0x1f1ff).contains(&(c as u32))
}

/// Terminal spacing marks: Unicode Spacing_Mark minus the three zero-width
/// exceptions, plus the legacy wcwidth extras pi lists.
fn is_terminal_spacing_mark(c: char) -> bool {
    match c as u32 {
        // Named zero-width exceptions.
        0x1734 | 0x302e | 0x302f => false,
        // Legacy wcwidth extras.
        0x065f | 0x0f7f | 0x102b | 0x102c | 0x1031 | 0x1033..=0x1035 | 0x1038 | 0x103a..=0x103e => {
            true
        }
        // A conservative slice of common Indic/SE-Asian spacing marks.
        0x093e..=0x094c
        | 0x09be..=0x09cd
        | 0x0abe..=0x0acd
        | 0x0b3e..=0x0b4d
        | 0x0bbe..=0x0bcd
        | 0x0c3e..=0x0c4d
        | 0x0d3e..=0x0d4d
        | 0x0dca
        | 0x0e33
        | 0x0eb3 => true,
        _ => false,
    }
}

fn is_mark(c: char) -> bool {
    // Combining marks measure as zero width under unicode-width.
    UnicodeWidthChar::width(c) == Some(0)
}

fn is_non_printing_char(c: char) -> bool {
    UnicodeWidthChar::width(c).is_none() || is_mark(c)
}

fn strip_leading_non_printing(s: &str) -> &str {
    let mut idx = 0;
    for c in s.chars() {
        if is_non_printing_char(c) {
            idx += c.len_utf8();
        } else {
            break;
        }
    }
    &s[idx..]
}

fn is_zero_width_cluster(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| UnicodeWidthChar::width(c).is_none() || UnicodeWidthChar::width(c) == Some(0))
}

/// Terminal width of one grapheme cluster (RE doc §1).
pub fn grapheme_width(segment: &str) -> usize {
    if segment == "\t" {
        return 3;
    }
    if !segment.is_empty() && segment.chars().all(is_terminal_spacing_mark) {
        return segment.chars().count();
    }
    if is_zero_width_cluster(segment) {
        return 0;
    }
    if could_be_emoji(segment) {
        // unicode-width returns 2 for emoji-presentation and most ZWJ forms.
        let w = segment.chars().map(char_width).sum::<usize>();
        if w >= 2 || segment.contains('\u{FE0F}') || segment.contains('\u{200D}') {
            return 2;
        }
    }
    let base = strip_leading_non_printing(segment);
    let Some(first) = base.chars().next() else {
        return 0;
    };
    if is_regional_indicator(first) {
        return 2;
    }
    let mut width = char_width(first);
    let mut follows_mark = false;
    for c in base.chars().skip(1) {
        if is_terminal_spacing_mark(c) {
            width += 1;
            follows_mark = false;
        } else if is_mark(c) {
            follows_mark = true;
        } else if !is_non_printing_char(c) {
            let cp = c as u32;
            if follows_mark || (0xff00..=0xffef).contains(&cp) {
                width += char_width(c);
            } else if cp == 0x0e33 || cp == 0x0eb3 {
                width += 1;
            }
            follows_mark = false;
        }
    }
    width
}

fn strip_sequences(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if let Some((code, len)) = extract_ansi_code(s, i) {
            let _ = code;
            i += len;
            continue;
        }
        let Some(ch) = char_at(s, i) else { break };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Visible width in terminal columns.
///
/// **Order: cache, then the ASCII fast path, then segmentation.** The cache is tried
/// first because a hash lookup is cheaper than walking bytes, which I measured rather
/// than assumed (gh #378 gap 1, a 400-cell styled row, release build):
///
/// | case | before | after |
/// |---|---|---|
/// | distinct rows (theme switch, resize) | ~22,000 ns | ~450-530 ns |
/// | one row repeated (cache hit, steady state) | ~183 ns | ~183 ns |
///
/// The win is on the case that hurts: a theme change, a resize or a scroll sees every
/// row **once**, misses the cache, and used to be allocation-bound. A repeated row
/// still hits the cache first and costs what it always did.
///
/// My first version put the fast path *before* the cache and wrote the 39x/2.4x
/// trade-off up as a considered decision. It was not one - a hash lookup is cheaper
/// than a byte walk, so cache-first is at least as good on both axes, and the ordering
/// is the cache's. (The benchmark's "repeated row" case does not show the cache win
/// because the distinct-row pass never inserted those strings; measured separately, a
/// warm cache answers in ~180 ns.)
///
/// The cache still only ever stores what the slow path computed, so this adds nothing
/// to its memory: styled lines now answer before they can be inserted.
pub fn visible_width(s: &str) -> usize {
    if let Some(w) = WIDTH_CACHE.with(|c| c.borrow_mut().get(&s.to_string())) {
        return w;
    }
    if let Some(width) = ascii_visible_width(s) {
        note_fast_path_hit();
        return width;
    }
    visible_width_slow(s)
}

/// The path [`visible_width`] falls back to: cache, tab expansion, sequence
/// stripping, then grapheme segmentation.
///
/// Split out (gh #378 gap 1) so the equivalence table can ask both paths the same
/// question and compare, which is the only thing that makes the fast path safe.
pub(super) fn visible_width_slow(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    let mut clean = if s.contains('\t') {
        s.replace('\t', "   ")
    } else {
        s.to_string()
    };
    if clean.contains('\x1b') {
        clean = strip_sequences(&clean);
    }
    let width: usize = clean.graphemes(true).map(grapheme_width).sum();
    WIDTH_CACHE.with(|c| c.borrow_mut().insert(s.to_string(), width));
    width
}

/// Strip ANSI/OSC/APC sequences, preserving visible text.
pub fn strip_terminal_sequences(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    strip_sequences(s)
}

/// Decompose Thai/Lao AM vowels and expand visible tabs to three spaces
/// (RE doc §3).
pub fn normalize_terminal_output(s: &str) -> String {
    let normalized = if s.contains('\u{0e33}') || s.contains('\u{0eb3}') {
        s.chars()
            .flat_map(|c| match c {
                '\u{0e33}' => vec!['\u{0e4d}', '\u{0e32}'],
                '\u{0eb3}' => vec!['\u{0ecd}', '\u{0eb2}'],
                other => vec![other],
            })
            .collect::<String>()
    } else {
        s.to_string()
    };
    if !normalized.contains('\t') {
        return normalized;
    }
    let mut result = String::with_capacity(normalized.len());
    let mut i = 0;
    while i < normalized.len() {
        if let Some((code, len)) = extract_ansi_code(&normalized, i) {
            result.push_str(&code);
            i += len;
            continue;
        }
        let Some(ch) = char_at(&normalized, i) else {
            break;
        };
        if ch == '\t' {
            result.push_str("   ");
        } else {
            result.push(ch);
        }
        i += ch.len_utf8();
    }
    result
}
