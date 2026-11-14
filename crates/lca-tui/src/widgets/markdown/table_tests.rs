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

// ---------------------------------------------------------------------------
// Gh #384 bug 2 - the allocator, not the measurement.
//
// Both rows below render a table whose columns cannot all reach their
// natural width, and assert the **widths** rather than "no panic": a starved
// neighbour is a number. The two numbers in each message are today's answer
// and the proportional one, so a reader can see the divergence without
// re-running anything.
// ---------------------------------------------------------------------------

// Verifies: gh #384 bug 2 - **grow by potential, not round-robin.**
//
// The fixture: a 2-char id column (min-word 2, natural 3) beside an 8-word
// prose column (min-word 7, natural 45), in a 20-column terminal. The budget
// for cells is `20 - (2*3+1) = 13`.
//
// Round-robin handed the id `+1` per pass until it hit its natural 3, so the
// prose column got 10. Proportional gives the id its minimum 2 (its grow
// potential is `3-2 = 1` against the prose's `45-7 = 38`) and the prose 11 -
// "the narrow column stops growing while prose keeps gaining".
#[test]
fn a_narrow_identifier_column_stops_growing_while_prose_keeps_gaining() {
    let md = "| id | notes |\n| --- | --- |\n\
              | a b | alpha beta gamma delta epsilon zeta eta theta |";
    let out = render_markdown(md, 20, &plain(), &Default::default());
    let widths = column_widths(&out);
    assert_eq!(
        widths,
        vec![2, 11],
        "the id column keeps its min-word 2 and the prose takes the rest. \
         Round-robin answered [3, 10] - the id grew to its natural 3 while prose \
         was still one short. widths: {widths:?}\n{out:?}"
    );
}

// Verifies: gh #384 bug 2 - **shrink by weight, not equally.**
//
// A 3-char id beside a three-word prose column whose longest word is 30 (the
// `getLongestWordWidth` cap). The minima sum to 33 against a 13-cell budget,
// so the shrink rule runs. Round-robin took one column from every column in
// turn, so the id lost its `b` and the prose lost 18; weighted shrink starts
// both at 1 and hands the budget out by `min_word - 1`, so the id keeps 2 and
// the prose 11.
#[test]
fn min_overflow_shrinks_a_wide_column_harder_than_a_narrow_one() {
    let md = "| id | note |\n| --- | --- |\n\
              | abc | antidisestablishmentarianism floccinaucinihilipilification \
              pneumonoultramicroscopicsilicovolcanoconiosis archaeological |";
    let out = render_markdown(md, 20, &plain(), &Default::default());
    let widths = column_widths(&out);
    assert_eq!(
        widths,
        vec![2, 11],
        "the id keeps 2 of its 3 columns and the prose column takes the loss. \
         Round-robin answered [1, 12] - the id lost exactly as much as the prose \
         column beside it. widths: {widths:?}\n{out:?}"
    );
}

