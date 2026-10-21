//! gh #391 gap 1: collapsed previews slice **visual** lines, and shells keep
//! the **tail**.
//!
//! Split from `tests.rs` for the 1,200-line ceiling.

use super::preview::{
    build_preview, collapse_carriage_returns, keeps_tail, preview_lines, strip_truncation_footer,
};

/// A build log whose first rows are boilerplate and whose last row is the
/// only row a reader wants.
fn build_log() -> String {
    let mut text = String::new();
    for i in 0..40 {
        text.push_str(&format!("   Compiling crate-{i} v0.1.0\n"));
    }
    text.push_str("test result: FAILED. 12 passed; 1 failed\n");
    text
}

// Verifies: gh #391 gap 1 - the collapsed shell preview shows the summary at
// the tail, not the `Compiling …` boilerplate at the head. Before the fix
// `result.lines().take(5)` kept five `Compiling` rows and hid the result.
#[test]
fn a_shell_preview_keeps_the_tail_not_the_boilerplate() {
    let preview = build_preview("shell", &build_log(), 60);
    let joined = preview.rows.join("\n");
    assert!(
        joined.contains("test result: FAILED"),
        "the summary must survive; got:\n{joined}"
    );
    assert!(
        !joined.contains("Compiling crate-0 "),
        "the first line is boilerplate and must be dropped:\n{joined}"
    );
    assert_eq!(preview.rows.len(), preview_lines("shell"));
    assert_eq!(
        preview.omitted, 36,
        "40 boilerplate rows + summary - 5 tail"
    );
    assert!(
        preview.marker_above,
        "a tail preview names its omission above"
    );
}

// Verifies: gh #391 gap 1 - a non-shell preview keeps the **head** (pi's
// `keep: "start"`): `ls` and `grep` read top-down, so the first rows are the
// useful ones and the marker belongs below.
#[test]
fn a_listing_preview_keeps_the_head_with_the_marker_below() {
    let text: String = (0..30).map(|i| format!("entry-{i}\n")).collect();
    let preview = build_preview("ls", &text, 60);
    assert!(
        preview.rows[0].contains("entry-0"),
        "a listing keeps its head: {:?}",
        preview.rows
    );
    assert!(
        !preview.rows.iter().any(|r| r.contains("entry-29")),
        "the tail is dropped for a head preview: {:?}",
        preview.rows
    );
    assert!(
        !preview.marker_above,
        "a head preview names its omission below"
    );
}

// Verifies: gh #391 gap 1 - **the wrap-before-slice rule.** One logical line
// of 4,000 characters wraps to far more than the budget; slicing logical
// lines first would keep all of it and blow the card open. This is the
// defect's headline symptom.
#[test]
fn one_enormous_logical_line_still_collapses_to_the_budget() {
    let minified = format!("[{}]", "1,".repeat(2_000));
    assert!(minified.len() > 3_000, "the fixture is a long single line");
    assert_eq!(
        minified.lines().count(),
        1,
        "and it really is one logical line"
    );
    let preview = build_preview("shell", &minified, 60);
    assert_eq!(
        preview.rows.len(),
        5,
        "five *visual* rows, not fifty: {:?}",
        preview.rows
    );
    assert!(
        preview.omitted > 40,
        "the omitted count is in visual rows ({}), which is what the user sees",
        preview.omitted
    );
}

// Verifies: gh #391 gap 1 - the per-width cache is really per width: the same
// output re-slices when the card narrows, because more visual rows exist.
#[test]
fn a_narrower_width_slices_more_rows() {
    let text: String = (0..12)
        .map(|i| format!("a fairly long output line number {i}\n"))
        .collect();
    let wide = build_preview("shell", &text, 120);
    let narrow = build_preview("shell", &text, 24);
    assert_eq!(wide.rows.len(), 5);
    assert_eq!(narrow.rows.len(), 5);
    assert!(
        narrow.omitted > wide.omitted,
        "a narrow card wraps into more rows, so it omits more: {} vs {}",
        narrow.omitted,
        wide.omitted
    );
}

