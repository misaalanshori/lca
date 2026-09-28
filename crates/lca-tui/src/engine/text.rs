//! Text measurement and surgery, ported from pi's
//! `packages/tui/src/utils.ts`
//! (`pi-tui-re/src_re/tui-engine/utils.md`).
//!
//! This layer decides whether the whole renderer looks right: width, wrap,
//! truncate, slice, and ANSI-state tracking.
//!
//! Port note (RE doc §10): `unicode-width` covers the common East Asian /
//! combining / emoji cases; the terminal-bug corrections pi layers on top
//! (tab = 3, regional indicators pinned at 2, Thai/Lao AM +1, the terminal
//! spacing-mark set) are kept explicitly.
//! `ponytail:` full Unicode `Mark`/`Spacing_Mark` property tables are not
//! ported; `unicode-width` plus the explicit mark sets cover the common
//! cases. Upgrade with a `unicode-properties`-class crate if drift shows.
//!
//! Attribution: `graphemeWidth` is "based on code from the string-width
//! library" (MIT), as pi's own header states.

mod ansi;
mod osc8;
use ansi::update_tracker_from_text;
pub use ansi::{AnsiCodeTracker, get_active_background_ansi};
use osc8::get_active_osc8_close;
pub use osc8::{ActiveHyperlink, Osc8Terminator, parse_osc8_hyperlink};

use std::cell::RefCell;
use std::collections::HashMap;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

const WIDTH_CACHE_SIZE: usize = 512;