// Verifies: gh #384 bug 2 - **the sweep is deterministic and spends the
// budget exactly.**
//
// Two halves. First, `sum(widths) == available` **exactly**, not `<=`: a table
// that leaves a column of the budget unspent has drawn a narrower frame than
// the terminal offers, which is a layout defect a `<=` assertion would pass.
// Second, the same table solves to the same widths twice, and allocating
// against the answer it produced changes nothing (`sum(natural) <= available`
// then holds, so rule 1 returns the widths unchanged) - that is the
// idempotence the largest-remainder sweep buys, and a sweep whose leftovers
// depended on iteration order would fail it.
#[test]
fn the_remainder_sweep_is_deterministic_and_sums_to_the_budget() {
    // (min_word, natural, available) triples whose remainders do NOT divide
    // evenly, so the leftover columns are what decides the layout.
    let cases: [(Vec<usize>, Vec<usize>, usize); 4] = [
        (vec![2, 7], vec![3, 45], 13),
        (vec![3, 30], vec![3, 119], 13),
        (vec![1, 1, 1], vec![20, 20, 20], 10),
        (vec![4, 9, 2], vec![8, 61, 6], 40),
    ];
    for (min_word, natural, available) in cases {
        let first = super::tables::solve_column_widths(&min_word, &natural, available);
        assert_eq!(
            first.iter().sum::<usize>(),
            available,
            "the budget is spent exactly: {first:?} for {available} \
             (min_word {min_word:?}, natural {natural:?})"
        );
        let second = super::tables::solve_column_widths(&min_word, &natural, available);
        assert_eq!(first, second, "the same table solves identically twice");
        // Idempotence: the widths become their own natural width, which fits.
        let idempotent = super::tables::solve_column_widths(&first, &first, available);
        assert_eq!(
            idempotent, first,
            "allocating against the allocation it produced changes nothing"
        );
        // And the sweep really is **largest-remainder**: each column within one
        // of its exact share of the budget. This is the clause that separates a
        // proportional allocator from an equal-share one - the old round-robin
        // satisfied sum/idempotence/bounds and still starved the wide column,
        // which is why those bounds alone are not the acceptance criterion.
        let fits_naturally = natural.iter().sum::<usize>() <= available;
        let (base, weights, budget) = if fits_naturally {
            (natural.clone(), vec![0usize; natural.len()], 0usize)
        } else if available >= min_word.iter().sum::<usize>() {
            (
                min_word.clone(),
                (0..min_word.len())
                    .map(|c| natural[c] - min_word[c])
                    .collect::<Vec<usize>>(),
                available - min_word.iter().sum::<usize>(),
            )
        } else {
            (
                vec![1usize; min_word.len()],
                min_word.iter().map(|w| w - 1).collect::<Vec<usize>>(),
                available - min_word.len(),
            )
        };
        let total: usize = weights.iter().sum();
        if total > 0 && budget > 0 {
            for c in 0..first.len() {
                let exact = base[c] + budget * weights[c] / total;
                assert!(
                    first[c].abs_diff(exact) <= 1,
                    "column {c} is {} away from its proportional share {exact} \
                     (widths {first:?}, base {base:?}, weights {weights:?}, \
                     budget {budget}) - equal shares are not proportional ones",
                    first[c].abs_diff(exact)
                );
            }
        }
    }
}

// Verifies: gh #384 bug 2 - **the tie-break is index-tied, and it is the
// tie-break that makes the result deterministic.**
//
// Two columns with **identical weights** leave the largest-remainder sweep a
// single leftover column to place, and both candidates have the same remainder:
// the fractional parts are equal, so ascending index must take it. Feeding the
// same triple twice proves nothing here - the sweep is a pure function of its
// inputs either way, and a descending tie-break is equally reproducible. What
// this row pins is the **orientation**, because pi's tie-break is ascending
// index and a descending one would disagree with pi on every such table.
//
// (`([1,1], [2,2], 3)` is the smallest instance: both remainders are 1, so the
// leftover goes to column 0.)
#[test]
fn a_largest_remainder_tie_breaks_by_ascending_column_index() {
    assert_eq!(
        super::tables::solve_column_widths(&[1, 1], &[2, 2], 3),
        vec![2, 1],
        "equal remainders go to the earlier column"
    );
    assert_eq!(
        super::tables::solve_column_widths(&[1, 1], &[3, 3], 3),
        vec![2, 1],
        "and the same triple answers identically on a second call"
    );
    // Same weights through the *shrink* rule, so both sweeps are pinned, not
    // only the grow one.
    assert_eq!(
        super::tables::solve_column_widths(&[2, 2], &[2, 2], 3),
        vec![2, 1],
        "the shrink sweep breaks the tie the same way"
    );
}

// Verifies: gh #384 bug 2 - the allocator's invariants over **arbitrary**
// `(min_word, natural, available)` triples, not the four samples above.
//
// Four properties, and the ones a hand-tuned `if` would violate:
//
// * every column is at least 1 wide (a 0-column is a missing column);
// * no column exceeds its natural width, and none is below its min-word width
//   **when the minima fit** - the case where the minima do not fit can only
//   promise >= 1, and pretending otherwise is what the old equal-shrink did;
// * the sum never exceeds the budget when a cell fits at all;
// * idempotence, for every triple.
#[cfg(test)]
mod allocator_properties {
    use proptest::prelude::*;