// Verifies: gh #391 gap 1 - trailing blank rows are not content. A command
// that ends with a newline must not spend one of five rows on the blank, or a
// build log's tail is four real rows plus nothing.
#[test]
fn trailing_blank_lines_do_not_eat_the_budget() {
    let text = format!("{}\n\n\n\n", build_log());
    let preview = build_preview("shell", &text, 60);
    assert_eq!(preview.rows.len(), 5);
    assert!(
        preview
            .rows
            .last()
            .is_some_and(|r| r.contains("test result")),
        "the last kept row is the summary, not a blank: {:?}",
        preview.rows
    );
}

// Verifies: gh #391 gap 1 - a short preview is not marked as truncated.
#[test]
fn a_short_output_is_not_marked_truncated() {
    let preview = build_preview("shell", "ok\n", 60);
    assert_eq!(preview.omitted, 0);
    assert_eq!(preview.rows, vec!["ok".to_string()]);
}

// Verifies: gh #391 gap 1 - tools with no preview budget show nothing (the
// caller prints its own one-line hint), and the helper says so.
#[test]
fn an_unpreviewed_tool_has_no_budget_and_no_tail_rule() {
    assert_eq!(preview_lines("read"), 0);
    assert!(!keeps_tail("read"));
    assert_eq!(preview_lines("shell"), 5);
    assert!(keeps_tail("shell"));
}

// --- gh #391 gap 4: the truncation footer -------------------------------

// Verifies: gh #391 gap 4 - the LLM-facing spillover footer is stripped from
// the preview. It must not eat two of five tail rows, and the card already
// renders the same fact as its own styled warning (the #328 family).
#[test]
fn the_truncation_footer_is_stripped_from_the_preview() {
    let with_footer = "line one\nline two\n\n[Output truncated. Full output: /tmp/lca-out-1.log]";
    assert_eq!(strip_truncation_footer(with_footer), "line one\nline two");
    let preview = build_preview("shell", with_footer, 60);
    let joined = preview.rows.join("\n");
    assert!(
        !joined.contains("Full output"),
        "the footer never reaches the preview: {joined}"
    );
    assert_eq!(preview.rows.len(), 2, "and it costs no row");
}

// Verifies: gh #391 gap 4 - an ordinary log line that happens to contain a
// bracket is **not** treated as a footer. The strip matches the footer's own
// shape, not any `\n\n[`.
#[test]
fn an_ordinary_bracketed_line_survives() {
    let text = "before\n\n[INFO] a normal log line\n after";
    assert_eq!(strip_truncation_footer(text), text);
}

// Verifies: gh #391 gap 4 - **LCA's own footer shape** is stripped. This is
// the one `lca-tools` actually writes (`lib.rs:299`):
// `\n[full output: N bytes, attachment <hash>]`. Matching only pi's
// `\n\n[Full output: /path]` spelling would have left the real footer in the
// preview eating tail rows - found by reading the tool layer instead of
// trusting the issue's quoted shape.
#[test]
fn the_footer_this_tree_actually_writes_is_stripped() {
    let text = "real line 1\nreal line 2\n[full output: 9192 bytes, attachment abc123]";
    assert_eq!(
        strip_truncation_footer(text),
        "real line 1\nreal line 2",
        "LCA's own footer shape is the one that matters"
    );
    // And a line that merely mentions an attachment is not a footer.
    let ordinary = "real\n[see attachment abc123]";
    assert_eq!(strip_truncation_footer(ordinary), ordinary);
}

// Verifies: gh #391 gap 4 - the tool layer's other footer spelling is
// stripped too, so a build that writes `[Output truncated` is covered.
#[test]
fn the_other_footer_spelling_is_stripped_too() {
    let text = "real output\n\n[Output truncated to 2000 lines]";
    assert_eq!(strip_truncation_footer(text), "real output");
}

// --- output hygiene ----------------------------------------------------

// Verifies: gh #391's output-hygiene note - `\r` progress overwrites collapse
// to their final visual state, so 50 Hz repaints do not become joined
// garbage. The terminal showed only the last segment; the preview keeps it.
#[test]
fn carriage_return_progress_collapses_to_the_last_state() {
    assert_eq!(collapse_carriage_returns("foo\rbar"), "bar");
    assert_eq!(
        collapse_carriage_returns("10%\r55%\r100% done"),
        "100% done"
    );
    assert_eq!(collapse_carriage_returns("a\n1%\r2%\nb"), "a\n2%\nb");
    // No CR: untouched, and identical (the fast path).
    assert_eq!(collapse_carriage_returns("plain\ntext"), "plain\ntext");
}
