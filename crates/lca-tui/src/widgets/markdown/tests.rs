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

#[test]
fn headings_and_paragraphs() {
    let out = strip(&render_markdown(
        "# Title\n\nsome text",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "Title");
    assert!(out.iter().any(|l| l.contains("some text")));
}

// Verifies: cycle 9 - a heading runs its *inline* tokens inside the
// heading style (markdown.md §4), so `**b**` and `c` render instead of
// showing their markup, and the heading color is re-armed after the
// code span's reset (§3's style-prefix trick) instead of the rest of
// the line falling back to the body color.
#[test]
fn headings_run_inline_tokens_in_the_heading_style() {
    let theme = MarkdownTheme {
        heading: Arc::new(|s| format!("\x1b[38;2;1;2;3m{s}\x1b[39m")),
        bold: Arc::new(|s| format!("\x1b[1m{s}\x1b[22m")),
        underline: Arc::new(|s| format!("\x1b[4m{s}\x1b[24m")),
        code: Arc::new(|s| format!("\x1b[38;2;9;9;9m{s}\x1b[39m")),
        ..Default::default()
    };
    let raw = render_markdown(
        "## A **b** and `c` here\n\n### Three",
        60,
        &theme,
        &MarkdownOptions::default(),
    );
    let out = strip(&raw);
    assert_eq!(out[0], "A b and c here", "inline markup rendered: {out:?}");
    assert!(
        raw[0].contains("\x1b[39m\x1b[38;2;1;2;3m"),
        "the heading color is re-armed after the code span: {:?}",
        raw[0]
    );
    assert!(
        raw[0].starts_with("\x1b[38;2;1;2;3m\x1b[1m"),
        "the heading carries its own color and the bold: {:?}",
        raw[0]
    );
    assert_eq!(out[2], "### Three", "h3+ keeps the literal prefix: {out:?}");
    // An h1 also underlines (pi: heading(bold(underline(…)))).
    let h1 = render_markdown("# One", 40, &theme, &MarkdownOptions::default());
    assert!(h1[0].contains("\x1b[4m"), "h1 underlines: {h1:?}");
    assert!(
        h1[0].contains("\x1b[38;2;1;2;3m"),
        "and keeps the heading color: {h1:?}"
    );
}

// Verifies: cycle 9 - a blockquote's *text* is quote+italic and its
// border is `mdQuoteBorder`, the two roles pi splits (markdown.md §4);
// the style survives a nested reset inside the quote.
#[test]
fn a_blockquote_paints_its_text_and_its_border_separately() {
    let theme = MarkdownTheme {
        quote: Arc::new(|s| format!("\x1b[38;2;5;5;5m{s}\x1b[39m")),
        quote_border: Arc::new(|s| format!("\x1b[38;2;6;6;6m{s}\x1b[39m")),
        italic: Arc::new(|s| format!("\x1b[3m{s}\x1b[23m")),
        code: Arc::new(|s| format!("\x1b[38;2;9;9;9m{s}\x1b[39m")),
        ..Default::default()
    };
    let raw = render_markdown(
        "> quoted with `code` inside",
        60,
        &theme,
        &MarkdownOptions::default(),
    );
    let line = raw
        .iter()
        .find(|l| l.contains('│'))
        .expect("the quote border row");
    assert!(
        line.starts_with("\x1b[38;2;6;6;6m│ "),
        "the border is mdQuoteBorder: {line:?}"
    );
    assert!(
        line.contains("\x1b[38;2;5;5;5m\x1b[3m"),
        "the text is quote+italic: {line:?}"
    );
    assert!(
        line.contains("\x1b[39m\x1b[38;2;5;5;5m\x1b[3m"),
        "re-armed after the code span: {line:?}"
    );
    assert_eq!(strip(&raw)[0], "│ quoted with code inside");
}

// pi: h1 = heading(bold(underline(text))), h2 = heading(bold(text)).
#[test]
fn the_level_one_heading_is_underlined() {
    let theme = MarkdownTheme {
        heading: Arc::new(|s| format!("[36m{s}[0m")),
        bold: Arc::new(|s| format!("[1m{s}[0m")),
        underline: Arc::new(|s| format!("[4m{s}[0m")),
        ..Default::default()
    };
    let h1 = render_markdown("# Title", 40, &theme, &MarkdownOptions::default());
    assert!(h1[0].contains("\x1b[4m"), "h1 underlines: {:?}", h1[0]);
    let h2 = render_markdown("## Title", 40, &theme, &MarkdownOptions::default());
    assert!(!h2[0].contains("\x1b[4m"), "h2 does not: {:?}", h2[0]);
}

#[test]
fn lists_get_bullets_and_continuation_indent() {
    let out = strip(&render_markdown(
        "- first item\n- second item that is quite long and wraps around the width",
        20,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out[0].starts_with("- first"), "{:?}", out[0]);
    assert!(out[1].starts_with("- second"), "{:?}", out[1]);
    assert!(out[2].starts_with("  ")); // continuation aligns under the text
}

#[test]
fn code_blocks_are_framed() {
    let out = strip(&render_markdown(
        "```rust\nlet x = 1;\n```",
        30,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out[0].starts_with('╭'));
    assert!(out.iter().any(|l| l.starts_with("│ let x = 1;")));
    assert!(out.last().unwrap().starts_with('╰'));
}

// Verifies: gh #32 - `markdown.codeblock_border = "horizontal"` draws
// top and bottom bars only: the code lines carry no side pipes, so a
// terminal selection pastes them clean, and the language keeps its
// title on the top bar (the exact shape the issue asked for).
#[test]
fn horizontal_bars_replace_the_frame_and_leave_the_lines_clean() {
    let options = MarkdownOptions {
        codeblock_border: CodeBlockBorder::Horizontal,
        ..Default::default()
    };
    let out = strip(&render_markdown(
        "```python\nprint(1)\nprint(2)\n```",
        30,
        &plain(),
        &options,
    ));
    assert_eq!(
        out,
        vec![
            "── python ──".to_string(),
            "print(1)".to_string(),
            "print(2)".to_string(),
            "────────────".to_string(),
        ],
        "bars top and bottom, code lines bare: {out:?}"
    );
    assert!(
        !out.iter().any(|line| line.contains('│')),
        "no side pipes anywhere"
    );
}

// Verifies: gh #32 - `none` is the bare block: no bars, no pipes, no
// padding - exactly what the fence wrote.
#[test]
fn none_renders_the_bare_lines() {
    let options = MarkdownOptions {
        codeblock_border: CodeBlockBorder::None,
        ..Default::default()
    };
    let out = strip(&render_markdown(
        "```rust\nfn main() {}\n```",
        30,
        &plain(),
        &options,
    ));
    assert_eq!(out, vec!["fn main() {}".to_string()], "{out:?}");
}

// Verifies: R3 - a fence that names a language goes through the
// theme's `highlight` hook (pi's `theme.highlightCode`); a fence with
// no language keeps `code_block`, which is also what pi does when
// `highlightCode` declines a language.
#[test]
fn code_blocks_highlight_through_the_hook() {
    let mut theme = MarkdownTheme {
        code_block: Arc::new(|s| format!("\x1b[32m{s}\x1b[0m")),
        highlight: Some(Arc::new(|code, lang| {
            (lang == "rust").then(|| vec![format!("\x1b[35m{code}\x1b[0m")])
        })),
        ..Default::default()
    };
    let rust = render_markdown(
        "```rust\nlet x = 1;\n```",
        30,
        &theme,
        &MarkdownOptions::default(),
    );
    assert!(
        rust.iter().any(|l| l.contains("\x1b[35mlet x = 1;\x1b[0m")),
        "highlighted: {rust:?}"
    );
    let bare = render_markdown(
        "```\nlet x = 1;\n```",
        30,
        &theme,
        &MarkdownOptions::default(),
    );
    assert!(
        bare.iter().any(|l| l.contains("\x1b[32mlet x = 1;\x1b[0m")),
        "the block style, not the highlight: {bare:?}"
    );
    theme.highlight = None;
    let off = render_markdown(
        "```rust\nlet x = 1;\n```",
        30,
        &theme,
        &MarkdownOptions::default(),
    );
    assert!(
        off.iter().any(|l| l.contains("\x1b[32mlet x = 1;\x1b[0m")),
        "no hook, no highlighting: {off:?}"
    );
}

// Verifies: cycle 9 - the table header is bold, and only the header is
// (pi's `renderTable` wraps each header cell in `theme.bold`; the data
// cells and the border characters stay unstyled).
#[test]
fn the_table_header_is_bold() {
    let theme = MarkdownTheme {
        bold: Arc::new(|s| format!("\x1b[1m{s}\x1b[22m")),
        ..Default::default()
    };
    let raw = render_markdown(
        "| a | b |\n| - | - |\n| 1 | 2 |",
        40,
        &theme,
        &MarkdownOptions::default(),
    );
    let header = raw
        .iter()
        .find(|l| l.contains('a') && l.contains('\u{2502}'))
        .expect("the header row");
    assert!(
        header.contains("\x1b[1ma"),
        "the header cell is bold: {header:?}"
    );
    // Match on the stripped row: the header's own `\x1b[1m` contains a
    // literal '1', which is how this assertion first fooled itself.
    let data = raw
        .iter()
        .find(|l| strip(&[(*l).to_string()])[0].contains("\u{2502} 1 \u{2502}"))
        .expect("the data row");
    assert!(
        !data.contains("\x1b[1m"),
        "only the header is bold: {data:?}"
    );
}

#[test]
fn tables_render_aligned_columns() {
    let md = "| name | value |\n| --- | --- |\n| a | 1 |\n| longer | 22 |";
    let out = strip(&render_markdown(
        md,
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out[0].starts_with('┌'));
    // Every body row has the same visible width.
    let widths: Vec<usize> = out
        .iter()
        .filter(|l| l.starts_with('│'))
        .map(|l| visible_width(l))
        .collect();
    assert!(widths.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn inline_styles_apply() {
    let theme = MarkdownTheme {
        bold: Arc::new(|s| format!("\x1b[1m{s}\x1b[0m")),
        code: Arc::new(|s| format!("\x1b[36m{s}\x1b[0m")),
        ..Default::default()
    };
    let out = render_inline("a **b** `c`", &theme, &MarkdownOptions::default());
    assert!(out.contains("\x1b[1mb\x1b[0m"));
    assert!(out.contains("\x1b[36mc\x1b[0m"));
}

#[test]
fn links_use_osc8_or_inline_fallback() {
    let theme = plain();
    let hyper = render_inline(
        "[site](http://x)",
        &theme,
        &MarkdownOptions {
            link_mode: LinkMode::Hyperlink,
            ..Default::default()
        },
    );
    assert!(hyper.contains("\x1b]8;;http://x\x1b\\"));
    assert!(!hyper.contains("(http"));
    let inline = render_inline(
        "[site](http://x)",
        &theme,
        &MarkdownOptions {
            link_mode: LinkMode::Inline,
            ..Default::default()
        },
    );
    assert!(inline.contains("site (http://x)"));
}

#[test]
fn partial_closing_fence_is_trimmed_during_streaming() {
    let out = strip(&render_markdown(
        "```\ncode line\n``",
        20,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out.iter().any(|l| l.contains("code line")));
    assert!(!out.iter().any(|l| l.contains("``")));
}

#[test]
fn html_is_rendered_as_literal_text() {
    let out = render_inline("<b>hi</b>", &plain(), &MarkdownOptions::default());
    assert_eq!(
        crate::engine::text::strip_terminal_sequences(&out),
        "<b>hi</b>"
    );
}

#[test]
fn wide_content_never_exceeds_width() {
    let out = render_markdown(
        "# a very long heading that should wrap nicely across lines",
        12,
        &plain(),
        &MarkdownOptions::default(),
    );
    assert!(out.iter().all(|l| visible_width(l) <= 12));
}

// Verifies: FR-UI-8 - an unterminated fence draws a complete frame, so
// the block never appears half-drawn while streaming (pi's behavior).
#[test]
fn an_unterminated_fence_draws_a_complete_frame() {
    let out = strip(&render_markdown(
        "```rust\nfn main() {",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out[0].starts_with('╭'));
    assert!(out.last().unwrap().starts_with('╰'));
    assert!(out.iter().any(|l| l.contains("fn main() {")));
}

// Verifies: FR-UI-8 - a half-written opening fence is plain text.
#[test]
fn a_partial_opening_fence_is_plain_text() {
    let out = strip(&render_markdown(
        "``",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out.iter().any(|l| l.contains("``")));
    assert!(!out.iter().any(|l| l.starts_with('╭')));
}

// Verifies: FR-UI-7 - the transcript renders markdown: headings, lists,
// tables with aligned columns, framed code blocks, and links.
#[test]
fn markdown_covers_the_transcript_vocabulary() {
    let raw = render_markdown(GOLDEN, 60, &plain(), &MarkdownOptions::default());
    let out = strip(&raw);
    assert!(out.iter().any(|l| l == "Title"), "heading");
    assert!(out.iter().any(|l| l.starts_with("- one")), "list");
    assert!(out.iter().any(|l| l.starts_with('┌')), "table");
    assert!(out.iter().any(|l| l.starts_with('╭')), "framed code");
    assert!(out.iter().any(|l| l.contains("quoted")), "quote");
    assert!(
        raw.iter().any(|l| l.contains("\x1b]8;;http://x\x1b\\")),
        "link as OSC 8"
    );
}

/// A golden document with every construct the streaming renderer must
/// tolerate.
const GOLDEN: &str = "# Title\n\nIntro with **bold**, `code`, and a [link](http://x).\n\n\
    - one\n- two\n\n> quoted\n\n\
    | name | value |\n| --- | --- |\n| alpha | 1 |\n\n\
    ```rust\nfn main() {\n    let x = 1;\n}\n```\n\n\
    Math $x^2$ and $$E=mc^2$$ render as math (M3).\n\n\
    - [x] shipped\n- [ ] todo\n\n\
    ```mermaid\nflowchart TD\n  A[One] --> B[Two]\n```\n";

fn frame_count(lines: &[String], open: char) -> usize {
    lines
        .iter()
        .filter(|l| {
            crate::engine::text::strip_terminal_sequences(l)
                .trim_start()
                .starts_with(open)
        })
        .count()
}

// Verifies: FR-UI-8 - streaming tolerance. For every prefix of a golden
// document, rendering neither panics nor emits an unterminated frame,
// and a table never appears without its intact separator row.
#[test]
fn every_prefix_renders_without_an_unterminated_frame() {
    for width in [40usize, 80] {
        for end in 0..=GOLDEN.len() {
            if !GOLDEN.is_char_boundary(end) {
                continue;
            }
            let prefix = &GOLDEN[..end];
            let lines = render_markdown(prefix, width, &plain(), &MarkdownOptions::default());
            let code_open = frame_count(&lines, '╭');
            let code_close = frame_count(&lines, '╰');
            assert_eq!(
                code_open, code_close,
                "unbalanced code frame at width {width}, prefix ending {end}\n{lines:?}"
            );
            let table_open = frame_count(&lines, '┌');
            let table_close = frame_count(&lines, '└');
            assert_eq!(
                table_open, table_close,
                "unbalanced table frame at width {width}, prefix ending {end}\n{lines:?}"
            );
            if !prefix.lines().any(is_table_separator) {
                assert_eq!(
                    table_open, 0,
                    "a table rendered without an intact separator at prefix ending {end}\n{lines:?}"
                );
            }
            assert!(
                lines.iter().all(|l| visible_width(l) <= width),
                "a prefix exceeded the width at {width}, ending {end}"
            );
        }
    }
}

// Verifies: FR-UI-8 - a half-written separator row renders as text.
#[test]
fn a_partial_separator_row_is_not_yet_a_table() {
    let out = strip(&render_markdown(
        "| name | value |\n| --- | ---",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(!out.iter().any(|l| l.starts_with('┌')), "{out:?}");
}

// Verifies: FR-UI-8 + TUI-10 M3 - unpaired inline markers stay literal,
// and math renders only when it is *closed*: streamed half-written `$x^2`
// prints as source until the closer arrives (pi's pending rule).
#[test]
fn unpaired_markers_stay_literal_and_pending_math_stays_raw() {
    let out = render_inline(
        "a **b and $x^2 and `",
        &plain(),
        &MarkdownOptions::default(),
    );
    let stripped = crate::engine::text::strip_terminal_sequences(&out);
    assert!(stripped.contains("**b"), "{stripped}");
    assert!(stripped.contains("$x^2"), "pending math is raw: {stripped}");
    assert!(stripped.ends_with('`'), "{stripped}");

    // Closed math renders as math (M3), the unpaired markers still don't.
    let out = render_inline(
        "a **b and $x^2$ and `",
        &plain(),
        &MarkdownOptions::default(),
    );
    let stripped = crate::engine::text::strip_terminal_sequences(&out);
    assert!(stripped.contains("x²"), "closed math renders: {stripped}");
    assert!(!stripped.contains("$x^2$"), "{stripped}");
    assert!(stripped.contains("**b"), "{stripped}");
    assert!(stripped.ends_with('`'), "{stripped}");
}

// Verifies: FR-UI-8 / D10 - the code-block border caps at the content
// width, not the terminal width.
#[test]
fn a_code_block_border_caps_at_the_content_width() {
    let out = strip(&render_markdown(
        "```\nx\n```",
        80,
        &plain(),
        &MarkdownOptions::default(),
    ));
    let top = visible_width(&out[0]);
    let bottom = visible_width(out.last().unwrap());
    assert_eq!(top, bottom);
    assert!(top < 80, "capped at the content width, got {top}");
}

// Verifies: D10/R16 - a code block is a full frame, not a half one.
#[test]
fn a_code_block_is_a_full_frame() {
    let out = strip(&render_markdown(
        "```python\nx = 1\n```",
        80,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(
        out[0].starts_with('╭') && out[0].ends_with('╮'),
        "{:?}",
        out[0]
    );
    assert!(out[0].contains("python"), "{:?}", out[0]);
    assert!(
        out[1].starts_with('│') && out[1].ends_with('│'),
        "{:?}",
        out[1]
    );
    let bottom = out.last().expect("a bottom");
    assert!(
        bottom.starts_with('╰') && bottom.ends_with('╯'),
        "{bottom:?}"
    );
}

// Verifies: M1 row 6 - pi's strict strikethrough
// (STRICT_STRIKETHROUGH_REGEX): only a well-formed `~~text~~` strikes, so
// mid-prose tildes and `~~~` runs stay literal.
#[test]
fn strikethrough_is_strict_like_pis() {
    let plain_doc = |src: &str| {
        strip(&render_markdown(
            src,
            60,
            &plain(),
            &MarkdownOptions::default(),
        ))
    };
    assert_eq!(plain_doc("~~gone~~")[0], "gone");
    assert_eq!(plain_doc("a ~~ b ~~ c")[0], "a ~~ b ~~ c");
    assert_eq!(plain_doc("~~ spaced ~~")[0], "~~ spaced ~~");
    assert_eq!(
        plain_doc("~~a~b~~")[0],
        "a~b",
        "an interior tilde is content"
    );
    assert_eq!(
        plain_doc("~~x~~~")[0],
        "~~x~~~",
        "`~~~` is not a closer run"
    );
    assert_eq!(plain_doc("~~")[0], "~~", "an unterminated run is literal");
}

// Verifies: M1 row 20 - the two escape modes pi has: normalized by
// default, raw under `preserveBackslashEscapes` (the user-message path).
#[test]
fn backslash_escapes_follow_the_two_modes() {
    let out = strip(&render_markdown(
        r"not \*em\* here",
        60,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "not *em* here");
    let preserve = MarkdownOptions {
        preserve_backslash_escapes: true,
        ..Default::default()
    };
    let out = strip(&render_markdown(
        r"not \*em\* here",
        60,
        &plain(),
        &preserve,
    ));
    assert_eq!(out[0], r"not \*em\* here");
}

// Verifies: M1 row 11/12 - the authored marker survives under
// `preserveOrderedListMarkers`, and the default renumbers with `.`
// exactly as pi's renderList does.
#[test]
fn ordered_and_unordered_markers_follow_the_preserve_option() {
    let default = MarkdownOptions::default();
    let out = strip(&render_markdown("1. one\n2. two", 60, &plain(), &default));
    assert_eq!(out[0], "1. one");
    assert_eq!(out[1], "2. two");
    let out = strip(&render_markdown("1) one\n2) two", 60, &plain(), &default));
    assert_eq!(out[0], "1. one", "the default renumbers `1)` to `1.`");
    assert_eq!(out[1], "2. two");

    let preserve = MarkdownOptions {
        preserve_ordered_list_markers: true,
        ..Default::default()
    };
    let out = strip(&render_markdown("1) one\n2) two", 60, &plain(), &preserve));
    assert_eq!(out[0], "1) one", "the authored delimiter prints");
    assert_eq!(out[1], "2) two");
    let out = strip(&render_markdown("+ plus", 60, &plain(), &preserve));
    assert_eq!(out[0], "+ plus", "the authored bullet prints");
    let out = strip(&render_markdown("+ plus", 60, &plain(), &default));
    assert_eq!(out[0], "- plus", "the default normalizes to `- `");
}

// Verifies: M1 row 14 - a blank line makes the list *loose*; it does not
// restart the ordered run (pi keeps counting inside one list token).
#[test]
fn an_ordered_run_keeps_counting_across_a_blank_line() {
    let out = strip(&render_markdown(
        "1. one\n\n1. two",
        60,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "1. one");
    assert!(
        out.iter().any(|l| l == "2. two"),
        "the second `1.` continues the run: {out:?}"
    );
}

// Verifies: M1 row 9 - a blockquote renders its children as blocks (pi's
// recursive blockquote case), and `>>` nests a second level instead of
// flattening.
#[test]
fn a_blockquote_renders_its_children_as_blocks() {
    let out = strip(&render_markdown(
        "> - item one\n> - item two",
        60,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "│ - item one", "{out:?}");
    assert_eq!(out[1], "│ - item two", "{out:?}");

    let out = strip(&render_markdown(
        "> ## Inside",
        60,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(
        out[0], "│ Inside",
        "a heading inside a quote renders as one: {out:?}"
    );

    let out = strip(&render_markdown(
        "> ```\n> x\n> ```",
        60,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(
        out[0].contains('╭'),
        "a framed fence inside a quote: {out:?}"
    );
    assert!(out.iter().any(|l| l.contains('x')), "{out:?}");
    assert!(
        out.iter().all(|l| l.starts_with("│ ")),
        "every quote line carries the border: {out:?}"
    );

    let out = strip(&render_markdown(
        ">> deep",
        60,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "│ │ deep", "two `>` levels nest: {out:?}");
}

// Verifies: M1 row 17 - GFM autolink literals (marked's default, so pi
// gets them free): bare URLs and emails become links, `www.` gains the
// scheme in its href, and mid-word text never linkifies.
#[test]
fn bare_urls_and_emails_become_links() {
    let theme = plain();
    let inline = MarkdownOptions {
        link_mode: LinkMode::Inline,
        ..Default::default()
    };
    let text = |src: &str| strip(&render_markdown(src, 80, &theme, &inline))[0].clone();
    // A trailing sentence period is not part of the URL, and the link
    // prints like plain text (label == href), so prove the match in the
    // OSC 8 form instead: the URL inside the escape has no period.
    assert_eq!(
        text("see https://example.com/a."),
        "see https://example.com/a."
    );
    assert_eq!(
        text("visit www.example.com now"),
        "visit www.example.com (http://www.example.com) now"
    );
    assert_eq!(text("mail me@example.com"), "mail me@example.com");
    assert_eq!(
        text("ahttps://x.com"),
        "ahttps://x.com",
        "mid-word: no link"
    );

    // The OSC 8 form carries the URL, never prints it, and excludes the
    // sentence's period from the link target.
    let hyper = render_markdown(
        "see https://example.com/a.",
        80,
        &theme,
        &MarkdownOptions::default(),
    );
    assert!(
        hyper[0].contains("\x1b]8;;https://example.com/a\x1b\\"),
        "the period stays outside the URL: {:?}",
        hyper[0]
    );
    assert!(!hyper[0].contains("(https"), "{:?}", hyper[0]);
}

// Verifies: M1 row 18 - pi's `image` token falls to the default inline
// case and prints its alt text only; the URL never renders (the picture
// itself is a transcript entry).
#[test]
fn an_image_prints_its_alt_text_only() {
    let out = strip(&render_markdown(
        "![alt text](http://img/x.png)",
        60,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "alt text");
    let out = strip(&render_markdown(
        "![alt text](http://img/x.png)",
        60,
        &plain(),
        &MarkdownOptions {
            link_mode: LinkMode::Inline,
            ..Default::default()
        },
    ));
    assert_eq!(out[0], "alt text");
}

// Verifies: M1 row 15 - a table narrower than 3n+1 falls back to the raw
// markdown source (pi's renderTable fallback), not to a pipe-stripped join.
#[test]
fn a_too_narrow_table_falls_back_to_the_raw_source() {
    let out = strip(&render_markdown(
        "| a | b |\n| - | - |\n| 1 | 2 |",
        6,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out[0].starts_with('|'), "the raw source line: {out:?}");
    assert!(out.iter().any(|l| l.contains("| - |")), "{out:?}");
}

// Verifies: M1 row 15 - pi's wrapCellText: the narrow styles reset after
// every non-final fragment of a wrapped cell, so a style inside the cell
// cannot bleed into the next fragment.
#[test]
fn a_wrapped_cell_resets_narrow_styles_between_fragments() {
    let raw = render_markdown(
        "| h |\n| - |\n| aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa |",
        20,
        &plain(),
        &MarkdownOptions::default(),
    );
    assert!(
        raw.iter()
            .any(|l| l.contains("\x1b[22;23;24;25;27;28;29;39m")),
        "the fragment reset is present: {raw:?}"
    );
}

// Verifies: M1 row 16 - the link is `link(underline(text))` in both modes
// (pi's inline renderer).
#[test]
fn links_carry_the_underline_pi_paints() {
    let theme = MarkdownTheme {
        link: Arc::new(|s| format!("\x1b[38;2;7;7;7m{s}\x1b[39m")),
        underline: Arc::new(|s| format!("\x1b[4m{s}\x1b[24m")),
        ..Default::default()
    };
    let hyper = render_inline(
        "[site](http://x)",
        &theme,
        &MarkdownOptions {
            link_mode: LinkMode::Hyperlink,
            ..Default::default()
        },
    );
    assert!(
        hyper.contains("\x1b[4msite\x1b[24m"),
        "underline inside the link style: {hyper:?}"
    );
    let inline = render_inline(
        "[site](http://x)",
        &theme,
        &MarkdownOptions {
            link_mode: LinkMode::Inline,
            ..Default::default()
        },
    );
    assert!(
        inline.contains(" (http://x)"),
        "the space rides inside the linkUrl style: {inline:?}"
    );
}

// Verifies: TUI-10 M3 - display math: `$$…$$` stacks what pi stacks,
// closes only when the closer ends a line, and fails soft to the raw
// source (delimiters included) on unsupported input.
#[test]
fn display_math_renders_and_fails_soft() {
    let out = strip(&render_markdown(
        "$$E=mc^2$$",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    // `=` gets pi's relation spacing (latex.ts parseSequence).
    assert_eq!(out[0], "E = mc²", "{out:?}");

    let out = strip(&render_markdown(
        "$$\n\\frac{a}{b}\n$$",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "a", "{out:?}");
    assert_eq!(out[1], "─", "{out:?}");
    assert_eq!(out[2], "b", "{out:?}");

    let out = strip(&render_markdown(
        "$$\\unknowncmd$$",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out[0].contains("\\unknowncmd"), "fail-soft: {out:?}");
    assert!(
        out[0].contains("$$"),
        "the raw source keeps its fences: {out:?}"
    );

    // The option turns math off the way pi's renderLatex:false does.
    let out = strip(&render_markdown(
        "$$E=mc^2$$",
        40,
        &plain(),
        &MarkdownOptions {
            render_latex: false,
            ..Default::default()
        },
    ));
    assert!(out[0].contains("E=mc^2"), "{out:?}");
}

// Verifies: TUI-10 M3 - a streamed, still-open display block prints as
// source (pending), and `$…$` prose guards leave dollars alone.
#[test]
fn pending_display_math_prints_as_source() {
    let out = strip(&render_markdown(
        "$$\\frac{a}",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "$$\\frac{a}", "the half block is raw: {out:?}");

    let out = strip(&render_markdown(
        "it costs $5 and that is that",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "it costs $5 and that is that", "{out:?}");

    let out = strip(&render_markdown(
        "set $FOO$x here",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "set $FOO$x here", "the identifier guard: {out:?}");
}

// Verifies: TUI-10 M3 - inline forms: `$…$`, `\(…\)`, and the escape
// awareness (an escaped delimiter never opens math).
#[test]
fn inline_math_forms_and_escapes() {
    let out = strip(&render_markdown(
        r"area is $\pi r^2$ ok",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out[0].contains("π r²"), "{out:?}");

    let out = strip(&render_markdown(
        r"inline \(a + b\) done",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "inline a + b done", "{out:?}");

    let out = strip(&render_markdown(
        r"escaped \$x\$ stays",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert_eq!(out[0], "escaped $x$ stays", "{out:?}");
}

// Verifies: TUI-10 M4 - a mermaid fence renders as Unicode art (never the
// framed code block), a too-wide diagram falls back to the frame (pi's
// width guard), and a diagram outside the subset keeps its source.
#[test]
fn mermaid_fences_render_as_art_and_fail_soft() {
    let out = strip(&render_markdown(
        "```mermaid\nflowchart TD\n  A[One] --> B[Two]\n```",
        60,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out.iter().any(|l| l.contains('┌')), "the art box: {out:?}");
    assert!(out.iter().any(|l| l.contains("One")), "{out:?}");
    assert!(
        !out.iter().any(|l| l.starts_with('╭')),
        "art is not the code frame: {out:?}"
    );

    let out = strip(&render_markdown(
        "```mermaid\nflowchart TD\n  A[One] --> B[Two]\n```",
        4,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(
        out.iter().any(|l| l.starts_with('╭')),
        "too wide for the pane: framed source: {out:?}"
    );

    let out = strip(&render_markdown(
        "```mermaid\npie title Pets\n  \"Dogs\": 42\n```",
        60,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(
        out.iter().any(|l| l.starts_with('╭')),
        "outside the subset: framed source: {out:?}"
    );
    assert!(out.iter().any(|l| l.contains("pie")), "{out:?}");
}

// Verifies: M4 - pi's warning contract: the raw source plus a styled note
// once the message settles, art while it is still streaming.
#[test]
fn mermaid_warnings_show_only_after_streaming() {
    let src = "```mermaid\nflowchart TD\n  A --> B\n  B --> A\n```";
    let out = strip(&render_markdown(
        src,
        60,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(
        out.iter().any(|l| l.contains("not rendered")),
        "the warning note: {out:?}"
    );
    let out = strip(&render_markdown(
        src,
        60,
        &plain(),
        &MarkdownOptions {
            streaming: true,
            ..Default::default()
        },
    ));
    assert!(
        !out.iter().any(|l| l.contains("not rendered")),
        "streaming suppresses the note: {out:?}"
    );
}

// Verifies: TUI-10 M5 - pi's streaming tolerance in the nested contexts
// (markdown.md section 1 trims partial fences recursively): a quote's
// fence and a list's fence balance at every chunk boundary.
#[test]
fn partial_fences_stay_balanced_inside_quotes_and_lists() {
    // Inside a blockquote: the child block loop runs the same fence path.
    let out = strip(&render_markdown(
        "> ```\n> code\n> `",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out.iter().any(|l| l.contains('╭')), "{out:?}");
    assert!(out.iter().any(|l| l.contains("code")), "{out:?}");
    assert!(
        out.iter().all(|l| l.starts_with("│ ")),
        "the quote border stays on every row: {out:?}"
    );
    let opens = out.iter().filter(|l| l.contains('╭')).count();
    let closes = out.iter().filter(|l| l.contains('╰')).count();
    assert_eq!(opens, closes, "balanced inside the quote: {out:?}");

    // Inside a list item: the fence is still a fence, partial or whole.
    let out = strip(&render_markdown(
        "- item\n```\ncode\n```",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(out.iter().any(|l| l.contains("item")), "{out:?}");
    assert!(out.iter().any(|l| l.contains("code")), "{out:?}");
    let opens = out.iter().filter(|l| l.contains('╭')).count();
    let closes = out.iter().filter(|l| l.contains('╰')).count();
    assert_eq!(opens, closes, "balanced inside the list: {out:?}");
}

// Verifies: M5 - a half-drawn mermaid diagram cannot parse, so it renders
// as source (pi: grok-mermaid returns nothing, the transformer keeps the
// raw block) and becomes art the moment it does.
#[test]
fn partial_mermaid_renders_as_source_until_it_parses() {
    let art = crate::widgets::mermaid::render("flowchart TD\n  A[One] -->");
    assert!(art.is_none(), "an unfinished edge cannot parse");

    let art = crate::widgets::mermaid::render("flowchart TD\n  A[One] --> B[Two]");
    assert!(art.is_some(), "the finished edge parses");

    let out = strip(&render_markdown(
        "```mermaid\nflowchart TD\n  A[One] -->",
        40,
        &plain(),
        &MarkdownOptions::default(),
    ));
    assert!(
        out.iter().any(|l| l.starts_with('╭')),
        "half a diagram renders as framed source: {out:?}"
    );
    let opens = out.iter().filter(|l| l.contains('╭')).count();
    let closes = out.iter().filter(|l| l.contains('╰')).count();
    assert_eq!(opens, closes, "{out:?}");
}

// Verifies: M5 - tasks and mermaid live in the golden document, so the
// every-prefix streaming sweep covers them byte by byte.
#[test]
fn the_golden_document_carries_tasks_and_a_diagram() {
    assert!(GOLDEN.contains("- [x] shipped"), "task row");
    assert!(GOLDEN.contains("```mermaid"), "diagram row");
}

// Verifies: gh #82 - the mermaid mode gates the art: off keeps the
// fence raw, final waits for the settled message, streaming renders
// mid-stream too (pi's `markdown.mermaid`).
#[test]
fn mermaid_mode_gates_the_art() {
    let fence = "```mermaid\nflowchart TD\n  A[One] --> B[Two]\n```";
    let render = |mode: MermaidMode, streaming: bool| {
        let options = MarkdownOptions {
            mermaid: mode,
            streaming,
            ..MarkdownOptions::default()
        };
        strip(&render_markdown(fence, 60, &plain(), &options))
    };
    let out = render(MermaidMode::Off, false);
    assert!(
        out.iter().any(|l| l.starts_with('╭')),
        "off keeps the frame: {out:?}"
    );
    let out = render(MermaidMode::Final, true);
    assert!(
        out.iter().any(|l| l.starts_with('╭')),
        "final waits out streaming: {out:?}"
    );
    let out = render(MermaidMode::Final, false);
    assert!(
        out.iter().any(|l| l.contains('┌')),
        "final renders settled: {out:?}"
    );
    let out = render(MermaidMode::Streaming, true);
    assert!(
        out.iter().any(|l| l.contains('┌')),
        "streaming renders mid-stream: {out:?}"
    );
}

// Verifies: gh #82 - the code indent prefixes every block line (pi's
// `markdown.codeBlockIndent`); empty keeps the shipped rows.
#[test]
fn code_indent_prefixes_block_lines() {
    let fence = "```rust\nlet x = 1;\n```";
    let render = |indent: &str| {
        let options = MarkdownOptions {
            code_indent: indent.to_string(),
            ..MarkdownOptions::default()
        };
        strip(&render_markdown(fence, 60, &plain(), &options))
    };
    let plain_out = render("");
    assert!(
        plain_out.iter().any(|l| l.starts_with('╭')),
        "unindented frame: {plain_out:?}"
    );
    let out = render(">>");
    assert!(
        out.iter().any(|l| l.starts_with(">>╭")),
        "the frame indents too: {out:?}"
    );
    assert!(
        out.iter().any(|l| l.starts_with(">>│")),
        "content indents: {out:?}"
    );
}
