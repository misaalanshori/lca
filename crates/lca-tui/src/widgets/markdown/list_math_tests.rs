//! Gh #385: an item's display math typesets instead of printing raw.
//!
//! Split from `tests.rs` by subject (gh #574): production `list_math.rs` sits
//! beside these rows, and `tests` keeps the rest of the markdown vocabulary. The
//! rows report as `widgets::markdown::list_math_tests::<row>`.

use super::tests::{plain, strip};
use super::*;

/// The measured value each row asserts, taken from `render_markdown` with the
/// plain theme (no SGR) so a failure shows the painted text, not escape codes.
fn rows(source: &str, width: usize) -> Vec<String> {
    strip(&render_markdown(
        source,
        width,
        &plain(),
        &MarkdownOptions::default(),
    ))
}

// Verifies: gh #385 gap 1 - a bulleted derivation typesets. Before this the item
// was one line through `render_inline`, so the fraction printed as `x = a/b`
// while the identical content inside `>` stacked (`a` over `─` over `b`).
#[test]
fn a_bulleted_fraction_typesets_under_its_bullet() {
    let out = rows("- $$x = \\frac{a}{b}$$", 40);
    assert_eq!(
        out[0], "-     a",
        "the bullet carries the fraction's top row: {out:?}"
    );
    assert_eq!(
        out[1], "  x = ─",
        "continuation aligns under the content: {out:?}"
    );
    assert_eq!(out[2], "      b", "{out:?}");
    assert!(
        !out.iter().any(|row| row.contains("\\frac")),
        "no raw source: {out:?}"
    );
}

// Verifies: gh #385 - **a task item's math typesets too**, which is a second
// deliberate divergence from pi (like the over-wide fallback).
//
// Measured on pi: `- [ ] $$\\frac{a}{b}$$` prints flat (`- [ ] a/b`) while a bare
// `- $$\\frac{a}{b}$$` typesets. That is marked's tokenizer showing through - the
// task marker stays inside the item's *text*, so the `$$` is mid-line and becomes
// an inline token - not a rendering decision anyone made. LCA strips the task
// marker into the bullet, so the item content starts at the block and typesets,
// which keeps a document consistent with itself. A reviewer who wants pi's
// asymmetry should change this row deliberately, not incidentally.
#[test]
fn a_task_marker_does_not_push_the_block_back_to_inline() {
    let out = rows("- [ ] $$\\frac{a}{b}$$", 40);
    assert_eq!(out[0], "- [ ] a", "{out:?}");
    assert_eq!(
        out[1], "      ─",
        "the block aligns past the task marker: {out:?}"
    );
    assert_eq!(out[2], "      b", "{out:?}");
}

// Verifies: gh #385 gap 2 for an ordered marker - the number prints and the rows
// align to *its* width, so `10. ` (four columns) indents deeper than `1. `.
#[test]
fn an_ordered_item_aligns_its_continuation_rows_to_the_marker_width() {
    let out = rows("1. $$\\frac{a}{b}$$", 40);
    assert_eq!(out[0], "1. a", "{out:?}");
    assert_eq!(out[1], "   ─", "{out:?}");
    assert_eq!(out[2], "   b", "{out:?}");

    let out = rows("10. $$\\frac{a}{b}$$", 40);
    assert_eq!(out[0], "10. a", "two digits shift the block: {out:?}");
    assert_eq!(out[1], "    ─", "{out:?}");
    assert_eq!(out[2], "    b", "{out:?}");
}

// Verifies: gh #385 gap 2 continued - **each item of a run typesets its own block
// and the renumbering keeps counting**. The measurement that makes this a real
// claim: a block consumes extra source lines, so a cursor that only advanced one
// line would re-render the block's body as list items of their own - the failure
// a single-item fixture cannot see.
#[test]
fn two_ordered_items_each_typeset_and_the_run_keeps_counting() {
    let out = rows("1. $$\\frac{a}{b}$$\n2. $$\\frac{c}{d}$$", 40);
    assert_eq!(
        out,
        vec!["1. a", "   ─", "   b", "2. c", "   ─", "   d"],
        "two blocks, each with its own marker: {out:?}"
    );
}

