//! Regression rows for gh #321: `truncate_head_tail` must honor
//! `max_bytes` strictly, including when a single line is longer than a
//! whole budget.
//!
//! The defect: the head loop guarded with `&& index > 0` and the tail loop
//! with `&& back > 0`, so the *first* and *last* line were always admitted
//! regardless of length. A one- or two-line minified file (the motivating
//! case: a 10 MB `bundle.min.js` on one line) was therefore emitted whole
//! and the tool result blew past `tool.result_limit_bytes`.
//!
//! Verifies: gh #321 - the emitted bytes never exceed `max_bytes`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_tools::truncate::truncate_head_tail;

/// The defect's own shape: two 100 KiB lines against a 10 KiB budget.
#[test]
fn a_two_line_file_of_huge_lines_stays_within_the_byte_budget() {
    let line = "x".repeat(100 * 1024);
    let content = format!("{line}\n{line}");
    let max = 10 * 1024;
    let (out, truncated) = truncate_head_tail(&content, max, 2000, None);
    assert!(truncated, "an oversized input reports truncation");
    assert!(
        out.len() <= max,
        "the result fits the byte budget: {} > {max}",
        out.len()
    );
}

/// A single line larger than the entire budget - no line boundary to
/// fall back on, so the cut has to happen inside the line.
#[test]
fn a_single_line_larger_than_the_budget_is_still_bounded() {
    let line = "y".repeat(100 * 1024);
    let max = 10 * 1024;
    let (out, truncated) = truncate_head_tail(&line, max, 2000, None);
    assert!(truncated, "an oversized input reports truncation");
    assert!(
        out.len() <= max,
        "the result fits the byte budget: {} > {max}",
        out.len()
    );
}

/// Both ends of the oversized line must survive: the head shows where the
/// output started and the tail shows how it ended.
#[test]
fn a_long_single_line_keeps_both_its_head_and_its_tail() {
    let content = format!("HEAD-MARKER{}{}", "m".repeat(100 * 1024), "TAIL-MARKER");
    let max = 4096;
    let (out, _) = truncate_head_tail(&content, max, 2000, None);
    assert!(out.len() <= max, "bounded: {}", out.len());
    assert!(
        out.starts_with("HEAD-MARKER"),
        "the head of the content survives: {:?}",
        out.chars().take(20).collect::<String>()
    );
    assert!(
        out.ends_with("TAIL-MARKER"),
        "the tail of the content survives: {:?}",
        out.chars().rev().take(20).collect::<String>()
    );
}

/// Every cut lands on a UTF-8 character boundary. A cut inside a
/// multi-byte character would either panic on slicing or send U+FFFD to a
/// renderer that then miscounts the row.
#[test]
fn the_cut_lands_on_a_character_boundary_for_multibyte_text() {
    // 3-byte CJK and 4-byte emoji, so both boundary shapes are exercised.
    let content = "漢".repeat(40_000) + &"😀".repeat(40_000);
    for max in [300usize, 1000, 4096, 10_000] {
        let (out, truncated) = truncate_head_tail(&content, max, 2000, None);
        assert!(truncated, "oversized at max={max}");
        assert!(out.len() <= max, "bounded at max={max}: {}", out.len());
        // A `String` is UTF-8 by construction; the real check is that no
        // replacement character was fabricated by a mid-character cut.
        assert!(
            !out.contains('\u{fffd}'),
            "no replacement character at max={max}"
        );
    }
}

/// A budget smaller than the omission marker itself: the contract is still
/// "at most `max_bytes`", and still valid UTF-8.
#[test]
fn a_budget_smaller_than_the_marker_is_still_bounded() {
    let content = format!("{}\n{}", "a".repeat(50_000), "b".repeat(50_000));
    for max in [0usize, 1, 10, 50, 149] {
        let (out, truncated) = truncate_head_tail(&content, max, 2000, None);
        assert!(truncated, "oversized at max={max}");
        assert!(out.len() <= max, "bounded at max={max}: {}", out.len());
        assert!(!out.contains('\u{fffd}'), "valid characters at max={max}");
    }
}

