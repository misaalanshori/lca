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
    assert!(hyper.contains("\x1b]8;;http://x\x07"));
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
        raw.iter().any(|l| l.contains("\x1b]8;;http://x\x07")),
        "link as OSC 8"
    );
}

/// A golden document with every construct the streaming renderer must
/// tolerate.
const GOLDEN: &str = "# Title\n\nIntro with **bold**, `code`, and a [link](http://x).\n\n\
    - one\n- two\n\n> quoted\n\n\
    | name | value |\n| --- | --- |\n| alpha | 1 |\n\n\
    ```rust\nfn main() {\n    let x = 1;\n}\n```\n\n\
    Math $x^2$ and $$E=mc^2$$ stay literal.\n";

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

// Verifies: FR-UI-8 - unpaired inline markers and math render literally.
#[test]
fn unpaired_inline_markers_and_math_stay_literal() {
    let out = render_inline(
        "a **b and $x^2$ and `",
        &plain(),
        &MarkdownOptions::default(),
    );
    let stripped = crate::engine::text::strip_terminal_sequences(&out);
    assert!(stripped.contains("**b"), "{stripped}");
    assert!(stripped.contains("$x^2$"), "{stripped}");
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
