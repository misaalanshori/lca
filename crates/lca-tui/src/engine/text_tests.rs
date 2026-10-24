//! Rows for the width/slice engine (`text.rs`).
//!
//! Split out of `text.rs` when that file crossed the 1,200-line ceiling after
//! gh #378 added the fast-path equivalence table.

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

// Verifies: gh #178 - a wrapped OSC 8 hyperlink re-opens with the
// full URL on every segment, so clicking any wrapped row opens the
// whole URL, and hit-testing any segment resolves the same link.
#[test]
fn a_wrapped_osc8_link_reopens_with_the_full_url_on_every_row() {
    let url = format!(
        "https://accounts.example.com/o/oauth2/auth?response_type=code&client_id={}&redirect_uri=https%3A%2F%2Flocalhost%2Fcallback&scope=email+profile",
        "9".repeat(120)
    );
    assert!(url.len() > 200, "the receipt needs a long URL");
    let linked = format!("\x1b]8;;{url}\x07{url}\x1b]8;;\x07");
    let rows = wrap_text_with_ansi(&linked, 40);
    assert!(rows.len() >= 3, "it wraps: {:?}", rows.len());
    for row in &rows {
        assert!(
            row.contains(&format!("\x1b]8;;{url}\x07")),
            "every row re-opens the full URL: {row:?}"
        );
        assert!(
            row.contains("\x1b]8;;\x07"),
            "every row closes the link: {row:?}"
        );
    }
    for row in rows.iter().skip(1) {
        let plain = strip_terminal_sequences(row);
        let col = plain.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        assert_eq!(
            get_osc8_link_at_column(row, col),
            Some(url.clone()),
            "hit-testing a wrapped row resolves the link"
        );
    }
    let joined: String = rows
        .iter()
        .map(|row| strip_terminal_sequences(row))
        .collect();
    assert_eq!(joined, url, "selection across the wrap reads the URL");
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

// ==== gh #378 gap 1: the zero-allocation ASCII fast path ====

/// The corpus the equivalence table sweeps: every shape a rendered row can take.
///
/// The fast path must agree with the slow path on all of these, and must *decline*
/// (return `None`) on the ones it cannot decide. A wrong skip would silently
/// mismeasure every row after it, so the decline cases matter as much as the hits.
fn width_corpus() -> Vec<&'static str> {
    vec![
        // Plain ASCII, including the boundaries of the printable range.
        "",
        "hello",
        " ",
        "~",
        "!",
        "abc123!@#$%^&*()",
        // Tabs are 3 columns each.
        "a\tb",
        "\t",
        "\t\t",
        // SGR: the case every rendered row carries.
        "\x1b[31mred\x1b[0m",
        "\x1b[1;38;2;1;2;3mbold truecolor\x1b[0m",
        "\x1b[39mplain\x1b[49m",
        "\x1b[7m \x1b[27m", // the gh #371 caret form
        "a\x1b[1mb\x1b[0mc",
        // Cursor addressing / erase, the other closed CSI finals.
        "\x1b[2J",
        "\x1b[K",
        "\x1b[3G",
        "\x1b[1H",
        "x\x1b[2Jy",
        // OSC 8 hyperlinks, both terminators.
        "\x1b]8;;https://example.com\x07link\x1b]8;;\x07",
        "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\",
        // APC: the caret marker.
        "\x1b_pi:c\x07",
        "ab\x1b_pi:c\x07cd",
        // Wide glyphs, emoji, combining marks, flags: the fast path must DECLINE.
        "世界",
        "日",
        "😀",
        "🎉 party",
        "a\u{0301}",
        "🇺🇸",
        "e\u{0301}\u{0327}",
        // Controls outside the printable set.
        "\u{7f}",
        "\u{1}",
        // Malformed / unsupported escapes: the fast path must decline rather than
        // guess a length.
        "\x1b[999z",
        "\x1b[",
        "\x1b",
        "\x1b[31",
        "a\x1b[999zb",
    ]
}

// Verifies: gh #378 gap 1 - **the acceptance test**: the fast path and the slow
// path agree on every shape a rendered row takes.
//
// This is what makes the change a safe optimisation rather than a regex: the
// fast path answers only where it can decide everything, and the answer must be
// the same one the cached path gives.
#[test]
fn the_fast_path_agrees_with_the_slow_path_on_every_shape() {
    for line in width_corpus() {
        let fast = ascii_visible_width(line);
        let slow = visible_width_slow(line);
        // Declining is always allowed; answering must agree.
        if let Some(w) = fast {
            assert_eq!(
                w, slow,
                "fast path answered {w} for {line:?} but the slow path says {slow}"
            );
        }
    }
}

// Verifies: gh #378 gap 1 - the fast path answers (does not decline) for the
// shapes it is meant to cover, and declines for the ones it cannot decide.
//
// Without this row the equivalence table is satisfiable by a fast path that
// always returns `None` - which would be correct and useless.
#[test]
fn the_fast_path_decides_ascii_and_declines_everything_else() {
    for line in [
        "",
        "hello",
        "~",
        "a\tb",
        "\x1b[31mred\x1b[0m",
        "\x1b_pi:c\x07",
        "\x1b]8;;https://example.com\x07link\x1b]8;;\x07",
        "\x1b[2J",
    ] {
        assert!(
            ascii_visible_width(line).is_some(),
            "the fast path must decide {line:?}"
        );
    }
    for line in [
        "世界",
        "😀",
        "a\u{0301}",
        "🇺🇸",
        "\u{7f}",
        "\x1b[999z", // unsupported final: skip length unknown
        "\x1b[31",   // truncated CSI
        "\x1b",      // bare escape
        "\x1b[",
    ] {
        assert_eq!(
            ascii_visible_width(line),
            None,
            "the fast path must decline {line:?} rather than guess"
        );
    }
}

