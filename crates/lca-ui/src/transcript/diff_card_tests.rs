//! Rows for the diff card's body (gh #392 slices A and B): chrome gone, a numbered
//! gutter, and the 1↔1 word spotlight.
//!
//! Every row here is about a **reader's** fact rather than the renderer's shape: is the
//! hunk header gone, does each row say which file line it is, is the changed *word* the
//! thing painted, is the indentation left alone, and does a row ever exceed the card.
//!
//! The SGR assertions use a coloured theme on purpose. The plain theme is identity, so a
//! plain-theme row cannot tell "the word is inverse" from "nothing is painted at all" -
//! the same reason gh #600's depth rows read the pane's raw bytes.

use super::*;
use crate::theme::Theme;

/// One hunk of a single-line change, the shape `edit` emits (gh #9's producer).
const ONE_WORD: &str = "--- src/main.rs\n+++ src/main.rs\n@@ -1,3 +1,3 @@\n fn a() {}\n-fn b() { return 1; }\n+fn b() { return 2; }\n fn c() {}\n";

fn strip(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| lca_tui::engine::text::strip_terminal_sequences(line))
        .collect()
}

// Verifies: gh #392 gap 5 - **the hunk's chrome is never printed.** `@@ -1,3 +1,3 @@`,
// `--- a/x` and `+++ b/x` are the producer's bookkeeping, not lines of the file: the old
// renderer painted all three as *context* rows, i.e. as though the file contained them.
// The card already names the path in its header, so a second copy is noise a reader has
// to skip on every changed file.
#[test]
fn the_hunk_chrome_is_never_printed() {
    let theme = Theme::plain();
    let rows = render(&parse(ONE_WORD), 80, &theme);
    for chrome in ["@@", "--- src/main.rs", "+++ src/main.rs"] {
        assert!(
            !strip(&rows).iter().any(|row| row.contains(chrome)),
            "the chrome {chrome:?} reached the card:\n{}",
            strip(&rows).join("\n")
        );
    }
    // And the three content rows survive, so the filter is not "drop everything".
    let joined = strip(&rows).join("\n");
    for content in ["fn a() {}", "fn b() { return 1; }", "fn b() { return 2; }"] {
        assert!(
            joined.contains(content),
            "the row {content:?} survives:\n{joined}"
        );
    }
}

// Verifies: gh #392 gap 5 - **every row says which file line it is.** A hunk that starts
// at 340 must number its rows 340, 341, 342, ... on the right side and give the removed
// row the old side's number: without this a reader cannot tell where in a 900-line file a
// change landed, which is the whole point of showing a diff.
#[test]
fn every_row_carries_its_file_line_number() {
    let theme = Theme::plain();
    let diff = "--- src/big.rs\n+++ src/big.rs\n@@ -340,3 +340,3 @@\n fn keep() {}\n-fn old() {}\n+fn new() {}\n fn tail() {}\n";
    let rows = strip(&render(&parse(diff), 80, &theme));
    let joined = rows.join("\n");
    // The removal carries the **old** number and the addition the **new** one; here they
    // coincide because nothing above the change was added or removed.
    assert!(
        joined.contains("- 341 fn old() {}"),
        "the removal's old number:\n{joined}"
    );
    assert!(
        joined.contains("+ 341 fn new() {}"),
        "the addition's new number:\n{joined}"
    );
    // Context rows are numbered too, and the counters walk: 340, 341, 342.
    assert!(
        joined.contains("  340 fn keep() {}"),
        "the first context row:\n{joined}"
    );
    assert!(
        joined.contains("  342 fn tail() {}"),
        "and the row after the change:\n{joined}"
    );
}