/// The normal case must be untouched. This row is the guard against
/// "fixed #321 by changing what every truncated result looks like": whole
/// lines, a citation marker, and head+tail both present.
#[test]
fn ordinary_multi_line_output_truncates_exactly_as_before() {
    let lines: Vec<String> = (1..=2000).map(|n| format!("line {n:04}")).collect();
    let content = lines.join("\n");
    let (out, truncated) = truncate_head_tail(&content, 4096, 200, Some("attachments/abc123"));
    assert!(truncated, "oversized truncates");
    assert!(out.len() <= 4096, "within the byte budget: {}", out.len());
    assert!(out.contains("line 0001"), "head kept");
    assert!(out.contains("line 2000"), "tail kept");
    assert!(!out.contains("line 1000"), "middle omitted");
    assert!(
        out.contains("lines omitted"),
        "the marker names the omission"
    );
    assert!(out.contains("of 2000"), "the marker counts total lines");
    assert!(
        out.contains("attachments/abc123"),
        "the marker cites the attachment"
    );
}

/// One line over budget plus many normal lines: the oversized line is
/// sliced, and the normal lines still contribute their whole-line windows.
#[test]
fn a_long_first_line_does_not_swallow_a_normal_tail() {
    let content = format!(
        "{}\n{}",
        "z".repeat(50_000),
        (1..=10)
            .map(|n| format!("tail {n}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let max = 2048;
    let (out, _) = truncate_head_tail(&content, max, 2000, None);
    assert!(out.len() <= max, "bounded: {}", out.len());
    assert!(out.contains("tail 10"), "the tail is still shown: {out:?}");
}

/// A long last line must not swallow the whole normal head either.
#[test]
fn a_long_last_line_does_not_swallow_a_normal_head() {
    let content = format!(
        "{}\n{}",
        (1..=10)
            .map(|n| format!("head {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "z".repeat(50_000)
    );
    let max = 2048;
    let (out, _) = truncate_head_tail(&content, max, 2000, None);
    assert!(out.len() <= max, "bounded: {}", out.len());
    assert!(out.contains("head 1"), "the head is still shown: {out:?}");
}

/// The marker must describe what was actually kept. In byte-window mode
/// there is no line range to cite, so a line-range marker would print
/// nonsense like `showing lines 1..0 and 3..2 of 2` - which is exactly what
/// the first draft of this fix emitted.
#[test]
fn the_marker_describes_a_byte_window_honestly() {
    let line = "z".repeat(100 * 1024);
    let content = format!("{line}\n{line}");
    let (out, _) = truncate_head_tail(&content, 4096, 2000, None);
    assert!(
        !out.contains("showing lines 1..0"),
        "no impossible line range: {out:?}"
    );
    assert!(
        out.contains("bytes"),
        "the marker cites bytes when it kept bytes: {out:?}"
    );
    assert!(
        out.contains("a single line is longer than the budget"),
        "the marker explains why it could not keep whole lines: {out:?}"
    );
}

/// A line-aligned result keeps the line-range marker, with the attachment
/// reference where there is one.
#[test]
fn the_marker_keeps_its_line_range_when_whole_lines_were_kept() {
    let lines: Vec<String> = (1..=2000).map(|n| format!("line {n:04}")).collect();
    let (out, _) = truncate_head_tail(&lines.join("\n"), 4096, 200, Some("attachments/deadbeef"));
    assert!(
        out.contains("showing lines 1.."),
        "the line range is cited: {out:?}"
    );
    assert!(out.contains("of 2000"), "the total is cited: {out:?}");
    assert!(
        out.contains("attachments/deadbeef"),
        "the attachment is cited: {out:?}"
    );
    assert!(
        !out.contains("bytes;"),
        "the byte marker is not used for a line-aligned result: {out:?}"
    );
}

/// The contract `truncate_head_tail` owns is the *content* budget: the
/// spill note (`[full output: N bytes, attachment <hash>]`) is appended by
/// the caller afterwards and is not part of it. This row pins that
/// separation, so a future reader comparing the finished tool result to
/// `tool.result_limit_bytes` knows which number it should see (measured
/// live: 65535 bytes of content under a 65536 cap).
#[test]
fn the_byte_budget_bounds_the_content_not_the_spill_note() {
    let line = "abcdefghij".repeat(10240);
    let content = format!("{line}\n{line}");
    let cap = 65536;
    let (out, _) = truncate_head_tail(&content, cap, 2000, None);
    assert!(
        out.len() <= cap,
        "the content fits the cap exactly: {} > {cap}",
        out.len()
    );
    assert!(
        out.len() > cap - 512,
        "and it uses the budget rather than under-filling it: {}",
        out.len()
    );
}