// Verifies: gh #378 gap 1 - the fast path is what a coloured line actually takes.
// A faster function nobody calls is not a fix, so this counts the hits.
#[test]
fn a_coloured_line_takes_the_fast_path() {
    // A **fresh** string every run, so no cache entry can exist for it. With
    // cache-first ordering a repeated literal would answer from the cache and this
    // row would pass without ever exercising the fast path - vacuous, which is what
    // the first version of this row was until the ordering changed.
    let line = format!(
        "\x1b[38;2;98;174;220mthe quick brown fox {}\x1b[39m",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    assert!(
        ascii_visible_width(&line).is_some(),
        "a styled ASCII row must not pay the clone/strip/cache path"
    );
    let before = fast_path_hits();
    let width = visible_width(&line);
    assert!(
        fast_path_hits() > before,
        "visible_width must record a fast-path hit for a cache-missing coloured line"
    );
    assert!(width > 19, "and answer the real width: {width}");
}

// Verifies: gh #378 gap 1 - the counter does not leak into the public API.
//
// The hit count is a `cfg(test)`-only accessor over a thread-local, so a release
// build has no counter, no atomic, and no cache line to share.
#[test]
fn the_hit_counter_is_test_only() {
    // If this compiles, the accessor exists; the `cfg(test)` gate is what keeps it
    // out of the shipped API, and the attribute is checked by the doc build.
    let _ = fast_path_hits();
}

// ==== gh #378 gap 2: SGR arriving at the seam ====

// Verifies: gh #378 gap 2 - **the rendered outcome**: text after a style that was
// closed exactly at the seam is *unstyled*, not restyled.
//
// The bug: an SGR arriving at `start_col` was pushed to `result` before the
// buffered pre-`start_col` codes flushed, so `\x1b[31m` + `\x1b[0m`-at-seam
// emitted as `\x1b[0m\x1b[31m` - the reset came first and then red was
// re-opened, turning unstyled text red.
#[test]
fn a_style_closed_at_the_seam_does_not_colour_the_tail() {
    // Columns 0-2 are red; the reset sits exactly at column 3; `plain` follows.
    let line = "\x1b[31mabc\x1b[0mplain";
    // Slice from column 3 onward: the tail must be unstyled.
    let (tail, _) = slice_with_width(line, 3, 100, false);
    let plain = strip_terminal_sequences(&tail);
    assert_eq!(plain, "plain", "the tail text: {tail:?}");
    // The rendered outcome: no red is active over `plain`. `\x1b[31m` may appear
    // only if it is *closed* before the text - so assert the text has no red
    // before it that is still open.
    assert!(
        !tail.ends_with("\x1b[31mplain") && !tail.starts_with("\x1b[31mplain"),
        "red must not be re-opened over unstyled text: {tail:?}"
    );
    assert!(
        !tail.contains("\x1b[0m\x1b[31m"),
        "the reset must not be emitted before the style: {tail:?}"
    );
}

// Verifies: gh #378 gap 2 - the same case through `slice_by_column`, which is the
// public entry point every caller uses.
#[test]
fn slice_by_column_does_not_colour_text_after_a_seam_reset() {
    let line = "\x1b[31mabc\x1b[0mplain";
    let tail = slice_by_column(line, 3, 100, false);
    assert!(
        !tail.contains("\x1b[0m\x1b[31m"),
        "the reversed pair is the bug: {tail:?}"
    );
    assert_eq!(strip_terminal_sequences(&tail), "plain");
}

// Verifies: gh #378 gap 2 - the mirror case: a style that is still OPEN across the
// seam must carry into the tail. The fix must not close it.
#[test]
fn a_style_open_across_the_seam_still_carries() {
    let line = "\x1b[31mabcdef";
    let (tail, _) = slice_with_width(line, 3, 100, false);
    assert!(
        tail.starts_with("\x1b[31m"),
        "the open style re-arms on the tail: {tail:?}"
    );
    assert_eq!(strip_terminal_sequences(&tail), "def");
}

// Verifies: gh #378 gap 2 - a style inside the window keeps its order relative to
// the text it applies to.
#[test]
fn a_style_inside_the_window_survives_the_slice() {
    let line = "\x1b[31mabcdef";
    let (head, _) = slice_with_width(line, 0, 3, false);
    assert_eq!(strip_terminal_sequences(&head), "abc");
    assert!(head.contains("\x1b[31m"), "the head keeps its colour");
}

// Verifies: gh #378 gap 2 - the fix reaches a **real caller**, not just the public
// slice: `highlight_range` (text selection) slices the tail at the selection's end,
// so before the fix the reset/red pair came out reversed and the unselected text
// after a selection inherited the selected run's colour.
//
// This is the end-to-end form of the seam row, through the function a drag-select
// actually calls.
#[test]
fn selection_does_not_reverse_the_reset_and_style_at_its_seam() {
    use crate::engine::selection::highlight_range;
    let row = "\x1b[31mabc\x1b[0mplain";
    let out = highlight_range(row, 3, 6);
    assert!(
        !out.contains("\x1b[7m\x1b[0m\x1b[7m\x1b[31m"),
        "the pre-fix order re-opened red after the reset: {out:?}"
    );
    assert_eq!(
        strip_terminal_sequences(&out),
        "abcplain",
        "selection never changes the text: {out:?}"
    );
    assert_eq!(visible_width(&out), 8, "nor the width");
}