// Verifies: gh #385 - **an item followed by a different item keeps both**. A
// typeset block used to be the only way to consume continuation lines, so the
// check that matters is the boundary: the math item's extra lines are consumed,
// the next item is not.
#[test]
fn a_typeset_item_is_followed_by_its_sibling() {
    let out = rows("- $$\\frac{a}{b}$$\n- second", 40);
    assert_eq!(
        out,
        vec!["- a", "  ─", "  b", "- second"],
        "the sibling starts its own item: {out:?}"
    );
}

// Verifies: gh #385 gap 3 - a nested item's math typesets too.
//
// **Measured divergence, reported not fixed**: LCA flattens a nested marker to
// column 0 (the line-based scanner trims the line before it reads the marker,
// so the authored indent is lost), where pi indents a child list by four:
// `- outer` / `    - a` / `      ─` / `      b`. That flattening predates this
// cycle and belongs to the nesting machinery the brief keeps out of scope, so
// this row pins the half that changed - the math typesets - and the indent
// half is recorded here rather than silently accepted.
#[test]
fn a_nested_items_math_typesets_at_the_flattened_marker_column() {
    let out = rows("- outer\n  - $$\\frac{a}{b}$$", 40);
    assert_eq!(out[0], "- outer", "{out:?}");
    assert_eq!(out[1], "- a", "the flattened nested marker: {out:?}");
    assert_eq!(out[2], "  ─", "its continuation still aligns: {out:?}");
    assert_eq!(out[3], "  b", "{out:?}");

    let out = rows("- a\n  - b\n    - $$\\frac{a}{b}$$", 40);
    assert_eq!(out[0], "- a", "{out:?}");
    assert_eq!(out[1], "- b", "{out:?}");
    assert_eq!(out[2], "- a", "the deepest item typesets: {out:?}");
}

// Verifies: gh #385 gap 3, the regression the issue names - `> - $$…$$` keeps its
// quote gutter *and* typesets. The blockquote re-enters `render_blocks` at two
// columns narrower, and the list branch spends that width on the bullet.
#[test]
fn a_bullet_inside_a_blockquote_typesets_and_keeps_the_gutter() {
    let out = rows("> - $$\\frac{a}{b}$$", 40);
    assert_eq!(out[0], "│ - a", "the gutter and the bullet: {out:?}");
    assert_eq!(out[1], "│   ─", "{out:?}");
    assert_eq!(out[2], "│   b", "{out:?}");
    assert!(
        out.iter().all(|row| row.starts_with("│ ")),
        "no row escapes the quote: {out:?}"
    );
}

// Verifies: gh #385's no-flicker rule - **a half-streamed block stays raw and its
// `_`/`*` are not italicised**. A theme whose italic paints SGR is the only way to
// ask that question: the plain theme's italic is identity, so it would pass for
// the trivial reason.
#[test]
fn an_unclosed_display_block_in_a_list_stays_raw_and_is_not_italicised() {
    let theme = MarkdownTheme {
        italic: Arc::new(|s| format!("\x1b[3m{s}\x1b[0m")),
        ..Default::default()
    };
    let options = MarkdownOptions::default();
    let out = render_markdown("- $$x_1 *y_2", 40, &theme, &options);
    let plain_out = strip(&out);
    assert_eq!(
        plain_out[0], "- $$x_1 *y_2",
        "the half block is raw: {plain_out:?}"
    );
    assert!(
        out.iter().all(|row| !row.contains("\x1b[3m")),
        "the pending source's `*`/`_` never reach the italic style: {out:?}"
    );

    // Once it closes, the same item typesets - the sink subscript is a real
    // script, so `x_1` stops being source.
    let out = rows("- $$x_1$$", 40);
    assert_eq!(out[0], "- x₁", "closed math renders: {out:?}");
}

// Verifies: gh #385 gap 4 - malformed math fails soft to raw source, exactly as
// the top-level path does, and never to a broken layout or a panic.
#[test]
fn malformed_display_math_in_a_list_stays_raw_source() {
    let out = rows("- $$\\notacommand$$", 40);
    assert_eq!(out[0], "- $$\\notacommand$$", "{out:?}");
    assert_eq!(out.len(), 1, "one row, no half-drawn block: {out:?}");
}

