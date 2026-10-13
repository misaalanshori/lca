//! Balanced head-and-tail truncation (gh #216, OpenCode's
//! `ToolOutput.truncate` shape): oversized output keeps its head
//! (invocation, flags, first errors) and its tail (summary, exit
//! state), with an explicit middle-omission marker citing line counts
//! and the attachment holding the full text.

/// Reserve for the omission marker, so head + tail + marker honor the
/// byte budget together.
const MARKER_RESERVE: usize = 150;

/// Split oversized `content` into head + omission marker + tail.
/// Returns the content unaltered with `false` when it fits both
/// budgets. Head takes ~40% of the remaining budget (whole lines),
/// tail ~60% (the summary end matters more). The total emitted bytes
/// never exceed `max_bytes`.
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
    let budget_lines = max_lines;
    // Whole lines, byte-boundary safe: walk the head forward, the tail
    // backward, splitting the budgets 40/60 (saturating: callers may
    // pass unbounded lines when bytes alone govern).
    let head_line_budget = budget_lines.saturating_mul(2) / 5;
    let mut head_end = 0usize;
    let mut head_bytes = 0usize;
    for (index, line) in lines.iter().enumerate() {
        if index >= head_line_budget {
            break;
        }
        let cost = line.len() + 1;
        if head_bytes + cost > budget_bytes.saturating_mul(2) / 5 && index > 0 {
            break;
        }
        head_bytes += cost;
        head_end = index + 1;
    }
    let tail_line_budget = budget_lines.saturating_sub(head_end);
    let mut tail_start = total_lines;
    let mut tail_bytes = 0usize;
    for (back, line) in lines.iter().rev().enumerate() {
        if back >= tail_line_budget {
            break;
        }
        let cost = line.len() + 1;
        if tail_bytes + cost > budget_bytes.saturating_sub(head_bytes) && back > 0 {
            break;
        }
        tail_bytes += cost;
        tail_start = total_lines - back - 1;
    }
    // The windows must not overlap (tiny budgets, huge lines): the
    // tail yields to the head, and the marker covers the rest.
    let tail_start = tail_start.max(head_end);
    let omitted = total_lines.saturating_sub(head_end + (total_lines - tail_start));
    let saved = match attachment_ref {
        Some(reference) => format!("; full output saved to {reference}"),
        None => String::new(),
    };
    let marker = format!(
        "\n[... showing lines 1..{head_end} and {}..{total_lines} of {total_lines} ({omitted} lines omitted{saved}) ...]\n",
        tail_start + 1,
    );
    let mut out = String::new();
    out.push_str(&lines[..head_end].join("\n"));
    if head_end > 0 {
        out.push('\n');
    }
    out.push_str(&marker);
    out.push_str(&lines[tail_start..].join("\n"));
    (out, true)
}
