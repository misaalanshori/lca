//! gh #384 bug 1: the table layout must measure what it paints.
//!
//! The split out of `tests.rs` is the file ceiling's (gate 11), not a
//! behavioral boundary: these rows all assert on `render_table`'s column
//! widths, which are what a starved neighbour looks like.

use super::*;

fn plain() -> MarkdownTheme {
    MarkdownTheme::default()
}

fn strip(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| crate::engine::text::strip_terminal_sequences(l))
        .collect()
}

/// The visible width of every column, read back off the top border.
///
/// This is the assertion the starved-column rows need: a rendered column
/// width is what a crushed neighbour looks like, and "no panic" is not a
/// measurement. The border between two `┬` runs is `w + 2` glyphs wide
/// (`─` repeated `width + 2`, `tables.rs`'s `border` closure).
fn column_widths(lines: &[String]) -> Vec<usize> {
    let stripped = strip(lines);
    let top = stripped.first().expect("a top border");
    assert!(top.starts_with('┌'), "the top border: {top:?}");
    top.trim_start_matches('┌')
        .trim_end_matches('┐')
        .split('┬')
        .map(|segment| {
            assert!(
                segment.chars().all(|c| c == '─'),
                "a border segment is all `─`: {segment:?}"
            );
            segment.chars().count().saturating_sub(2)
        })
        .collect()
}

fn options_with(link_mode: LinkMode) -> MarkdownOptions {
    MarkdownOptions {
        link_mode,
        ..Default::default()
    }
}

// The bug, stated as one row: a linked cell must not take the budget its
// neighbours need. The link renders as 4 columns; the raw source is 51.
// The old measurement gave `count` and `status` the 1-column minimum, so
// the words wrapped one letter per row.
#[test]
fn a_long_linked_cell_does_not_starve_the_short_columns() {
    let md = "| ref | count | status |\n| --- | --- | --- |\n\
              | [doc](https://example.com/a/very/long/path/that/goes/on) | 12 | ok |";
    let out = render_markdown(md, 40, &plain(), &options_with(LinkMode::Hyperlink));
    let widths = column_widths(&out);
    assert_eq!(
        widths,
        vec![3, 5, 6],
        "every column keeps its natural width: {widths:?}\n{out:?}"
    );
    // And the paint agrees with the measurement: the data row is one line
    // per cell, not a vertical smear.
    let stripped = strip(&out);
    assert!(
        stripped
            .iter()
            .any(|l| l.contains("12") && l.contains("ok")),
        "the data row paints on one line: {stripped:?}"
    );
}

// Both hyperlink modes, because they measure differently and a naive
// `len()` would pass in one and fail in the other: OSC 8 hides the URL
// (4 columns), the inline fallback prints `text (url)` (56).
#[test]
fn an_osc8_link_cell_measures_its_label() {
    let md = "| ref | n |\n| --- | --- |\n| [doc](https://example.com/x) | 1 |";
    let out = render_markdown(md, 40, &plain(), &options_with(LinkMode::Hyperlink));
    assert_eq!(
        column_widths(&out),
        vec![3, 1],
        "the label is the width: {out:?}"
    );
    assert!(
        out.iter()
            .any(|l| l.contains("\x1b]8;;https://example.com/x")),
        "the OSC 8 bytes are still painted: {out:?}"
    );
}

#[test]
fn an_inline_link_cell_measures_its_printed_fallback() {
    let md = "| ref | n |\n| --- | --- |\n| [doc](https://example.com/x) | 1 |";
    let out = render_markdown(md, 40, &plain(), &options_with(LinkMode::Inline));
    // `doc (https://example.com/x)` is 27 columns, and it is what prints.
    assert_eq!(
        column_widths(&out),
        vec![27, 1],
        "the printed fallback is the width: {out:?}"
    );
    assert!(
        strip(&out)
            .iter()
            .any(|l| l.contains("doc (https://example.com/x)")),
        "the fallback prints: {out:?}"
    );
}

// `**bold**` is 8 source columns and 4 painted ones.
#[test]
fn bold_measures_its_visible_text_not_its_asterisks() {
    let md = "| h |\n| --- |\n| **bold** |";
    let out = render_markdown(md, 40, &plain(), &Default::default());
    assert_eq!(column_widths(&out), vec![4], "{out:?}");
    assert!(
        !strip(&out).iter().any(|l| l.contains('*')),
        "no asterisk reaches the screen: {out:?}"
    );
}

// `$\alpha+\beta$` is 14 source columns and renders as `α+β` (3) - the
// latex widget typesets the symbols, not the source's spelling. Measuring
// the source is what gave the cell 14 columns for 3 painted glyphs.
#[test]
fn inline_math_measures_the_rendered_form_not_the_source() {
    let md = "| sym |\n| --- |\n| $\\alpha+\\beta$ |";
    let out = render_markdown(md, 40, &plain(), &Default::default());
    assert_eq!(column_widths(&out), vec![3], "{out:?}");
    let stripped = strip(&out);
    assert!(
        stripped.iter().any(|l| l.contains("α+β")),
        "the math renders: {stripped:?}"
    );
    assert!(
        !stripped.iter().any(|l| l.contains("alpha")),
        "the source does not: {stripped:?}"
    );
}

