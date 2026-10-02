//! Markdown style-leakage property test (issue #5).
//!
//! Verifies: W1 (issue #5) - for every marker (bold, italic, strike, code, link, heading, quote)
//! across construct seams, the text/row after never leaks open SGR or styles.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use lca_tui::engine::text::strip_terminal_sequences;
use lca_tui::widgets::markdown::{MarkdownOptions, MarkdownTheme, render_markdown};
use proptest::prelude::*;
use std::sync::Arc;

fn test_theme() -> MarkdownTheme {
    MarkdownTheme {
        heading: Arc::new(|s| format!("\x1b[1;38;2;255;0;0m{s}\x1b[22;39m")),
        bold: Arc::new(|s| format!("\x1b[1m{s}\x1b[22m")),
        italic: Arc::new(|s| format!("\x1b[3m{s}\x1b[23m")),
        strike: Arc::new(|s| format!("\x1b[9m{s}\x1b[29m")),
        code: Arc::new(|s| format!("\x1b[38;2;0;255;0m{s}\x1b[39m")),
        quote: Arc::new(|s| format!("\x1b[38;2;128;128;128m{s}\x1b[39m")),
        quote_border: Arc::new(|s| format!("\x1b[38;2;80;80;80m{s}\x1b[39m")),
        link: Arc::new(|s| format!("\x1b[38;2;0;0;255m{s}\x1b[39m")),
        link_url: Arc::new(|s| format!("\x1b[38;2;100;100;100m{s}\x1b[39m")),
        underline: Arc::new(|s| format!("\x1b[4m{s}\x1b[24m")),
        ..Default::default()
    }
}

#[test]
fn strikethrough_never_leaks_to_following_line() {
    let theme = test_theme();
    let md = "~~struck text~~\nnormal text";
    let lines = render_markdown(md, 80, &theme, &MarkdownOptions::default());
    assert!(lines.len() >= 2);
    // Line 1 has strikethrough
    assert!(lines[0].contains("\x1b[9m"));
    // Line 1 must end with a reset (or closing 29m)
    assert!(lines[0].contains("\x1b[29m") || lines[0].contains("\x1b[22;23;24;25;27;28;29;39m"));
    // Line 2 MUST NOT have any strike SGR
    assert!(
        !lines[1].contains("\x1b[9m"),
        "line 2 has leaked strikethrough: {:?}",
        lines[1]
    );
    assert_eq!(strip_terminal_sequences(&lines[1]), "normal text");
}

#[test]
fn all_markdown_constructs_clean_line_endings() {
    let theme = test_theme();
    let constructs = [
        "# Heading 1",
        "## Heading 2",
        "> Quoted block with ~~strike~~ and **bold**",
        "- List item with *italic* and `code`",
        "| Col 1 | Col 2 |\n|---|---|\n| Cell **1** | Cell ~~2~~ |",
        "```rust\nfn main() { println!(\"hello\"); }\n```",
        "[Link label](https://example.com)",
        "~~standalone strike~~",
        "**bold then normal** and *italic*",
    ];

    for c in constructs {
        let text = format!("{c}\nafter line");
        let lines = render_markdown(&text, 60, &theme, &MarkdownOptions::default());
        let after_idx = lines
            .iter()
            .position(|l| l.contains("after line"))
            .expect("after line exists");
        let after_row = &lines[after_idx];
        assert!(
            !after_row.contains("\x1b[9m"),
            "strikethrough leaked in {c:?}: {after_row:?}"
        );
        assert!(
            !after_row.contains("\x1b[1m"),
            "bold leaked in {c:?}: {after_row:?}"
        );
        assert!(
            !after_row.contains("\x1b[3m"),
            "italic leaked in {c:?}: {after_row:?}"
        );
        assert!(
            !after_row.contains("\x1b[4m"),
            "underline leaked in {c:?}: {after_row:?}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    // Property test: combinations of styled markers followed by unstyled text never leak SGR
    #[test]
    fn prop_markdown_styles_never_leak_across_lines(
        word1 in "[a-zA-Z0-9]{1,10}",
        word2 in "[a-zA-Z0-9]{1,10}",
        marker in prop::sample::select(vec!["**", "*", "~~", "`"])
    ) {
        let theme = test_theme();
        let md = format!("{marker}{word1}{marker}\n{word2}");
        let lines = render_markdown(&md, 80, &theme, &MarkdownOptions::default());
        prop_assert!(lines.len() >= 2);
        let second_line = &lines[1];
        prop_assert!(!second_line.contains("\x1b[9m"), "strikethrough leaked: {second_line}");
        prop_assert!(!second_line.contains("\x1b[1m"), "bold leaked: {second_line}");
        prop_assert!(!second_line.contains("\x1b[3m"), "italic leaked: {second_line}");
        prop_assert_eq!(strip_terminal_sequences(second_line), word2);
    }
}
