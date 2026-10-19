//! Balanced head-and-tail truncation (gh #216, OpenCode's
//! `ToolOutput.truncate` shape): oversized output keeps its head
//! (invocation, flags, first errors) and its tail (summary, exit
//! state), with an explicit middle-omission marker citing line counts
//! and the attachment holding the full text.
//!
//! Two selection modes. When whole lines fit their budgets, the emitted
//! text is line-aligned and reads exactly as it did before gh #321. When a
//! single line is longer than its budget - a minified bundle, a one-line
//! JSON blob - whole lines cannot honor `max_bytes`, so the head and tail
//! are byte windows into the content cut on UTF-8 character boundaries.
//! Either way the result never exceeds `max_bytes`.

/// Reserve for the omission marker, so head + tail + marker honor the
/// byte budget together.
const MARKER_RESERVE: usize = 150;

/// Split oversized `content` into head + omission marker + tail.
/// Returns the content unaltered with `false` when it fits both
/// budgets. Head takes ~40% of the remaining budget, tail ~60% (the
/// summary end matters more). The total emitted bytes never exceed
/// `max_bytes`.
pub fn truncate_head_tail(
    content: &str,
    max_bytes: usize,
    max_lines: usize,
    attachment_ref: Option<&str>,
) -> (String, bool) {
    let total_lines = content.lines().count();
    if content.len() <= max_bytes && total_lines <= max_lines {
        return (content.to_string(), false);
    }
    let lines: Vec<&str> = content.lines().collect();
    let budget_bytes = max_bytes.saturating_sub(MARKER_RESERVE);
    let head_budget = budget_bytes.saturating_mul(2) / 5;
    let tail_budget = budget_bytes.saturating_sub(head_budget);
    // Whole lines, byte-boundary safe: walk the head forward, the tail
    // backward. A line only fits if its cost does *and* its line budget
    // allows it - the `index > 0` / `back > 0` bypass that let the first and
    // last line through regardless of length is what gh #321 removed.
    let head_line_budget = max_lines.saturating_mul(2) / 5;
    let mut head_end = 0usize;
    let mut head_bytes = 0usize;
    for (index, line) in lines.iter().enumerate() {
        if index >= head_line_budget {
            break;
        }
        let cost = line.len() + 1;
        if head_bytes + cost > head_budget {
            break;
        }
        head_bytes += cost;
        head_end = index + 1;
    }
    let tail_line_budget = max_lines.saturating_sub(head_end);
    let mut tail_start = total_lines;
    let mut tail_bytes = 0usize;
    for (back, line) in lines.iter().rev().enumerate() {
        if back >= tail_line_budget {
            break;
        }
        let cost = line.len() + 1;
        if tail_bytes + cost > tail_budget {
            break;
        }
        tail_bytes += cost;
        tail_start = total_lines - back - 1;
    }
    // The windows must not overlap (tiny budgets, huge lines): the tail
    // yields to the head, and the marker covers the rest.
    let tail_start = tail_start.max(head_end);
    let omitted = total_lines.saturating_sub(head_end + (total_lines - tail_start));
    // gh #321: when a single line exceeded its budget, no whole line was
    // selected on that side, so fall back to a byte window on that side.
    // Both cuts are on character boundaries, and the window sizes come from
    // what the whole-line selection left over, so the total still fits.
    let head_part = if head_end > 0 {
        lines[..head_end].join("\n")
    } else {
        byte_window_head(content, head_budget)
    };
    let tail_part = if tail_start < total_lines {
        lines[tail_start..].join("\n")
    } else {
        byte_window_tail(content, tail_budget)
    };
    // The marker describes what was actually kept. A byte window has no
    // line range to cite - "showing lines 1..0" is nonsense - so it reports
    // the byte counts instead.
    let marker = if head_end > 0 && tail_start < total_lines {
        marker_lines(head_end, tail_start, total_lines, omitted, attachment_ref)
    } else {
        marker_bytes(
            head_part.len(),
            tail_part.len(),
            content.len(),
            attachment_ref,
        )
    };
    let mut out = String::with_capacity(head_part.len() + marker.len() + tail_part.len());
    out.push_str(&head_part);
    out.push_str(&marker);
    out.push_str(&tail_part);
    // The marker's own length varies with the counts it cites, so the
    // budget above is an estimate. Clamp to the hard contract rather than
    // trust it: a caller passing `max_bytes` gets at most `max_bytes`.
    if out.len() > max_bytes {
        out.truncate(out.floor_char_boundary(max_bytes));
    }
    (out, true)
}

/// The omission marker for a line-aligned result, with its leading and
/// trailing newlines.
fn marker_lines(
    head_end: usize,
    tail_start: usize,
    total_lines: usize,
    omitted: usize,
    attachment_ref: Option<&str>,
) -> String {
    let saved = match attachment_ref {
        Some(reference) => format!("; full output saved to {reference}"),
        None => String::new(),
    };
    format!(
        "\n[... showing lines 1..{head_end} and {}..{total_lines} of {total_lines} ({omitted} lines omitted{saved}) ...]\n",
        tail_start + 1,
    )
}

/// The omission marker for a byte-window result: no line range applies, so
/// it cites the two byte windows and the total instead.
fn marker_bytes(
    head_len: usize,
    tail_len: usize,
    total_len: usize,
    attachment_ref: Option<&str>,
) -> String {
    let saved = match attachment_ref {
        Some(reference) => format!("; full output saved to {reference}"),
        None => String::new(),
    };
    format!(
        "\n[... showing the first {head_len} and last {tail_len} of {total_len} bytes; a single line is longer than the budget{saved} ...]\n"
    )
}

/// The first `budget` bytes of `content`, cut on a character boundary.
fn byte_window_head(content: &str, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }
    content[..content.floor_char_boundary(budget)].to_string()
}

/// The last `budget` bytes of `content`, cut on a character boundary.
fn byte_window_tail(content: &str, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }
    let start = content.len().saturating_sub(budget);
    content[content.ceil_char_boundary(start)..].to_string()
}

#[cfg(test)]
mod tests {
    use super::truncate_head_tail;

    /// The gh #321 shape: two 100 KiB lines against a 10 KiB budget used to
    /// emit both lines whole.
    #[test]
    fn long_lines_are_bounded() {
        let line = "x".repeat(100 * 1024);
        let content = format!("{line}\n{line}");
        let (out, truncated) = truncate_head_tail(&content, 10 * 1024, 2000, None);
        assert!(truncated);
        assert!(out.len() <= 10 * 1024, "bounded: {}", out.len());
    }

    /// The marker alone must never push the result past the budget, even
    /// when `max_bytes` is smaller than the marker.
    #[test]
    fn a_budget_below_the_marker_length_is_still_bounded() {
        let content = format!("{}\n{}", "a".repeat(50_000), "b".repeat(50_000));
        for max in [0usize, 1, 10, 50, 149, 150] {
            let (out, _) = truncate_head_tail(&content, max, 2000, None);
            assert!(out.len() <= max, "max={max} gave {}", out.len());
        }
    }

    /// Byte windows cut on character boundaries: no replacement character
    /// and no panic from a mid-character slice.
    #[test]
    fn windows_cut_on_character_boundaries() {
        let content = "漢".repeat(20_000) + &"😀".repeat(20_000);
        for max in [300usize, 1000, 4096] {
            let (out, _) = truncate_head_tail(&content, max, 2000, None);
            assert!(out.len() <= max);
            assert!(!out.contains('\u{fffd}'));
        }
    }
}
