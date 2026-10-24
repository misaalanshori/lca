//! Gh #384 bug 1: the markdown table layout measured raw Markdown, so a
//! single `[link](https://…)` cell counted ~42 columns against the 4 it
//! painted and starved every other column to its 1-column minimum - the
//! words in the neighbouring cells wrapped one letter per row.
//!
//! The fix is one principle: **measure what will be painted, once**. The
//! cells' inline tokens are rendered a single time and the *rendered* form
//! is both measured and painted, so the layout and the painter cannot take
//! different inputs. `render_table` also takes the caller's
//! `MarkdownOptions` now; it used to render cells with
//! `MarkdownOptions::default()`, which measured and painted a different
//! link mode than the transcript asked for.
//!
//! Verifies: FR-UI-7 (defect gh #384, bug 1). The unit rows live in
//! `crates/lca-tui/src/widgets/markdown/table_tests.rs`; this file is the
//! NFR-24 guard at the transcript's own entry point.

use lca_tui::engine::text::{strip_terminal_sequences, visible_width};
use lca_tui::widgets::markdown::{LinkMode, MarkdownOptions, MarkdownTheme, render_markdown};

/// A message whose first column is a long link and whose other columns are
/// short words - the shape from the report.
const STARVED: &str = "\
| reference | count | status |
| --- | --- | --- |
| [the spec](https://example.com/docs/a/very/long/path/that/goes/on/and/on) | 12 | ok |
| [a second one](https://example.com/another/long/path/entirely) | 7 | ok |";

fn options(link_mode: LinkMode) -> MarkdownOptions {
    MarkdownOptions {
        link_mode,
        ..Default::default()
    }
}

/// The visible width of each column, read back off the top border: a border
/// segment is `width + 2` glyphs (`─` repeated `w + 2`, then the junction).
fn column_widths(lines: &[String]) -> Vec<usize> {
    let stripped: Vec<String> = lines.iter().map(|l| strip_terminal_sequences(l)).collect();
    let top = stripped.first().expect("a top border");
    assert!(top.starts_with('┌'), "the top border: {top:?}");
    top.trim_start_matches('┌')
        .trim_end_matches('┐')
        .split('┬')
        .map(|segment| segment.chars().count().saturating_sub(2))
        .collect()
}

#[test]
fn a_long_linked_cell_no_longer_starves_its_neighbours() {
    let out = render_markdown(
        STARVED,
        60,
        &MarkdownTheme::default(),
        &options(LinkMode::Hyperlink),
    );
    let widths = column_widths(&out);

    // `the spec` and `a second one` are the widest labels (12); the count
    // column needs 5 (`count`); status needs 6.
    assert_eq!(
        widths,
        vec![12, 5, 6],
        "the short columns keep their natural width instead of collapsing to 1: {widths:?}\n{out:?}"
    );

    // The observable half of the bug: `count` and `status` painted one
    // letter per row. Each painted row is one line per logical row now.
    let stripped: Vec<String> = out.iter().map(|l| strip_terminal_sequences(l)).collect();
    assert!(
        stripped
            .iter()
            .any(|line| line.contains("count") && line.contains("status")),
        "the header paints on one line: {stripped:?}"
    );
    assert!(
        stripped
            .iter()
            .any(|line| line.contains("12") && line.contains("ok")),
        "the data row paints on one line: {stripped:?}"
    );
}

// Both hyperlink modes, because they measure differently: OSC 8 paints the
// label, the inline fallback paints `text (url)` and is far wider. A
// measurement on raw Markdown would pass or fail arbitrarily between them.
#[test]
fn both_hyperlink_modes_measure_what_they_paint() {
    let osc8 = render_markdown(
        STARVED,
        60,
        &MarkdownTheme::default(),
        &options(LinkMode::Hyperlink),
    );
    let inline = render_markdown(
        STARVED,
        60,
        &MarkdownTheme::default(),
        &options(LinkMode::Inline),
    );

    let osc8_widths = column_widths(&osc8);
    let inline_widths = column_widths(&inline);
    assert_eq!(osc8_widths[0], 12, "the OSC 8 label: {osc8:?}");
    // `the spec (https://example.com/docs/a/very/long/path/that/goes/on/and/on)`
    // is what prints in the fallback, so that is what the column measures.
    assert!(
        inline_widths[0] > osc8_widths[0],
        "the inline fallback prints the url, so its column is wider: \
         {inline_widths:?} vs {osc8_widths:?}"
    );
    let inline_text: Vec<String> = inline.iter().map(|l| strip_terminal_sequences(l)).collect();
    assert!(
        inline_text
            .iter()
            .any(|l| l.contains("(https://example.com/docs/")),
        "the fallback prints the url: {inline_text:?}"
    );

    // Every painted row in both modes is the width the solver chose.
    for out in [&osc8, &inline] {
        let widths = column_widths(out);
        let expected: usize = widths.iter().sum::<usize>() + widths.len() * 3 + 1;
        for line in out
            .iter()
            .map(|l| strip_terminal_sequences(l))
            .filter(|l| l.starts_with('│'))
        {
            assert_eq!(
                visible_width(&line),
                expected,
                "a body row is {expected} columns: {line:?}"
            );
        }
    }
}

// The measured string is the painted string: whatever the inline renderer
// does to a cell, the column is exactly as wide as the result paints.
// Asserted over the four shapes the report names, in both modes.
#[test]
fn the_column_is_exactly_as_wide_as_the_cell_paints() {
    let cells = [
        ("bold", "**bold**"),
        ("math", "$\\alpha+\\beta$"),
        ("code", "`some_code()`"),
        ("link", "[label](https://example.com/a/long/url)"),
        ("strike", "~~gone~~"),
    ];
    for mode in [LinkMode::Hyperlink, LinkMode::Inline] {
        for (name, cell) in cells {
            let md = format!("| h |\n| --- |\n| {cell} |");
            let out = render_markdown(&md, 60, &MarkdownTheme::default(), &options(mode));
            let widths = column_widths(&out);
            // What the inline renderer produces for that cell, measured the
            // same way the table must have measured it.
            let painted = lca_tui::widgets::markdown::render_inline(
                cell,
                &MarkdownTheme::default(),
                &options(mode),
            );
            assert_eq!(
                widths[0],
                visible_width(&painted),
                "{name} in {mode:?}: the column is the painted width, not the source's\n{out:?}"
            );
        }
    }
}

// A plain-text table is untouched by the fix: no source markup means the
// rendered form is the source.
#[test]
fn a_plain_table_is_byte_identical_to_before() {
    let out: Vec<String> = render_markdown(
        "| name | value |\n| --- | --- |\n| a | 1 |\n| longer | 22 |",
        40,
        &MarkdownTheme::default(),
        &options(LinkMode::Hyperlink),
    )
    .iter()
    .map(|l| strip_terminal_sequences(l))
    .collect();
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
        ]
    );
}

