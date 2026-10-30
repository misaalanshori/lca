//! Gh #328: an expanded diff card printed the result twice.
//!
//! Split from `tests.rs` under gate 11's 1,200-line ceiling, and along a real seam: every
//! row here is about the **diff-versus-output choice** (`render.rs`'s `Body`), while
//! `tests.rs` covers the transcript's state and the rest of its rendering.
//!
//! The defect: the diff arm is `if let Some(diff)` and the general output block right
//! after it is `if let Some(result)` - no `else`. So a result carrying a diff rendered the
//! diff **and then** the general block over the same `result`, and a long diff looked like
//! the tool had done the work twice.

use super::*;
use lca_tui::engine::text::strip_terminal_sequences;

fn strip(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| strip_terminal_sequences(l)).collect()
}

/// How many rendered rows contain `needle`.
fn count_rows(rows: &[String], needle: &str) -> usize {
    strip(rows)
        .iter()
        .filter(|row| row.contains(needle))
        .count()
}

/// A tool entry finished with a structured diff, the shape `edit` produces (gh #9).
fn diff_card(summary: &str, diff: &str) -> Transcript {
    let mut t = Transcript::new();
    t.start_tool("edit", r#"{"path":"src/main.rs"}"#);
    t.finish_tool_with_diff(
        ToolStatus::Ok,
        Some(summary.to_string()),
        Some(diff.to_string()),
    );
    t
}

const SMALL_DIFF: &str = "--- src/main.rs\n+++ src/main.rs\n@@ -1,3 +1,3 @@\n fn a() {}\n-fn b() {}\n+fn bb() {}\n fn c() {}\n";
const SUMMARY: &str = "Successfully replaced 1 block(s) in src/main.rs.";

// Verifies: gh #328 - an **expanded** diff card renders its changes once.
//
// The removed line is the distinctive needle: it comes from the diff, so one occurrence
// means the diff arm drew it and the general block did not draw it again.
#[test]
fn an_expanded_diff_card_renders_its_changes_once() {
    let theme = Theme::plain();
    let mut t = diff_card(SUMMARY, SMALL_DIFF);
    t.tools_expanded = true;
    let rows = t.render(80, &theme);
    assert_eq!(
        count_rows(&rows, "-fn b() {}"),
        1,
        "the removed line appears exactly once:\n{}",
        strip(&rows).join("\n")
    );
    assert_eq!(
        count_rows(&rows, "+fn bb() {}"),
        1,
        "and so does the added line:\n{}",
        strip(&rows).join("\n")
    );
}

// Verifies: gh #328 - the **summary stays**. This is the over-correction row: the summary
// is the card's one-line claim and the diff is its evidence, so a fix that dropped it
// would trade a duplicate for a card that says nothing at a glance.
#[test]
fn an_expanded_diff_card_keeps_its_summary() {
    let theme = Theme::plain();
    let mut t = diff_card(SUMMARY, SMALL_DIFF);
    t.tools_expanded = true;
    let rows = t.render(80, &theme);
    assert_eq!(
        count_rows(&rows, SUMMARY),
        1,
        "the summary appears exactly once:\n{}",
        strip(&rows).join("\n")
    );
}

// Verifies: gh #328 - the **collapsed** state is unchanged: it shows the summary and a
// bounded diff preview, with no general output block.
#[test]
fn a_collapsed_diff_card_is_unchanged() {
    let theme = Theme::plain();
    let t = diff_card(SUMMARY, SMALL_DIFF);
    let rows = t.render(80, &theme);
    assert_eq!(
        count_rows(&rows, SUMMARY),
        1,
        "the summary shows once when collapsed:\n{}",
        strip(&rows).join("\n")
    );
    assert_eq!(
        count_rows(&rows, "-fn b() {}"),
        1,
        "and the preview shows the change:\n{}",
        strip(&rows).join("\n")
    );
}

// Verifies: gh #328 design point 4 - a **non-diff** result is unchanged. The control row:
// the general output block still renders it exactly as before, so the fix cannot have
// been "stop rendering results".
#[test]
fn a_non_diff_result_still_renders_its_output() {
    let theme = Theme::plain();
    let mut t = Transcript::new();
    t.start_tool("grep", r#"{"pattern":"needle"}"#);
    t.finish_tool(
        ToolStatus::Ok,
        Some("src/a.rs:12: needle here\nsrc/b.rs:34: needle too\n".into()),
    );
    t.tools_expanded = true;
    let rows = t.render(80, &theme);
    assert_eq!(
        count_rows(&rows, "src/a.rs:12: needle here"),
        1,
        "the output line renders once, as it always did:\n{}",
        strip(&rows).join("\n")
    );
    assert_eq!(
        count_rows(&rows, "src/b.rs:34: needle too"),
        1,
        "and the second line too:\n{}",
        strip(&rows).join("\n")
    );
}

// Verifies: gh #328 design point 3 - a result that is **partly a diff and partly a
// trailing note** keeps the note.
//
// A tool may report a structured diff *and* a line of prose that is not in it (a warning,
// a summary of what else changed). Dropping the general block would delete that line, so
// the row states that it survives - once.
#[test]
fn a_partly_diff_result_keeps_its_trailing_note() {
    let theme = Theme::plain();
    let note = "note: 2 further files were skipped";
    let mut t = diff_card(SUMMARY, SMALL_DIFF);
    t.tools_expanded = true;
    // The result carries both the summary and a note the diff does not contain.
    t.entries.clear();
    t.start_tool("edit", r#"{"path":"src/main.rs"}"#);
    t.finish_tool_with_diff(
        ToolStatus::Ok,
        Some(format!("{SUMMARY}\n{note}")),
        Some(SMALL_DIFF.to_string()),
    );
    let rows = t.render(80, &theme);
    assert_eq!(
        count_rows(&rows, note),
        1,
        "the note the diff does not contain survives, exactly once:\n{}",
        strip(&rows).join("\n")
    );
    assert_eq!(
        count_rows(&rows, "-fn b() {}"),
        1,
        "and the diff is still once:\n{}",
        strip(&rows).join("\n")
    );
}

// Verifies: gh #328 design point 6 - a **truncated** diff result's marker appears exactly
// once.
//
// The marker comes from #216's head-and-tail truncation, which the general block draws.
// If the fix removes that block wholesale, the marker disappears and the reader is told
// nothing about the truncation.
#[test]
fn a_truncated_diffs_marker_appears_once() {
    let theme = Theme::plain();
    // A long diff, so the card's own preview reports the remainder.
    let mut diff = String::from("--- src/main.rs\n+++ src/main.rs\n@@ -1,60 +1,60 @@\n");
    for index in 0..60 {
        diff.push_str(&format!("-fn old_{index}() {{}}\n+fn new_{index}() {{}}\n"));
    }
    let t = diff_card(SUMMARY, &diff);
    let rows = t.render(80, &theme);
    let marker_rows = strip(&rows)
        .iter()
        .filter(|row| row.contains("more lines"))
        .count();
    assert_eq!(
        marker_rows,
        1,
        "the 'more lines' marker appears exactly once when collapsed:\n{}",
        strip(&rows).join("\n")
    );
}

// Verifies: gh #328 - a tool whose **result text is itself the diff** (an extension patch
// tool, or any tool that reports the diff as prose) does not print the whole change twice.
//
// This is the case the issue describes most sharply, and it is distinct from `edit`'s:
// `edit` returns a one-line success message beside the structured diff, so its duplicate
// is a single row. A tool that puts the diff in `result` gets every line twice.
#[test]
fn a_result_that_is_the_diff_is_not_printed_twice() {
    let theme = Theme::plain();
    let mut t = Transcript::new();
    t.start_tool("patch", r#"{"path":"src/main.rs"}"#);
    // The result *is* the diff text, and the structured diff carries the same lines.
    t.finish_tool_with_diff(
        ToolStatus::Ok,
        Some(SMALL_DIFF.to_string()),
        Some(SMALL_DIFF.to_string()),
    );
    t.tools_expanded = true;
    let rows = t.render(80, &theme);
    // Every distinctive line of the diff, not just the change rows: a summary taken from
    // the result's first line would be a *header* copy, which counting only `-fn b() {}`
    // would miss. This is why the row asserts all four.
    for line in [
        "--- src/main.rs",
        "+++ src/main.rs",
        "@@ -1,3 +1,3 @@",
        "-fn b() {}",
    ] {
        assert_eq!(
            count_rows(&rows, line),
            1,
            "{line:?} prints once, not once per path:\n{}",
            strip(&rows).join("\n")
        );
    }
}

// Verifies: gh #328 design point 5 - a **nested** call does not duplicate either.
//
// The brief worried that #404's nesting is "a second composition site". It is not, and
// this row is the evidence rather than the assumption: a `ChildCall` carries a name, a
// status and a head, and `nested::render_checklist` draws exactly those - it never
// renders a child's result or diff, so there is nothing here to duplicate. The row pins
// that a parent with a diff-carrying child still shows its own change once and the child
// once, which is what "the fix holds there too" means concretely.
#[test]
fn a_nested_call_does_not_duplicate_a_diff() {
    let theme = Theme::plain();
    let mut t = Transcript::new();
    t.start_tool("codemode", r#"{"code":"…"}"#);
    t.finish_tool_with_diff(
        ToolStatus::Ok,
        Some(SUMMARY.to_string()),
        Some(SMALL_DIFF.to_string()),
    );
    let children = vec![super::nested::ChildCall {
        name: "edit".to_string(),
        status: super::nested::ChildStatus::Ok,
        head: String::new(),
    }];
    t.push_children(children);
    t.tools_expanded = true;
    let rows = t.render(80, &theme);
    assert_eq!(
        count_rows(&rows, "-fn b() {}"),
        1,
        "the parent's diff prints once:\n{}",
        strip(&rows).join("\n")
    );
    assert_eq!(
        count_rows(&rows, "edit"),
        1,
        "and the child appears once in the checklist:\n{}",
        strip(&rows).join("\n")
    );
}