thread_local! {
    static WIDTH_CACHE: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
}

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
            let mut j = pos + 2;
            while j < bytes.len() && !matches!(bytes[j], b'm' | b'G' | b'K' | b'H' | b'J') {
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

fn is_printable_ascii(s: &str) -> bool {
    s.bytes().all(|b| (0x20..=0x7e).contains(&b))
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
pub fn visible_width(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    if is_printable_ascii(s) {
        return s.len();
    }
    if let Some(w) = WIDTH_CACHE.with(|c| c.borrow().get(s).copied()) {
        return w;
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
    WIDTH_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() >= WIDTH_CACHE_SIZE
            && let Some(first) = cache.keys().next().cloned()
        {
            cache.remove(&first);
        }
        cache.insert(s.to_string(), width);
    });
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

// =============================================================================
// Wrapping (RE doc §6)
// =============================================================================

fn is_cjk(ch: char) -> bool {
    let cp = ch as u32;
    (0x3040..=0x30ff).contains(&cp) // Hiragana/Katakana
        || (0x3400..=0x4dbf).contains(&cp) // CJK Ext A
        || (0x4e00..=0x9fff).contains(&cp) // CJK Unified
        || (0xac00..=0xd7af).contains(&cp) // Hangul syllables
        || (0x1100..=0x11ff).contains(&cp) // Hangul Jamo
        || (0x3100..=0x312f).contains(&cp) // Bopomofo
        || (0xf900..=0xfaff).contains(&cp)
}

fn split_into_tokens_with_ansi(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut pending_ansi = String::new();
    let mut current_kind: Option<bool> = None; // Some(true)=space
    let mut i = 0;

    let flush =
        |current: &mut String, current_kind: &mut Option<bool>, tokens: &mut Vec<String>| {
            if !current.is_empty() {
                tokens.push(std::mem::take(current));
                *current_kind = None;
            }
        };

    while i < text.len() {
        if let Some((code, len)) = extract_ansi_code(text, i) {
            pending_ansi.push_str(&code);
            i += len;
            continue;
        }
        let mut end = i;
        while end < text.len() && extract_ansi_code(text, end).is_none() {
            let Some(ch) = char_at(text, end) else { break };
            end += ch.len_utf8();
        }
        for segment in text[i..end].graphemes(true) {
            let is_space = segment == " ";
            if !is_space && segment.chars().any(is_cjk) {
                flush(&mut current, &mut current_kind, &mut tokens);
                let token = format!("{pending_ansi}{segment}");
                pending_ansi.clear();
                tokens.push(token);
                continue;
            }
            let kind = is_space;
            if !current.is_empty() && current_kind != Some(kind) {
                flush(&mut current, &mut current_kind, &mut tokens);
            }
            if !pending_ansi.is_empty() {
                current.push_str(&pending_ansi);
                pending_ansi.clear();
            }
            current_kind = Some(kind);
            current.push_str(segment);
        }
        i = end;
    }

    if !pending_ansi.is_empty() {
        if !current.is_empty() {
            current.push_str(&pending_ansi);
        } else if let Some(last) = tokens.last_mut() {
            last.push_str(&pending_ansi);
        } else {
            current = pending_ansi;
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn break_long_word(word: &str, width: usize, tracker: &mut AnsiCodeTracker) -> Vec<String> {
    enum Seg {
        Ansi(String),
        Grapheme(String),
    }
    let mut segments: Vec<Seg> = Vec::new();
    let mut i = 0;
    while i < word.len() {
        if let Some((code, len)) = extract_ansi_code(word, i) {
            segments.push(Seg::Ansi(code));
            i += len;
        } else {
            let mut end = i;
            while end < word.len() && extract_ansi_code(word, end).is_none() {
                let Some(ch) = char_at(word, end) else { break };
                end += ch.len_utf8();
            }
            for g in word[i..end].graphemes(true) {
                segments.push(Seg::Grapheme(g.to_string()));
            }
            i = end;
        }
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current_line = tracker.active_codes();
    let mut current_width = 0usize;
    for seg in segments {
        match seg {
            Seg::Ansi(code) => {
                current_line.push_str(&code);
                tracker.process(&code);
            }
            Seg::Grapheme(g) => {
                if g.is_empty() {
                    continue;
                }
                let w = visible_width(&g);
                if current_width + w > width {
                    current_line.push_str(&tracker.line_end_reset());
                    lines.push(std::mem::take(&mut current_line));
                    current_line = tracker.active_codes();
                    current_width = 0;
                }
                current_line.push_str(&g);
                current_width += w;
            }
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_single_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    if visible_width(line) <= width {
        return vec![line.to_string()];
    }
    let mut wrapped: Vec<String> = Vec::new();
    let mut tracker = AnsiCodeTracker::new();
    let tokens = split_into_tokens_with_ansi(line);
    let mut current_line = String::new();
    let mut current_len = 0usize;

    for token in tokens {
        let token_len = visible_width(&token);
        let is_whitespace = token.trim().is_empty();

        if token_len > width && !is_whitespace {
            if !current_line.is_empty() {
                let reset = tracker.line_end_reset();
                current_line.push_str(&reset);
                wrapped.push(std::mem::take(&mut current_line));
            }
            let mut broken = break_long_word(&token, width, &mut tracker);
            let last = broken.pop().unwrap_or_default();
            wrapped.extend(broken);
            current_len = visible_width(&last);
            current_line = last;
            continue;
        }

        if current_len + token_len > width && current_len > 0 {
            let mut line_to_wrap = current_line.trim_end().to_string();
            line_to_wrap.push_str(&tracker.line_end_reset());
            wrapped.push(line_to_wrap);
            if is_whitespace {
                current_line = tracker.active_codes();
                current_len = 0;
            } else {
                current_line = format!("{}{}", tracker.active_codes(), token);
                current_len = token_len;
            }
        } else {
            current_line.push_str(&token);
            current_len += token_len;
        }
        update_tracker_from_text(&token, &mut tracker);
    }
    if !current_line.is_empty() {
        wrapped.push(current_line);
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    wrapped
        .into_iter()
        .map(|l| l.trim_end().to_string())
        .collect()
}

/// Word-wrap with ANSI preserved. No padding, no background. Active styles
/// carry across line breaks and literal newlines.
pub fn wrap_text_with_ansi(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut result: Vec<String> = Vec::new();
    let mut tracker = AnsiCodeTracker::new();
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for input_line in normalized.split('\n') {
        let prefix = if result.is_empty() {
            String::new()
        } else {
            tracker.active_codes()
        };
        let wrapped = wrap_single_line(&format!("{prefix}{input_line}"), width);
        result.extend(wrapped);
        update_tracker_from_text(input_line, &mut tracker);
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

/// Pad `line` to `width` and wrap the whole thing in `bg_fn`'s styling.
pub fn apply_background_to_line(
    line: &str,
    width: usize,
    bg_fn: impl Fn(&str) -> String,
) -> String {
    let visible = visible_width(line);
    let padding = " ".repeat(width.saturating_sub(visible));
    bg_fn(&format!("{line}{padding}"))
}

// =============================================================================
// Truncation (RE doc §7)
// =============================================================================

fn truncate_fragment_to_width(text: &str, max_width: usize) -> (String, usize) {
    if max_width == 0 || text.is_empty() {
        return (String::new(), 0);
    }
    if is_printable_ascii(text) {
        let clipped: String = text.chars().take(max_width).collect();
        let w = clipped.len();
        return (clipped, w);
    }
    let mut result = String::new();
    let mut width = 0;
    let mut i = 0;
    let mut pending_ansi = String::new();
    while i < text.len() {
        if let Some((code, len)) = extract_ansi_code(text, i) {
            pending_ansi.push_str(&code);
            i += len;
            continue;
        }
        let Some(ch) = char_at(text, i) else { break };
        if ch == '\t' {
            if width + 3 > max_width {
                break;
            }
            result.push_str(&pending_ansi);
            pending_ansi.clear();
            result.push('\t');
            width += 3;
            i += 1;
            continue;
        }
        let mut end = i;
        while end < text.len() && extract_ansi_code(text, end).is_none() {
            let Some(ch) = char_at(text, end) else { break };
            end += ch.len_utf8();
        }
        for g in text[i..end].graphemes(true) {
            let w = grapheme_width(g);
            if width + w > max_width {
                return (result, width);
            }
            if !pending_ansi.is_empty() {
                result.push_str(&pending_ansi);
                pending_ansi.clear();
            }
            result.push_str(g);
            width += w;
        }
        i = end;
    }
    (result, width)
}

fn finalize_truncated(
    prefix: &str,
    prefix_width: usize,
    ellipsis: &str,
    ellipsis_width: usize,
    max_width: usize,
    pad: bool,
) -> String {
    let reset = "\x1b[0m";
    let hyperlink_close = get_active_osc8_close(prefix);
    let visible = prefix_width + ellipsis_width;
    let mut result = if !ellipsis.is_empty() {
        format!("{prefix}{hyperlink_close}{reset}{ellipsis}{reset}")
    } else {
        format!("{prefix}{hyperlink_close}{reset}")
    };
    if pad {
        result.push_str(&" ".repeat(max_width.saturating_sub(visible)));
    }
    result
}

/// Truncate to a maximum visible width, appending an ellipsis when needed
/// (contiguous-prefix policy, RE doc §7).
pub fn truncate_to_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.is_empty() {
        return if pad {
            " ".repeat(max_width)
        } else {
            String::new()
        };
    }
    let ellipsis_width = visible_width(ellipsis);
    if ellipsis_width >= max_width {
        let text_width = visible_width(text);
        if text_width <= max_width {
            return if pad {
                format!("{text}{}", " ".repeat(max_width - text_width))
            } else {
                text.to_string()
            };
        }
        let (clipped, clipped_width) = truncate_fragment_to_width(ellipsis, max_width);
        if clipped_width == 0 {
            return if pad {
                " ".repeat(max_width)
            } else {
                String::new()
            };
        }
        return finalize_truncated("", 0, &clipped, clipped_width, max_width, pad);
    }

    if is_printable_ascii(text) {
        if text.len() <= max_width {
            return if pad {
                format!("{text}{}", " ".repeat(max_width - text.len()))
            } else {
                text.to_string()
            };
        }
        let target = max_width - ellipsis_width;
        let prefix: String = text.chars().take(target).collect();
        return finalize_truncated(&prefix, target, ellipsis, ellipsis_width, max_width, pad);
    }

    let target = max_width - ellipsis_width;
    let mut result = String::new();
    let mut pending_ansi = String::new();
    let mut visible_so_far = 0usize;
    let mut kept_width = 0usize;
    let mut keep_contiguous = true;
    let mut overflowed = false;

    let has_ansi = text.contains('\x1b');
    let has_tabs = text.contains('\t');
    if !has_ansi && !has_tabs {
        for g in text.graphemes(true) {
            let w = grapheme_width(g);
            if keep_contiguous && kept_width + w <= target {
                result.push_str(g);
                kept_width += w;
            } else {
                keep_contiguous = false;
            }
            visible_so_far += w;
            if visible_so_far > max_width {
                overflowed = true;
                break;
            }
        }
        if !overflowed {
            return if pad {
                format!(
                    "{text}{}",
                    " ".repeat(max_width.saturating_sub(visible_so_far))
                )
            } else {
                text.to_string()
            };
        }
    } else {
        let mut i = 0;
        while i < text.len() {
            if let Some((code, len)) = extract_ansi_code(text, i) {
                pending_ansi.push_str(&code);
                i += len;
                continue;
            }
            let Some(ch) = char_at(text, i) else { break };
            if ch == '\t' {
                if keep_contiguous && kept_width + 3 <= target {
                    result.push_str(&pending_ansi);
                    pending_ansi.clear();
                    result.push('\t');
                    kept_width += 3;
                } else {
                    keep_contiguous = false;
                    pending_ansi.clear();
                }
                visible_so_far += 3;
                if visible_so_far > max_width {
                    overflowed = true;
                    break;
                }
                i += 1;
                continue;
            }
            let mut end = i;
            while end < text.len() && extract_ansi_code(text, end).is_none() {
                let Some(ch) = char_at(text, end) else { break };
                end += ch.len_utf8();
            }
            for g in text[i..end].graphemes(true) {
                let w = grapheme_width(g);
                if keep_contiguous && kept_width + w <= target {
                    result.push_str(&pending_ansi);
                    pending_ansi.clear();
                    result.push_str(g);
                    kept_width += w;
                } else {
                    keep_contiguous = false;
                    pending_ansi.clear();
                }
                visible_so_far += w;
                if visible_so_far > max_width {
                    overflowed = true;
                    break;
                }
            }
            if overflowed {
                break;
            }
            i = end;
        }
        if !overflowed {
            return if pad {
                format!(
                    "{text}{}",
                    " ".repeat(max_width.saturating_sub(visible_so_far))
                )
            } else {
                text.to_string()
            };
        }
    }
    finalize_truncated(
        &result,
        kept_width,
        ellipsis,
        ellipsis_width,
        max_width,
        pad,
    )
}

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
            } else if current_col >= after_start && current_col < after_end && after_started {
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

/// The punctuation set used by word navigation.
pub const PUNCTUATION: &str = "(){}[]<>.,;:'\"!?+-=*/\\|&%^$#@~`";

/// Whether a character is whitespace.
pub fn is_whitespace_char(c: char) -> bool {
    c.is_whitespace()
}

/// Whether a character is punctuation.
pub fn is_punctuation_char(c: char) -> bool {
    PUNCTUATION.contains(c)
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

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: I.2 width engine (wide chars, combining, emoji, flags).
    #[test]
    fn width_of_ascii_and_wide_and_combining() {
        assert_eq!(visible_width("hello"), 5);
        assert_eq!(visible_width("世界"), 4); // CJK wide
        assert_eq!(visible_width("a\u{0301}"), 1); // combining acute
        assert_eq!(visible_width("😀"), 2); // emoji
        assert_eq!(visible_width("🇺🇸"), 2); // regional indicator pair
        assert_eq!(visible_width("a\tb"), 5); // tab = 3
        assert_eq!(visible_width("\x1b[31mred\x1b[0m"), 3);
    }

    #[test]
    fn ansi_extraction_is_the_closed_vocabulary() {
        assert_eq!(extract_ansi_code("\x1b[31m", 0).unwrap().0, "\x1b[31m");
        assert_eq!(extract_ansi_code("\x1b[2J", 0).unwrap().0, "\x1b[2J");
        assert!(extract_ansi_code("\x1b[999z", 0).is_none()); // unsupported final
        assert_eq!(extract_ansi_code("\x1b]8;;http://x\x07", 0).unwrap().1, 14);
    }

    #[test]
    fn wrap_breaks_on_words_and_long_words_by_grapheme() {
        let lines = wrap_text_with_ansi("the quick brown fox", 9);
        assert!(lines.iter().all(|l| visible_width(l) <= 9));
        assert_eq!(lines, vec!["the quick", "brown fox"]);
        let long = wrap_text_with_ansi("abcdefghij", 4);
        assert_eq!(long, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_carries_style_across_lines() {
        let lines = wrap_text_with_ansi("\x1b[31mthe quick brown fox\x1b[0m", 9);
        assert!(lines[1].starts_with("\x1b[31m"));
        assert!(lines.iter().all(|l| visible_width(l) <= 9));
    }

    #[test]
    fn wrap_breaks_cjk_per_ideograph() {
        let lines = wrap_text_with_ansi("世界你好吗", 4);
        assert!(lines.iter().all(|l| visible_width(l) <= 4));
        assert_eq!(lines, vec!["世界", "你好", "吗"]);
    }

    #[test]
    fn truncate_keeps_a_contiguous_prefix() {
        let out = truncate_to_width("hello world", 8, "...", false);
        assert_eq!(visible_width(&out), 8);
        assert_eq!(strip_terminal_sequences(&out), "hello...");
        assert_eq!(truncate_to_width("hello", 8, "...", false), "hello");
        assert_eq!(truncate_to_width("hello", 8, "...", true), "hello   ");
        let linked = truncate_to_width("\x1b]8;;http://x\x07abcdef\x1b]8;;\x07", 5, "...", false);
        assert!(visible_width(&linked) <= 5);
        assert_eq!(strip_terminal_sequences(&linked), "ab...");
    }

    #[test]
    fn truncate_never_straddles_a_wide_char() {
        let out = truncate_to_width("世界世界", 5, "...", false);
        assert!(visible_width(&out) <= 5);
        assert!(out.starts_with("世"));
    }

    #[test]
    fn slice_preserves_styles_entering_the_range() {
        let line = "\x1b[31mabcdef\x1b[0m";
        let (sliced, width) = slice_with_width(line, 2, 3, false);
        assert_eq!(width, 3);
        assert!(sliced.starts_with("\x1b[31m"));
        assert_eq!(strip_terminal_sequences(&sliced), "cde");
    }

    #[test]
    fn tracker_line_end_reset_closes_only_underline_and_link() {
        let mut tracker = AnsiCodeTracker::new();
        tracker.process("\x1b[4m");
        tracker.process("\x1b[41m");
        tracker.process("\x1b]8;;http://x\x07");
        let reset = tracker.line_end_reset();
        assert!(reset.contains("\x1b[24m"));
        assert!(reset.contains("\x1b]8;;\x07"));
        assert!(!reset.contains("49m")); // background intentionally bleeds
        assert!(tracker.active_codes().contains("41"));
    }

    #[test]
    fn osc8_terminator_is_preserved_on_reopen() {
        let mut tracker = AnsiCodeTracker::new();
        tracker.process("\x1b]8;;http://x\x07");
        assert!(tracker.active_codes().ends_with("\x07"));
        let mut tracker = AnsiCodeTracker::new();
        tracker.process("\x1b]8;;http://x\x1b\\");
        assert!(tracker.active_codes().ends_with("\x1b\\"));
    }

    #[test]
    fn sgr_reset_does_not_clear_the_hyperlink() {
        let mut tracker = AnsiCodeTracker::new();
        tracker.process("\x1b]8;;http://x\x07");
        tracker.process("\x1b[0m");
        assert!(tracker.active_codes().contains("http://x"));
    }

    #[test]
    fn normalize_decomposes_thai_am_and_expands_tabs() {
        assert_eq!(normalize_terminal_output("\u{0e33}"), "\u{0e4d}\u{0e32}");
        assert_eq!(normalize_terminal_output("a\tb"), "a   b");
        assert_eq!(
            normalize_terminal_output("\x1b[31m\t\x1b[0m"),
            "\x1b[31m   \x1b[0m"
        );
    }

    #[test]
    fn extract_segments_inherits_style_across_the_gap() {
        let line = "\x1b[31mabcdef\x1b[0m";
        let (before, bw, after, aw) = extract_segments(line, 3, 4, 2, false);
        assert_eq!(strip_terminal_sequences(&before), "abc");
        assert_eq!(bw, 3);
        assert_eq!(strip_terminal_sequences(&after), "ef");
        assert_eq!(aw, 2);
        assert!(after.starts_with("\x1b[31m"));
    }
}
