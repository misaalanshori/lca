//! Gh #380 gap 1: `@file` completes wherever a *word* starts, not only at
//! column 0.
//!
//! Split from `autocomplete.rs`'s own `tests` module for the workspace's
//! 1,200-line ceiling (gate 11); that module keeps the slash-command and
//! path rows, and this one holds the attachment-token rows.
//!
//! The rule these pin is **asymmetric on purpose** (see
//! [`super::token`]): `@` is a reference marker and belongs wherever a word
//! starts, while `/` stays line-start-only so a mid-sentence `/` in
//! `https://…` or `src/index.ts` never pops a command menu.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{AutocompleteProvider, CombinedAutocompleteProvider};

/// A scratch tree with one file, so a `@` query has something to match.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("lca-ac-380-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::write(root.join("src/main.rs"), "x").expect("write");
        std::fs::write(root.join("README.md"), "x").expect("write");
        Self { root }
    }

    fn provider(&self) -> CombinedAutocompleteProvider {
        CombinedAutocompleteProvider::new(Vec::new(), &self.root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The `.value` of every suggested row: what accepting the row inserts.
///
/// Paths are normalised to `/` because the walk builds them with the native
/// separator - the same `to_display_path` convention the existing rows in
/// `autocomplete.rs` use, and what makes these pass on Windows.
fn values(suggestions: Option<super::Suggestions>) -> Vec<String> {
    normalise(
        suggestions
            .map(|s| s.items.into_iter().map(|i| i.value).collect())
            .unwrap_or_default(),
    )
}

/// The `.description` of every suggested row, which for a file row is the
/// path relative to the base directory.
fn descriptions(suggestions: Option<super::Suggestions>) -> Vec<String> {
    normalise(
        suggestions
            .map(|s| {
                s.items
                    .iter()
                    .filter_map(|i| i.description.clone())
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// pi's `toDisplayPath`: the walk joins with the OS separator, so a row that
/// should read `src/main.rs` reads `src\main.rs` on Windows.
fn normalise(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .map(|path| path.replace('\\', "/"))
        .collect()
}

// The issue's own case. `@` at the start of a *word* starts an attachment;
// before the fix `strip_prefix('@')` only matched at column 0.
#[test]
fn a_mid_sentence_at_sign_proposes_files() {
    let fixture = Fixture::new("mid");
    let got = descriptions(fixture.provider().get_suggestions("Check @src/m", false));
    assert!(
        got.iter().any(|d| d == "src/main.rs"),
        "a mid-sentence `@` completes: {got:?}"
    );
}

// An *unclosed* prose wrapper is stripped, so `(@src/m` completes as
// `src/m` rather than looking for a directory called `(@src`.
#[test]
fn an_unclosed_wrapper_before_the_at_sign_is_stripped() {
    let fixture = Fixture::new("wrapper-open");
    for line in ["(@src/m", "see [@src/m", "`@src/m", "<@src/m", "{@src/m"] {
        let got = descriptions(fixture.provider().get_suggestions(line, false));
        assert!(
            got.iter().any(|d| d == "src/main.rs"),
            "the unclosed wrapper before `@` is stripped ({line}): {got:?}"
        );
    }
}

// A *closed* wrapper is part of a route-shaped path, not prose. Stripping
// here would break Next.js-style `(marketing)`/`[slug]` routes.
#[test]
fn a_closed_wrapper_is_part_of_the_path_and_is_kept() {
    let root = std::env::temp_dir().join(format!("lca-ac-380-route-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("app/(marketing)")).expect("mkdir");
    std::fs::create_dir_all(root.join("app/[slug]")).expect("mkdir");
    std::fs::write(root.join("app/(marketing)/page.tsx"), "x").expect("write");
    std::fs::write(root.join("app/[slug]/page.tsx"), "x").expect("write");
    let provider = CombinedAutocompleteProvider::new(Vec::new(), &root);

    // The query carries the closing bracket, so it is a route, not prose.
    // These go through the plain *path* branch (no `@`), whose rows carry the
    // path in `value` and no description, so assert on the values.
    for (line, dir) in [
        ("app/(marketing)/pa", "app/(marketing)"),
        ("app/[slug]/pa", "app/[slug]"),
    ] {
        let values = values(provider.get_suggestions(line, false));
        assert!(
            values.iter().any(|v| v == &format!("{dir}/page.tsx")),
            "the closed wrapper stays part of the path ({line}): {values:?}"
        );
    }
    // And the same through an `@`, which is how a user actually types it.
    let got = descriptions(provider.get_suggestions("@app/(marketing)/pa", false));
    assert!(
        got.iter().any(|d| d == "app/(marketing)/page.tsx"),
        "a route under an `@` keeps its brackets: {got:?}"
    );
    let values = values(provider.get_suggestions("@app/(marketing)/pa", false));
    assert!(
        values.iter().any(|v| v == "@app/(marketing)/page.tsx"),
        "and the inserted value keeps them too: {values:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// CJK prose has no ASCII space before the `@`, so a whitespace-only
// tokenizer misses the most common way a CJK user types it.
#[test]
fn a_cjk_sentence_completes_the_attachment() {
    let fixture = Fixture::new("cjk");
    for line in [
        "请查看，@src/m", // fullwidth comma - the issue's own case
        "请查看。@src/m", // fullwidth full stop
        "请查看：@src/m", // fullwidth colon
        "请查看、@src/m", // ideographic comma
        "请查看「@src/m", // CJK bracket
        "　@src/m",       // ideographic space
    ] {
        let got = descriptions(fixture.provider().get_suggestions(line, false));
        assert!(
            got.iter().any(|d| d == "src/main.rs"),
            "CJK prose before the `@` ({line}): {got:?}"
        );
    }
}

// CJK *letters* are letters, not separators: `查看@src` is one word ending
// in an `@`, which is an email/handle shape, not an attachment.
#[test]
fn a_cjk_letter_before_the_at_sign_is_not_a_word_start() {
    let fixture = Fixture::new("cjk-letter");
    for line in [
        "查看@src/m",
        "あ@src/m",
        "カ@src/m",
        "한@src/m",
        "ㄅ@src/m",
        "々@src/m",
    ] {
        assert!(
            fixture.provider().get_suggestions(line, false).is_none(),
            "a CJK letter is part of the word, so `@` is not a word start: {line}"
        );
    }
}

// **The row that matters most**: `@` and `/` are asymmetric, and this is
// what goes red if someone generalises the tokenizer to "after any
// delimiter" for both. A mid-sentence `/` must never pop a command menu.
#[test]
fn a_mid_sentence_slash_does_not_pop_a_command_menu() {
    let fixture = Fixture::new("slash-asymmetry");
    let provider = CombinedAutocompleteProvider::new(
        vec![super::SlashCommand {
            name: "model".to_string(),
            description: Some("Pick a model".to_string()),
            argument_hint: None,
            argument_completions: None,
        }],
        &fixture.root,
    );
    // `https://…` and `src/index.ts` are the two shapes the rule exists for.
    for line in ["See https://example.com/pa", "open src/mod", "see ./mod"] {
        let suggestions = provider.get_suggestions(line, false);
        let is_command_menu = suggestions
            .as_ref()
            .is_some_and(|s| s.items.iter().any(|i| i.label.starts_with('/')));
        assert!(
            !is_command_menu,
            "a mid-sentence `/` must not open the command menu ({line}): {suggestions:?}"
        );
    }
    // And a column-0 `/` still does, so the row is not passing by accident.
    let at_start = provider.get_suggestions("/mod", false);
    assert!(
        at_start.is_some_and(|s| s.items.iter().any(|i| i.label == "/model")),
        "a line-start `/` still completes commands"
    );
}

// An email or a handle is not an attachment: the character before the `@`
// is part of the same word.
#[test]
fn an_email_address_does_not_start_an_attachment() {
    let fixture = Fixture::new("email");
    for line in ["a@b.com", "user@src", "me@src/m", "x@README"] {
        assert!(
            fixture.provider().get_suggestions(line, false).is_none(),
            "`{line}` is an address, not an attachment"
        );
    }
}

// Accepting a mid-sentence completion must be text surgery on the token
// alone: the prose before it survives, the `@` is not duplicated, and the
// path lands once.
#[test]
fn accepting_a_mid_sentence_completion_keeps_the_prose_and_inserts_once() {
    let fixture = Fixture::new("accept");
    let mut editor = crate::widgets::editor::Editor::new();
    editor.set_autocomplete(Arc::new(fixture.provider()));
    editor.set_text("Check @src/m");
    editor.refresh_suggestions(false);
    editor.accept_suggestion();
    assert_eq!(
        editor.text().replace('\\', "/"),
        "Check @src/main.rs",
        "the prose is kept and the path is inserted once"
    );

    // Same through CJK prose.
    let mut editor = crate::widgets::editor::Editor::new();
    editor.set_autocomplete(Arc::new(fixture.provider()));
    editor.set_text("请查看，@src/m");
    editor.refresh_suggestions(false);
    editor.accept_suggestion();
    assert_eq!(
        editor.text().replace('\\', "/"),
        "请查看，@src/main.rs",
        "CJK prose is kept and the path is inserted once"
    );

    // Same inside an unclosed wrapper: the wrapper is prose and stays.
    let mut editor = crate::widgets::editor::Editor::new();
    editor.set_autocomplete(Arc::new(fixture.provider()));
    editor.set_text("(@src/m");
    editor.refresh_suggestions(false);
    editor.accept_suggestion();
    assert_eq!(
        editor.text().replace('\\', "/"),
        "(@src/main.rs",
        "the unclosed wrapper stays in the line and is not re-inserted"
    );
}

// The no-regression row: a column-0 `@` behaves exactly as before.
#[test]
fn a_column_zero_at_sign_still_works_the_same() {
    let fixture = Fixture::new("col0");
    let suggestions = fixture.provider().get_suggestions("@src/m", false);
    let suggestions = suggestions.expect("a column-0 `@` completes");
    assert_eq!(
        suggestions.prefix, "@src/m",
        "the prefix is the whole token, so accept replaces exactly it"
    );
    let got = descriptions(Some(suggestions));
    assert!(got.iter().any(|d| d == "src/main.rs"), "{got:?}");

    // Bare `@` still lists the tree.
    let bare = fixture
        .provider()
        .get_suggestions("@", false)
        .expect("bare @");
    assert!(!bare.items.is_empty(), "bare `@` lists entries");
}

// The delimiter set lives in one place and says so: every character in it
// starts a token, and the set is not whitespace-only.
#[test]
fn the_delimiter_set_is_named_in_one_place() {
    use super::token;
    // Start of line: a word starts at 0.
    assert!(token::is_word_start(""));
    // ASCII whitespace.
    for prefix in [" ", "\t", "\n", "Check ", "a  "] {
        assert!(
            token::is_word_start(prefix),
            "ASCII whitespace starts a word: {prefix:?}"
        );
    }
    // CJK punctuation (the ideographic comma/full stop/colon and friends).
    for prefix in [
        "，", "。", "：", "；", "！", "？", "（", "）", "「", "」", "《", "…", "—", "\u{3000}",
    ] {
        assert!(
            token::is_word_start(prefix),
            "CJK punctuation starts a word: {prefix:?}"
        );
    }
    // A letter is not a delimiter, in either script. This is the half that
    // keeps `查看@src` and `a@b.com` from opening a file menu.
    for prefix in ["a", "Z", "0", "查", "あ", "한", "々", "Ａ"] {
        assert!(
            !token::is_word_start(prefix),
            "a letter is part of the word: {prefix:?}"
        );
    }
    // Path characters are not delimiters either - `src/index.ts` is one
    // token, which is why a `/` inside it cannot start a command.
    for prefix in ["/", ".", "-", "_", ":"] {
        assert!(
            !token::is_word_start(prefix),
            "a path character is part of the word: {prefix:?}"
        );
    }
}

// Guards the `Path` import above from being an unused-warning build break
// on a platform where the fixture path type differs.
#[test]
fn the_fixture_base_path_is_a_path() {
    let fixture = Fixture::new("path");
    let base: &Path = &fixture.root;
    assert!(base.is_dir());
}
