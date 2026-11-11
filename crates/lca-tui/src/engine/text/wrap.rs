//! Word wrap with ANSI preserved (RE doc §6).
//!
//! Split from the barrel for gate 11; the wrapping rules, their comments and
//! their rows are unchanged. A wrap never splits a grapheme and carries the
//! active style across the break, which is what the rows in `tests.rs` pin.

use unicode_segmentation::UnicodeSegmentation;

use super::ansi::{AnsiCodeTracker, update_tracker_from_text};
use super::scan::{char_at, extract_ansi_code};
use super::stream_safety::sanitize_incomplete_escapes;
use super::width::visible_width;

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
    // gh #405: a sequence that never completed is not text - drop it rather than
    // let its bytes reach the terminal.
    let text = sanitize_incomplete_escapes(text);
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