    fn triple(cols: usize) -> impl Strategy<Value = (Vec<usize>, Vec<usize>, usize)> {
        (
            proptest::collection::vec(1usize..31, cols..=cols),
            proptest::collection::vec(1usize..81, cols..=cols),
            1usize..120,
        )
            .prop_map(|(min_word, mut natural, available)| {
                // `natural >= min_word` is what a measurement produces; the
                // allocator is not asked to invent order out of nonsense.
                for (n, m) in natural.iter_mut().zip(min_word.iter()) {
                    *n = (*n).max(*m);
                }
                (min_word, natural, available)
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

        #[test]
        fn the_allocator_keeps_its_invariants(case in (1usize..4).prop_flat_map(triple)) {
            let (min_word, natural, available) = case;
            let widths = super::tables::solve_column_widths(&min_word, &natural, available);
            prop_assert_eq!(widths.len(), min_word.len(), "one width per column");
            prop_assert!(
                widths.iter().all(|w| *w >= 1),
                "no column is zero wide: {:?}",
                widths
            );
            prop_assert!(
                widths.iter().zip(natural.iter()).all(|(w, n)| *w <= *n),
                "no column exceeds its natural width: {:?} vs {:?}",
                widths,
                natural
            );
            // The three rules, and the invariant each one owes:
            if natural.iter().sum::<usize>() <= available {
                // Rule 1: everything fits, so the widths ARE the natural widths.
                // This is the case that must render byte-identically to the
                // pre-cycle tree, so it is asserted as an equality, not a bound.
                prop_assert_eq!(
                    widths.clone(),
                    natural.clone(),
                    "a table that fits takes its natural widths"
                );
            } else if available >= min_word.iter().sum::<usize>() {
                // Rule 3: grown from the minima by potential. Every column keeps
                // its min-word and the budget is spent **exactly** - a column of
                // room left unspent is a frame narrower than the terminal.
                prop_assert!(
                    widths.iter().zip(min_word.iter()).all(|(w, m)| *w >= *m),
                    "when the minima fit, every column is at least min-word: {:?} vs {:?}",
                    widths,
                    min_word
                );
                prop_assert_eq!(
                    widths.iter().sum::<usize>(),
                    available,
                    "and the budget is spent exactly"
                );
            } else {
                // Rule 2: the minima overflow, so only >= 1 is promised.
                prop_assert!(
                    widths.iter().sum::<usize>() <= available.max(min_word.len()),
                    "an over-full minima set still fits the budget (or degrades to \
                     one cell per column): {:?} for {:?}",
                    widths,
                    available
                );
            }
            // **The policy invariant, and the one the round-robin allocator
            // violated.** Several width vectors satisfy every bound above - the
            // old round-robin one did - so the invariants alone cannot tell
            // "proportional" from "equal shares". Largest-remainder distribution
            // is characterised by its fidelity: every column's width is within
            // **one** of its exact proportional share. A round-robin column can
            // be several columns off (measured: a 2-cell id got 4 where its share
            // was 1), which is the starvation this bug is about.
            {
                let (base, weights, budget) = if natural.iter().sum::<usize>() <= available {
                    // Nothing is distributed: rule 1 returns the naturals.
                    (natural.clone(), vec![0usize; natural.len()], 0usize)
                } else if available >= min_word.iter().sum::<usize>() {
                    let floor_total = min_word.iter().sum::<usize>();
                    (
                        min_word.clone(),
                        (0..min_word.len())
                            .map(|c| natural[c].saturating_sub(min_word[c]))
                            .collect::<Vec<usize>>(),
                        available - floor_total,
                    )
                } else {
                    (
                        vec![1usize; min_word.len()],
                        min_word.iter().map(|w| w.saturating_sub(1)).collect::<Vec<usize>>(),
                        available.saturating_sub(min_word.len()),
                    )
                };
                let total: usize = weights.iter().sum();
                if total > 0 && budget > 0 {
                    for c in 0..widths.len() {
                        let exact = base[c] + budget * weights[c] / total;
                        prop_assert!(
                            widths[c].abs_diff(exact) <= 1,
                            "column {c} is {} away from its proportional share {exact} \
                             (widths {:?}, base {:?}, weights {:?}, budget {budget})",
                    widths[c].abs_diff(exact),
                            widths,
                            base,
                            weights
                        );
                    }
                }
            }
            // Idempotence: the answer, fed back as its own target, does not move.
            let again = super::tables::solve_column_widths(&widths, &widths, available);
            prop_assert_eq!(again, widths, "allocating against itself changes nothing");
        }
    }
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