// Verifies: gh #385 gap 4 continued - **an unclosed block cannot swallow a
// deeper item**. The measurement that produced this row: pi keeps the nested
// `- second` after a pending `- $$\frac{a}{` (`["- $$","  - second"]`), so a body
// scan that ran to the end of the list would turn one half-streamed formula into
// the loss of every line below it.
//
// The fixture is **indented** on purpose. A flush `- second` is stopped by the
// content-column check alone (measured: removing the sibling test leaves that
// case green), so only a nested item - whose indent satisfies the column test -
// can show whether the scan also refuses to eat a marker.
#[test]
fn an_unclosed_block_does_not_swallow_a_deeper_item() {
    let out = rows("- $$\\frac{a}{\n  - second", 40);
    assert_eq!(out[0], "- $$\\frac{a}{", "{out:?}");
    assert!(
        out.iter().any(|row| row == "- second"),
        "the deeper item survives the pending block: {out:?}"
    );
}

// Verifies: gh #385 - **a multi-line block whose closing `$$` is at column 0 still
// typesets**, which is how a model writes one from the top level and then bullets
// it. Measured on pi: `- $$\\n\\frac{a}{b}\\n$$` prints `- a` / `  ─` / `  b`.
// The body scan therefore keeps a shallower line only while the block it opened is
// still unclosed - a closed block has already consumed its own lines.
#[test]
fn an_unindented_multiline_block_still_typesets_in_an_item() {
    let out = rows("- $$\n\\frac{a}{b}\n$$", 40);
    assert_eq!(out, vec!["- a", "  ─", "  b"], "{out:?}");
}

// Verifies: gh #385 - a **pending** opener keeps the shallower paragraph below it
// as part of the item, which is pi's lazy continuation. Measured on pi:
// `- $$x_1 *y` then `following paragraph` prints `- $$x_1 *y` /
// `  following paragraph`. The body scan keeps a shallower line while the block is
// open, and stops at the next marker - so the loss is bounded by the item.
#[test]
fn a_pending_opener_keeps_the_lazy_paragraph_inside_the_item() {
    let out = rows("- $$x_1 *y\nfollowing paragraph", 40);
    assert_eq!(out[0], "- $$x_1 *y", "{out:?}");
    assert_eq!(out[1], "  following paragraph", "{out:?}");
}

// Verifies: gh #385 - **an unindented block after the item's text is still the
// item's**, which is the shape a model writes when it drops the indent:
// `- note` then `$$` at column 0 then `\frac{a}{b}` then `$$`. Measured on pi:
// `- note` / `  a` / `  ─` / `  b`, all prefixed. The scan keeps a shallower line
// when it *opens* a display block, and keeps taking lines until that block closes.
#[test]
fn an_unindented_block_after_the_items_text_is_a_lazy_continuation() {
    let out = rows("- note\n$$\n\\frac{a}{b}\n$$", 40);
    assert_eq!(
        out,
        vec!["- note", "  a", "  ─", "  b"],
        "the block is the item's and prefixed: {out:?}"
    );
}

// Verifies: gh #385 - **the lazy block stops at its closer.** After an unindented
// block closes, the lines below it are the item's ordinary continuation (pi prints
// `  after text` prefixed), and the next `- item` is a new item. This is what the
// lazy-start sentinel buys: without it the scan would keep taking column-0 lines
// for the rest of the document (mutant-verified as *green* on the rows above, which
// is why this row exists).
#[test]
fn a_lazy_block_stops_at_its_closer_and_the_rest_is_the_items_continuation() {
    let out = rows("- note\n$$\n\\frac{a}{b}\n$$\nafter text\n- next item", 40);
    assert_eq!(
        out,
        vec!["- note", "  a", "  ─", "  b", "  after text", "- next item"],
        "{out:?}"
    );
}

