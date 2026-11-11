//! Column arithmetic: slicing a rendered line, the overlay's before/after
//! split, and the column hit-tests that answer "what is under this cell".
//!
//! Split from the barrel for gate 11; the rules, their comments and their rows
//! are unchanged. gh #378 gap 2 lives here: an SGR arriving at the slice seam
//! keeps its order.

use unicode_segmentation::UnicodeSegmentation;

use super::ansi::{AnsiCodeTracker, is_apc_marker};
use super::osc8::parse_osc8_hyperlink;
use super::scan::{char_at, extract_ansi_code};
use super::width::grapheme_width;

// =============================================================================
// Slicing and overlay surgery (RE doc §8)
// =============================================================================

/// Extract a range of visible columns.
pub fn slice_by_column(line: &str, start_col: usize, length: usize, strict: bool) -> String {
    slice_with_width(line, start_col, length, strict).0
}

/// Like [`slice_by_column`], also returning the actual width.
pub fn slice_with_width(
    line: &str,
    start_col: usize,
    length: usize,
    strict: bool,
) -> (String, usize) {
    if length == 0 {
        return (String::new(), 0);
    }
    let end_col = start_col + length;
    let mut result = String::new();
    let mut result_width = 0usize;
    let mut current_col = 0usize;
    let mut i = 0;
    let mut pending_ansi = String::new();
    while i < line.len() {
        if let Some((code, len)) = extract_ansi_code(line, i) {
            if current_col >= start_col && current_col < end_col {
                // Gh #378 gap 2: flush the buffered pre-seam codes **before** this
                // one. Emitting the code first reverses the pair at the seam: a
                // `\x1b[0m` sitting exactly at `start_col` was pushed to `result`
                // before the `\x1b[31m` that preceded it, so the tail read
                // `\x1b[0m\x1b[31m` and unstyled text came out red. pi does the
                // same flush (`result += pending + code`).
                result.push_str(&pending_ansi);
                pending_ansi.clear();
                result.push_str(&code);
            } else if current_col < start_col {
                pending_ansi.push_str(&code);
            }
            i += len;
            continue;
        }
        let mut text_end = i;
        while text_end < line.len() && extract_ansi_code(line, text_end).is_none() {
            let Some(ch) = char_at(line, text_end) else {
                break;
            };
            text_end += ch.len_utf8();
        }
        for g in line[i..text_end].graphemes(true) {
            let w = grapheme_width(g);
            let in_range = current_col >= start_col && current_col < end_col;
            let fits = !strict || current_col + w <= end_col;
            if in_range && fits {
                result.push_str(&pending_ansi);
                pending_ansi.clear();
                result.push_str(g);
                result_width += w;
            }
            current_col += w;
            if current_col >= end_col {
                break;
            }
        }
        i = text_end;
        if current_col >= end_col {
            break;
        }
    }
    (result, result_width)
}

/// The before/after split used by overlay compositing, with style
/// inheritance across the gap.
pub fn extract_segments(
    line: &str,
    before_end: usize,
    after_start: usize,
    after_len: usize,
    strict_after: bool,
) -> (String, usize, String, usize) {
    let mut before = String::new();
    let mut before_width = 0usize;
    let mut after = String::new();
    let mut after_width = 0usize;
    let mut current_col = 0usize;
    let mut i = 0;
    let mut pending_ansi_before = String::new();
    let mut after_started = false;
    let after_end = after_start + after_len;
    let mut tracker = AnsiCodeTracker::new();

    while i < line.len() {
        if let Some((code, len)) = extract_ansi_code(line, i) {
            tracker.process(&code);
            if current_col < before_end {
                pending_ansi_before.push_str(&code);
            } else if current_col >= after_start
                && current_col < after_end
                && (after_started || is_apc_marker(&code))
            {
                // Gh #378 gap 3: a code is copied into `after` only once `after`
                // has started, because the first `after` grapheme prepends the
                // tracker's consolidated state and copying twice would double it
                // (gap 2, in the other direction). That is right for a style and
                // wrong for an APC marker: nothing re-emits a marker, so skipping
                // one at the seam drops it silently and the caret/IME anchor
                // moves or disappears. A marker therefore passes through even
                // before the first grapheme - it is zero-width, so the splice's
                // column is unaffected.
                after.push_str(&code);
            }
            i += len;
            continue;
        }
        let mut text_end = i;
        while text_end < line.len() && extract_ansi_code(line, text_end).is_none() {
            let Some(ch) = char_at(line, text_end) else {
                break;
            };
            text_end += ch.len_utf8();
        }
        for g in line[i..text_end].graphemes(true) {
            let w = grapheme_width(g);
            if current_col < before_end && current_col + w <= before_end {
                before.push_str(&pending_ansi_before);
                pending_ansi_before.clear();
                before.push_str(g);
                before_width += w;
            } else if current_col >= after_start && current_col < after_end {
                let fits = !strict_after || current_col + w <= after_end;
                if fits {
                    if !after_started {
                        after.push_str(&tracker.active_codes());
                        after_started = true;
                    }
                    after.push_str(g);
                    after_width += w;
                }
            }
            current_col += w;
            if if after_len == 0 {
                current_col >= before_end
            } else {
                current_col >= after_end
            } {
                break;
            }
        }
        i = text_end;
        if if after_len == 0 {
            current_col >= before_end
        } else {
            current_col >= after_end
        } {
            break;
        }
    }
    (before, before_width, after, after_width)
}

/// The terminal-cell range a grapheme at a visible column occupies.
pub fn get_grapheme_cell_range(line: &str, column: usize) -> Option<(usize, usize)> {
    let mut current_col = 0usize;
    let mut i = 0;
    while i < line.len() {
        if let Some((_code, len)) = extract_ansi_code(line, i) {
            i += len;
            continue;
        }
        let mut text_end = i;
        while text_end < line.len() && extract_ansi_code(line, text_end).is_none() {
            let Some(ch) = char_at(line, text_end) else {
                break;
            };
            text_end += ch.len_utf8();
        }
        for g in line[i..text_end].graphemes(true) {
            let w = grapheme_width(g);
            if w > 0 && column >= current_col && column < current_col + w {
                return Some((current_col, current_col + w));
            }
            current_col += w;
        }
        i = text_end;
    }
    None
}

/// The OSC 8 URL covering a visible column, if any.
pub fn get_osc8_link_at_column(line: &str, column: usize) -> Option<String> {
    let mut active: Option<String> = None;
    let mut current_col = 0usize;
    let mut i = 0;
    while i < line.len() {
        if let Some((code, len)) = extract_ansi_code(line, i) {
            if let Some(link) = parse_osc8_hyperlink(&code) {
                active = link.map(|l| l.url);
            }
            i += len;
            continue;
        }
        let mut text_end = i;
        while text_end < line.len() && extract_ansi_code(line, text_end).is_none() {
            let Some(ch) = char_at(line, text_end) else {
                break;
            };
            text_end += ch.len_utf8();
        }
        for g in line[i..text_end].graphemes(true) {
            let w = if g == "\t" { 3 } else { grapheme_width(g) };
            if column >= current_col && column < current_col + w {
                return active;
            }
            current_col += w;
        }
        i = text_end;
    }
    None
}