// Verifies: gh #392 gap 5 - **an insertion shifts the new side only.** After one added
// line, the next row's old and new numbers differ by one, which is the case a shared
// counter gets wrong. This is the row that would pass if the gutter printed a single
// number advanced once per row.
#[test]
fn an_insertion_numbers_the_two_sides_separately() {
    let theme = Theme::plain();
    let diff = "--- a\n+++ b\n@@ -10,4 +10,5 @@\n ctx\n+added\n ctx2\n ctx3\n ctx4\n";
    let rows = strip(&render(&parse(diff), 80, &theme)).join("\n");
    assert!(
        rows.contains("+ 11 added"),
        "the added row's new number:\n{rows}"
    );
    // `ctx2` is old line 11 and new line 12: the context row reports the new side.
    assert!(
        rows.contains("  12 ctx2"),
        "the context row after an insertion carries the new side's number:\n{rows}"
    );
    assert!(
        rows.contains("  13 ctx3"),
        "and the counter keeps walking:\n{rows}"
    );
}

// Verifies: gh #392 gap 5 - **a row before any hunk header prints no number rather than a
// wrong one.** A truncated or malformed patch (the streaming case, gap 7) can deliver a
// row whose position is genuinely unknown; inventing `0` or `1` there would be worse than
// a blank gutter, because the reader would trust it.
#[test]
fn a_row_before_any_hunk_header_prints_no_number() {
    let rows = parse("-orphan line\n");
    assert_eq!(rows.len(), 1, "the row is kept, not dropped");
    assert_eq!(rows[0].old, None, "no header, no number: {rows:?}");
    let rendered = strip(&render(&rows, 80, &Theme::plain()));
    assert_eq!(rendered.len(), 1);
    assert!(
        !rendered[0].chars().any(|c| c.is_ascii_digit()),
        "the gutter is blank rather than invented: {:?}",
        rendered[0]
    );
}

// Verifies: gh #392 gap 2 - **a one-word change paints the word, not the line.**
//
// The whole reason for the inverse span: at a glance, `return 1` → `return 2` should read
// as "the 1 became a 2". The row asserts the exact SGR sequence around each differing
// word (`\x1b[7mword\x1b[27m`) rather than "some inverse appears", so a span that wrapped
// the whole line cannot pass.
#[test]
fn a_one_word_change_paints_the_word_in_inverse() {
    let theme = Theme::colored();
    let rows = render(&parse(ONE_WORD), 80, &theme);
    let joined = rows.join("\n");
    assert!(
        joined.contains("\x1b[7m1\x1b[27m"),
        "the removed word is inverse: {:?}",
        rows
    );
    assert!(
        joined.contains("\x1b[7m2\x1b[27m"),
        "and so is the added one: {:?}",
        rows
    );
    // The unchanged words of the same line are **not** inverse: `return` and `fn b()` are
    // identical on both sides, so wrapping them would say the whole line changed.
    assert!(
        !joined.contains("\x1b[7mfn b()"),
        "the unchanged head is left alone: {:?}",
        rows
    );
    assert!(
        !joined.contains("\x1b[7mreturn"),
        "and the unchanged middle word too: {:?}",
        rows
    );
}

// Verifies: gh #392 gap 2 - **the indentation is emitted outside the inverse span.**
//
// pi's rule, and the difference between a diff card and a barcode: an indent that rides
// inside the span paints a solid bar down the left of every one-line change. The rows
// below have leading whitespace on both sides, and the assertion is that the inverse
// opens *after* the indent - i.e. the sequence is `"    \x1b[7m"`, never `"\x1b[7m    "`.
#[test]
fn the_indentation_stays_outside_the_inverse_span() {
    let theme = Theme::colored();
    let diff = "@@ -5,1 +5,1 @@\n-    let x = 1;\n+    let x = 9;\n";
    let rows = render(&parse(diff), 80, &theme);
    let joined = rows.join("\n");
    // The indent, the unchanged text and the changed token in order: the span opens
    // exactly at the `1`, which is the property - not "an inverse appears somewhere".
    assert!(
        joined.contains("    let x = \x1b[7m1\x1b[27m;"),
        "the indent and unchanged words are emitted before the span opens on the change: {:?}",
        rows
    );
    assert!(
        !joined.contains("\x1b[7m    "),
        "the indent never starts the span: {:?}",
        rows
    );
}