// Verifies: gh #385's width honesty - a 2D block wider than the item's content
// width falls back to **raw wrapped source**, and no row exceeds the width.
// Falling back is not enough on its own: a fallback that still overflows is
// the same defect wearing a different name.
#[test]
fn an_over_wide_matrix_falls_back_to_raw_source_within_the_item_width() {
    let matrix = "\\begin{matrix} 1111 & 2222 & 3333 & 4444 & 5555 & 6666 \\\\ 7777 & 8888 & 9999 & 0000 & aaaa & bbbb \\end{matrix}";
    let source = format!("- $${matrix}$$");
    for width in [20usize, 24, 30] {
        let out = rows(&source, width);
        assert!(
            out.iter().any(|row| row.contains("\\begin{matrix}")),
            "the raw source prints at {width}: {out:?}"
        );
        let widest = out
            .iter()
            .map(|row| crate::engine::text::visible_width(row))
            .max()
            .unwrap_or(0);
        assert!(
            widest <= width,
            "no row exceeds {width} columns (widest {widest}): {out:?}"
        );
    }

    // The same block at a width that fits keeps its typeset shape, so the
    // fallback is a width rule and not a refusal to render matrices.
    let out = rows(&source, 80);
    assert!(
        !out.iter().any(|row| row.contains("\\begin")),
        "wide enough, it typesets: {out:?}"
    );
    assert!(out[0].starts_with("- 1111 │ 2222"), "{out:?}");
}

// Verifies: gh #385 - the `\\[…\\]` spelling takes the same branch as `$$…$$`,
// because `block_math` accepts both (pi's `tokenizeBlockLatex`). A router that
// checked for `$$` by name would leave the bracket form raw.
#[test]
fn the_bracket_spelling_of_display_math_typesets_in_a_list_too() {
    let out = rows("- \\[\\frac{a}{b}\\]", 40);
    assert_eq!(out[0], "- a", "{out:?}");
    assert_eq!(out[1], "  ─", "{out:?}");
    assert_eq!(out[2], "  b", "{out:?}");
}

// Verifies: gh #385 gap 5 - an **inline** `$…$` inside an item keeps today's
// single-line path and its guards. Routing it through the block path would stack
// an inline fraction, which is exactly what pi does not do.
#[test]
fn an_inline_dollar_run_in_a_list_item_keeps_the_inline_path() {
    let out = rows("- $x^2$", 40);
    assert_eq!(out[0], "- x²", "{out:?}");
    assert_eq!(out.len(), 1, "inline math stays on one row: {out:?}");

    // The dollar-prose guard still applies inside an item.
    let out = rows("- it costs $5 and that is that", 40);
    assert_eq!(out[0], "- it costs $5 and that is that", "{out:?}");
}

// Verifies: gh #385 routing boundary - **an `$$` that starts mid-line is an inline
// token**, which is pi's own classification (`markdown.ts:655` handles `latex`
// inline and only a block-start reaches `latexBlock`). Measured on pi:
// `- note: $$x = \frac{a}{b}$$` prints `- note: x = a/b`.
#[test]
fn a_midline_display_run_keeps_the_inline_path() {
    let out = rows("- steady state: $$x = \\frac{a}{b}$$", 40);
    assert_eq!(
        out[0], "- steady state: x = a/b",
        "the inline flattening pi also does: {out:?}"
    );
    assert_eq!(out.len(), 1, "{out:?}");
}

// Verifies: gh #385 - a block that **follows an item's first line** typesets too,
// because an item's body is a block sequence: `- note:` then an indented `$$…$$`.
// The text line stays a paragraph and the block starts at the content column.
#[test]
fn a_block_after_the_items_first_line_typesets_at_the_content_column() {
    let out = rows("- note:\n  $$\\frac{a}{b}$$", 40);
    assert_eq!(out[0], "- note:", "{out:?}");
    assert_eq!(out[1], "  a", "the block aligns under the text: {out:?}");
    assert_eq!(out[2], "  ─", "{out:?}");
    assert_eq!(out[3], "  b", "{out:?}");
}

// Verifies: gh #385 - the **plain-text path stays plain**. `MarkdownTheme::default`
// paints no SGR, so a typeset block inside a list must not gain any: the rows are
// spaced with characters, not with escape codes.
#[test]
fn a_typeset_list_block_paints_no_sgr_with_the_plain_theme() {
    for source in [
        "- $$\\frac{a}{b}$$",
        "- $$\\begin{cases} a & x>0 \\\\ b & x<0 \\end{cases}$$",
    ] {
        let out = render_markdown(source, 40, &plain(), &MarkdownOptions::default());
        assert!(
            out.iter().all(|row| !row.contains('\x1b')),
            "the plain theme paints nothing: {out:?}"
        );
    }
}