// The existing width rules apply to the rendered form too: a wide glyph is
// 2 columns and a combining mark adds none.
#[test]
fn a_cell_with_a_wide_glyph_or_combining_mark_measures_correctly() {
    let md = "| h |\n| --- |\n| 日本 |";
    let out = render_markdown(md, 40, &plain(), &Default::default());
    assert_eq!(column_widths(&out), vec![4], "two wide glyphs: {out:?}");

    // `e` + U+0301 is one grapheme, one column.
    let md = "| h |\n| --- |\n| e\u{301}e\u{301} |";
    let out = render_markdown(md, 40, &plain(), &Default::default());
    assert_eq!(column_widths(&out), vec![2], "two combining pairs: {out:?}");
}

// The fix is scoped: a table of plain text renders byte-identically.
#[test]
fn a_plain_table_renders_exactly_as_before() {
    let md = "| name | value |\n| --- | --- |\n| a | 1 |\n| longer | 22 |";
    let out = strip(&render_markdown(md, 40, &plain(), &Default::default()));
    assert_eq!(
        out,
        vec![
            "┌────────┬───────┐",
            "│ name   │ value │",
            "├────────┼───────┤",
            "│ a      │ 1     │",
            "├────────┼───────┤",
            "│ longer │ 22    │",
            "└────────┴───────┘",
        ],
        "the plain table is unchanged"
    );
}

// Behavior outside tables does not move (brief §5): the paragraph and the
// code block keep their current output.
#[test]
fn a_paragraph_still_renders_inline_markup() {
    let out = strip(&render_markdown(
        "para **bold** and [l](https://example.com/x) end",
        60,
        &plain(),
        &options_with(LinkMode::Inline),
    ));
    assert_eq!(
        out,
        vec!["para bold and l (https://example.com/x) end"],
        "the paragraph is unchanged: {out:?}"
    );
}

#[test]
fn a_code_block_is_unchanged() {
    let out = strip(&render_markdown(
        "```rust\nfn main() {}\n```",
        30,
        &plain(),
        &Default::default(),
    ));
    assert_eq!(
        out,
        vec!["╭─ rust ───────╮", "│ fn main() {} │", "╰──────────────╯"],
        "the code block is unchanged: {out:?}"
    );
}

// The narrow fallback still prints the raw source (pi's `3n + 1` rule).
#[test]
fn the_narrow_terminal_fallback_is_unchanged() {
    let md = "| a | b |\n| - | - |\n| [d](https://example.com/x) | 2 |";
    let out = strip(&render_markdown(md, 6, &plain(), &Default::default()));
    assert!(out[0].starts_with('|'), "the raw source line: {out:?}");
    assert!(
        out.iter().any(|l| l.contains("| - |")),
        "the separator is the source's: {out:?}"
    );
}

// A cell's rendered form is computed once and reused for the paint: the
// paint cannot be measuring a different string than the layout did, which
// is what would put the columns back to wrong the moment the renderer
// changes. Asserted through the observable contract: every painted row is
// exactly the sum of its column widths plus the border, at the width the
// solver chose.
#[test]
fn every_painted_row_matches_the_solved_widths() {
    let md = "| a | b | c |\n| --- | --- | --- |\n\
              | **x** | [link](https://example.com/very/long) | 9 |\n\
              | plain | two | three |";
    for mode in [LinkMode::Hyperlink, LinkMode::Inline] {
        let out = render_markdown(md, 48, &plain(), &options_with(mode));
        let widths = column_widths(&out);
        let expected: usize = widths.iter().sum::<usize>() + widths.len() * 3 + 1;
        for line in strip(&out).iter().filter(|l| l.starts_with('│')) {
            assert_eq!(
                visible_width(line),
                expected,
                "every body row is {expected} wide: {line:?}\n{out:?}"
            );
        }
    }
}

// Wrapped cells keep pi's narrow-style reset between fragments even though
// the seed is now a rendered string.
#[test]
fn a_wrapped_cell_still_resets_narrow_styles_between_fragments() {
    let raw = render_markdown(
        "| h |\n| - |\n| aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa |",
        20,
        &plain(),
        &Default::default(),
    );
    assert!(
        raw.iter()
            .any(|l| l.contains("\x1b[22;23;24;25;27;28;29;39m")),
        "the fragment reset is present: {raw:?}"
    );
}

// A long *plain* cell still wraps rather than being truncated (pi's
// `wrapCellText`), so rendered measurement did not turn wrapping off.
#[test]
fn a_long_plain_cell_still_wraps() {
    let md = "| h |\n| --- |\n| aaaa bbbb cccc dddd eeee |";
    let out = strip(&render_markdown(md, 14, &plain(), &Default::default()));
    let body: Vec<&String> = out.iter().filter(|l| l.starts_with('│')).collect();
    assert!(body.len() > 1, "the cell wrapped across rows: {out:?}");
}
