//! Truncation to a cell budget, with an ellipsis (RE doc §7).
//!
//! Split from the barrel for gate 11; the truncation rules, their comments and
//! their rows are unchanged. The ASCII shortcut here (`is_printable_ascii`)
//! belongs to truncation rather than to `scan`: a printable-ASCII string is one
//! cell per byte, which is true of nothing else.

use unicode_segmentation::UnicodeSegmentation;

use super::osc8::get_active_osc8_close;
use super::scan::{char_at, extract_ansi_code};
use super::width::{grapheme_width, visible_width};

fn is_printable_ascii(s: &str) -> bool {
    s.bytes().all(|b| (0x20..=0x7e).contains(&b))
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
