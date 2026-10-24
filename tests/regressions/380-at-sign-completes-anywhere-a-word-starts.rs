//! Gh #380 gap 1: `@file` completed only at **column 0**, because the
//! extraction was `text.strip_prefix('@')`.
//!
//! So `Check @src/main.rs`, `(@src/m…` and `请查看，@src/main.ts` never
//! proposed anything: the feature worked in the one position a user rarely
//! types it. The extraction now walks back to the last *delimiter* (line
//! start, ASCII whitespace, CJK punctuation) and refuses an `@` that sits
//! inside a word, which is what keeps `a@b.com` from opening a file menu.
//!
//! The asymmetry with `/` is the design and is pinned here as hard as the
//! fix itself: a mid-sentence `/` in `https://…` or `src/index.ts` must
//! never open a command menu, so `/` stays line-start-only.
//!
//! Gaps 2 (`@`-quoted paths), 3 (`/s` ranking) and 4 (depth scoring) are
//! out of scope for this cycle.
//!
//! Verifies: FR-UI-9 (defect gh #380 gap 1). The unit rows live in
//! `crates/lca-tui/src/widgets/autocomplete/attachment_tests.rs` and
//! `.../autocomplete/token.rs`; this file is the NFR-24 guard.

use std::path::PathBuf;
use std::sync::Arc;

use lca_tui::widgets::autocomplete::{
    AutocompleteProvider, CombinedAutocompleteProvider, SlashCommand,
};
use lca_tui::widgets::editor::Editor;

/// A scratch tree: `src/main.rs` plus a Next.js-shaped route pair, so both
/// the ordinary completion and the route-preservation rule have something
/// real to complete against.
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("lca-reg380-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["src", "app/(marketing)", "app/[slug]"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        std::fs::write(root.join("src/main.rs"), "x").expect("write");
        std::fs::write(root.join("app/(marketing)/page.tsx"), "x").expect("write");
        std::fs::write(root.join("app/[slug]/page.tsx"), "x").expect("write");
        Self { root }
    }

    fn provider(&self) -> CombinedAutocompleteProvider {
        CombinedAutocompleteProvider::new(Vec::new(), &self.root)
    }

    /// A provider that also owns one slash command, so the `/`-asymmetry
    /// row can tell a command menu from a file list.
    fn provider_with_commands(&self) -> CombinedAutocompleteProvider {
        CombinedAutocompleteProvider::new(
            vec![SlashCommand {
                name: "model".to_string(),
                description: Some("Pick a model".to_string()),
                argument_hint: None,
                argument_completions: None,
            }],
            &self.root,
        )
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn values(suggestions: Option<lca_tui::widgets::autocomplete::Suggestions>) -> Vec<String> {
    suggestions
        .map(|s| s.items.into_iter().map(|i| i.value).collect())
        .unwrap_or_default()
}

#[test]
fn an_at_sign_anywhere_a_word_starts_completes_a_file() {
    let tree = Tree::new("word-start");
    let provider = tree.provider();
    for line in [
        "@src/m",         // column 0, the old behaviour
        "Check @src/m",   // the issue's own case
        "(@src/m",        // unclosed prose wrapper
        "`@src/m",        // backtick wrapper
        "请查看，@src/m", // CJK punctuation, no ASCII space (the issue's case)
        "请查看。@src/m", // CJK full stop
        "\u{3000}@src/m", // ideographic space
    ] {
        let got = values(provider.get_suggestions(line, false));
        assert!(
            got.iter().any(|v| v == "@src/main.rs"),
            "`{line}` completes the attachment: {got:?}"
        );
    }
}

#[test]
fn an_at_sign_inside_a_word_never_completes_a_file() {
    let tree = Tree::new("in-word");
    let provider = tree.provider();
    // An address or a handle, and the CJK-letter shape that pi's own suite
    // rejects for the same reason: the character before the `@` belongs to
    // the same word.
    for line in [
        "a@b.com",
        "user@src/m",
        "查看@src/m",
        "あ@src/m",
        "々@src/m",
    ] {
        assert!(
            provider.get_suggestions(line, false).is_none(),
            "`{line}` is a word containing an `@`, not an attachment"
        );
    }
}

/// The row that goes red if someone generalises the tokenizer to "after any
/// delimiter" for both markers. This is the half of the asymmetry that must
/// not move.
#[test]
fn a_mid_sentence_slash_never_opens_a_command_menu() {
    let tree = Tree::new("slash");
    let provider = tree.provider_with_commands();

    for line in [
        "See https://example.com/pa",          // a URL's slashes
        "Please see https://example.com/docs", // another URL
        "open src/mod",                        // a relative path mid-sentence
        "请查看，/mod",                        // CJK punctuation then a slash
    ] {
        let suggestions = provider.get_suggestions(line, false);
        let opened_commands = suggestions
            .as_ref()
            .is_some_and(|s| s.items.iter().any(|i| i.label.starts_with('/')));
        assert!(
            !opened_commands,
            "`{line}` must not open the command menu: {suggestions:?}"
        );
    }

    // And at column 0 it still does, so the row is not passing by accident.
    let at_start = provider.get_suggestions("/mod", false);
    assert!(
        at_start.is_some_and(|s| s.items.iter().any(|i| i.label == "/model")),
        "a line-start `/` still completes commands"
    );
}

#[test]
fn a_route_shaped_path_keeps_its_brackets() {
    let tree = Tree::new("routes");
    let provider = tree.provider();
    // Closed wrapper ⇒ part of the route name; stripping it would break
    // Next.js-style paths for the people who use them.
    for (line, expected) in [
        ("app/(marketing)/pa", "app/(marketing)/page.tsx"),
        ("app/[slug]/pa", "app/[slug]/page.tsx"),
        ("@app/(marketing)/pa", "@app/(marketing)/page.tsx"),
        ("@app/[slug]/pa", "@app/[slug]/page.tsx"),
    ] {
        let got = values(provider.get_suggestions(line, false));
        assert!(
            got.iter().any(|v| v == expected),
            "`{line}` keeps its closed wrapper: {got:?}"
        );
    }
}

/// The text surgery, which is where a "completes" fix can still be wrong:
/// accepting must touch the token alone.
#[test]
fn accepting_inserts_the_path_once_and_keeps_the_prose() {
    for (tag, typed, expected) in [
        ("prose", "Check @src/m", "Check @src/main.rs"),
        ("cjk", "请查看，@src/m", "请查看，@src/main.rs"),
        ("wrapper", "(@src/m", "(@src/main.rs"),
        ("column0", "@src/m", "@src/main.rs"),
    ] {
        let tree = Tree::new(tag);
        let mut editor = Editor::new();
        editor.set_autocomplete(Arc::new(tree.provider()));
        editor.set_text(typed);
        editor.refresh_suggestions(false);
        editor.accept_suggestion();
        assert_eq!(
            editor.text(),
            expected,
            "`{typed}` accepts without swallowing prose or duplicating the `@`"
        );
    }
}