// Verifies: gh #392 gap 2 - **a two-line replacement stays line-level.** The issue's
// explicit restriction (and pi's): only 1↔1 gets a word diff, because a block rewrite's
// word alignment is a different problem and a checkerboard of inverse spans is worse than
// two solid rows. This is the row that fails if the spotlight is applied to any adjacent
// `-`/`+` pair.
#[test]
fn a_two_line_replacement_stays_line_level() {
    let theme = Theme::colored();
    let diff = "@@ -1,4 +1,4 @@\n ctx\n-fn one() { 1 }\n-fn two() { 2 }\n+fn one() { 3 }\n+fn two() { 4 }\n ctx2\n";
    let rows = render(&parse(diff), 80, &theme);
    let joined = rows.join("\n");
    assert!(
        !joined.contains("\x1b[7m"),
        "a 2↔2 replacement paints no inverse span: {:?}",
        rows
    );
    // The line-level roles are still there, so the row did not simply go blank.
    assert!(
        joined.contains("fn one() { 1 }") && joined.contains("fn one() { 3 }"),
        "and both sides still render: {:?}",
        rows
    );
}

// Verifies: gh #392 gap 2's SGR rule - **no all-channel reset in a diff row.**
//
// `\x1b[0m` here would clear the card's state band (the background tint the tool card
// paints), so a diff row that reset everything would punch a hole in the capsule. The
// roles close their own channel (`39`) and the inverse closes with `27`; this row is what
// keeps that true for the new gutter + body composition.
#[test]
fn a_diff_row_never_emits_an_all_channel_reset() {
    let theme = Theme::colored();
    let rows = render(&parse(ONE_WORD), 80, &theme);
    for row in &rows {
        assert!(
            !row.contains("\x1b[0m"),
            "a diff row reset every channel: {row:?}"
        );
    }
    // And the two channels really are closed separately, so the absence above is not
    // because nothing was painted.
    let joined = rows.join("\n");
    assert!(
        joined.contains("\x1b[39m"),
        "the foreground closes on 39: {joined:?}"
    );
    if joined.contains("\x1b[7m") {
        assert!(
            joined.contains("\x1b[27m"),
            "the inverse closes on 27: {joined:?}"
        );
    }
}

// Verifies: gh #392 gap 5 - **the body never exceeds the card.** The gutter is a fixed
// slice of the width (so no row's code shifts by a digit), and what is left is what the
// row may use. A long line at 40 columns must be clipped inside that budget, which is the
// half of the issue's "the diff never exceeds `width` at any size" that slice A owns.
#[test]
fn the_body_never_exceeds_the_card_width() {
    let theme = Theme::plain();
    let diff = "@@ -1,1 +1,1 @@\n-fn old() { this_line_is_deliberately_long_enough_to_need_clipping_at_forty_columns() }\n+fn new() { this_line_is_deliberately_long_enough_to_need_clipping_at_forty_columns() }\n";
    let inner = 40;
    let rows = render(&parse(diff), inner, &theme);
    for row in &rows {
        let width = lca_tui::engine::text::visible_width(row);
        assert!(
            width <= inner,
            "a row is {width} wide against a budget of {inner}: {row:?}"
        );
    }
    assert!(
        rows.iter().any(|row| row.contains('…')),
        "and the long row says it was clipped: {rows:?}"
    );
}

// Verifies: gh #392 gap 7 (already met by construction, verified rather than rebuilt) - a
// **truncated** patch still renders its rows. A streaming hunk is an incomplete diff; the
// parser must take what it has and never blank the card, because a card that disappears
// mid-stream reads as a crash.
#[test]
fn a_truncated_patch_still_renders_what_it_has() {
    let theme = Theme::plain();
    // No closing context, no trailing newline, and the last row cut mid-token.
    let partial = "--- a\n+++ b\n@@ -1,3 +1,3 @@\n fn a() {}\n-fn b() { ret";
    let rows = parse(partial);
    assert_eq!(rows.len(), 2, "both available rows are parsed: {rows:?}");
    let rendered = strip(&render(&rows, 80, &theme));
    assert_eq!(rendered.len(), 2, "and both render: {rendered:?}");
}