// The upstream defect the fix exposed, pinned end to end: a rendered link
// closes OSC 8 with `ST` (`ESC \`), and `sanitize_incomplete_escapes` used
// to treat a trailing `ST` as an *incomplete* sequence and chop it. The
// dangling `ESC ] 8 ; ;` then made the rest of the table row OSC payload,
// so the neighbouring cells disappeared from the pane entirely - worse than
// the starvation, and invisible to a stripped-text assertion.
//
// This row asserts on what the terminal is actually left with: the row's
// other cells survive, and no OSC 8 is left open at the end of any line.
#[test]
fn a_wrapped_link_cell_leaves_no_open_hyperlink_and_keeps_its_neighbours() {
    let out = render_markdown(
        STARVED,
        44,
        &MarkdownTheme::default(),
        &options(LinkMode::Hyperlink),
    );
    for line in &out {
        // An OSC sequence that never terminates consumes the rest of the
        // stream as its payload. Before the fix a wrapped link cell left a
        // bare `ESC ] 8 ; ;` behind, so the remaining cells became link text
        // and never painted. Every OSC introduced on a line ends in `ST`
        // (`ESC \```) or `BEL`.
        for (at, _) in line.match_indices("\x1b]") {
            let rest = &line[at..];
            assert!(
                rest.contains("\x1b\\") || rest.contains('\u{7}'),
                "every OSC on a line is terminated: {line:?}\n{out:?}"
            );
        }
    }
    let stripped: Vec<String> = out.iter().map(|l| strip_terminal_sequences(l)).collect();
    assert!(
        stripped
            .iter()
            .any(|l| l.contains("12") && l.contains("ok")),
        "the cells after the link are still on the row: {stripped:?}"
    );
    assert!(
        stripped.iter().filter(|l| l.starts_with('│')).count() > 2,
        "the table still paints its rows: {stripped:?}"
    );
}
