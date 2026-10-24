//! GitHub #383 gap 1: `Ctrl+W` deletes a segment, not a path.
//!
//! Word motion and deletion were **whitespace-only**, so `Ctrl+W` on
//! `crates/lca-tui/src/widgets/editor.rs:42` or `foo.bar(baz_qux)` deleted every
//! character at once. `text.rs` already had `PUNCTUATION` and `is_punctuation_char`;
//! the editor simply never used them.
//!
//! The behavioural rows - the segmentation table, the motion/deletion agreement, the
//! paste-marker row, and the conformance table transcribed from pi's
//! `word-navigation.test.ts` - live in `crates/lca-tui/src/widgets/editor_word_tests.rs`.
//! This file is the NFR-24 named guard: it pins the structure that keeps the three
//! callers agreeing, which is the property a future refactor is most likely to break.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

fn strip_tests(source: &str) -> String {
    match source.rfind("\n#[cfg(test)]") {
        Some(at) => source[..at].to_string(),
        None => source.to_string(),
    }
}

fn editor_source() -> String {
    strip_tests(&read("crates/lca-tui/src/widgets/editor.rs"))
}

fn word_source() -> String {
    strip_tests(&read("crates/lca-tui/src/engine/word.rs"))
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

// Verifies: gh #383 gap 1 design call 1 - **one classifier, three callers**.
//
// Motion left, motion right and deletion must agree about where a word begins and ends.
// If each decides for itself they drift, and a `Ctrl+W` that lands somewhere `Alt+Left`
// cannot reach is this same bug in a new shape. So all three must delegate to
// `engine::word`, and none may carry its own loop over `is_whitespace`.
#[test]
fn all_three_callers_delegate_to_one_classifier() {
    let editor = editor_source();
    for (signature, helper) in [
        ("pub fn delete_word_backward(", "word_start_before"),
        ("pub fn cursor_word_left(", "word_start_before"),
        ("pub fn cursor_word_right(", "word_end_after"),
    ] {
        let body = function_body(&editor, signature);
        assert!(
            body.contains(helper),
            "{signature} must delegate to {helper}: {body}"
        );
        assert!(
            !body.contains("is_whitespace()"),
            "{signature} must not walk whitespace itself - that is the pre-fix shape: \
             {body}"
        );
    }
}

// Verifies: gh #383 gap 1 design call 2 - the classifier distinguishes **three** classes,
// with punctuation as its own.
//
// Treating punctuation as "anything non-alphanumeric" gives `(` then `)` then `-`, which
// is worse than the whitespace-only behaviour it replaces. The class must be explicit,
// and the punctuation set is the repo's existing `PUNCTUATION`.
#[test]
fn the_classifier_has_three_classes_including_punctuation() {
    let word = word_source();
    assert!(
        word.contains("enum WordClass"),
        "there must be a named classification"
    );
    for variant in ["Word", "Punctuation", "Space"] {
        assert!(
            word.contains(&format!("{variant},")),
            "the classification must include {variant}"
        );
    }
    let body = function_body(&word, "pub fn word_class(");
    assert!(
        body.contains("is_punctuation_char"),
        "punctuation must use the existing set, not a second copy: {body}"
    );
    // And it is not a second punctuation table.
    assert_eq!(
        word.matches("const PUNCTUATION").count(),
        0,
        "the punctuation set stays in `text.rs`; `word.rs` borrows it"
    );
}

// Verifies: gh #383 gap 1 design call 3 - **deletion and motion are the same walk**.
//
// `Ctrl+W` must remove exactly what `Alt+Left` crosses. That is structural here: both
// call `word_start_before`, so they cannot disagree. This row fails if deletion ever
// grows its own copy of the walk.
#[test]
fn deletion_and_backward_motion_share_the_walk() {
    let editor = editor_source();
    let del = function_body(&editor, "pub fn delete_word_backward(");
    let mv = function_body(&editor, "pub fn cursor_word_left(");
    let call = "crate::engine::text::word_start_before(&chars, self.cursor_col)";
    assert!(del.contains(call), "deletion calls the shared walk: {del}");
    assert!(mv.contains(call), "and so does motion: {mv}");
}

// Verifies: gh #383 gap 1 design call 4 - a **paste marker is atomic**.
//
// A caret landing inside `[paste #1 +3 lines]` is a different class of bug: the marker
// is one token, so motion hops it whole and deletion removes it whole. pi models this as
// `isAtomicSegment`.
#[test]
fn a_paste_marker_is_atomic_in_both_directions() {
    let word = word_source();
    assert!(
        word.contains("pub fn atomic_span_at("),
        "the atomic-segment recogniser exists"
    );
    let backward = function_body(&word, "pub fn word_start_before(");
    assert!(
        backward.contains("atomic_start_ending_at"),
        "backward motion hops an atomic segment whole: {backward}"
    );
    let forward = function_body(&word, "pub fn word_end_after(");
    assert!(
        forward.contains("atomic_span_at"),
        "and so does forward motion: {forward}"
    );
}

// Verifies: gh #383 gap 1 - the ASCII `_` is a **word** character, which is what makes
// `baz_qux` one segment rather than `baz` + `_` + `qux`.
//
// This is the detail my own first segmentation table got wrong, so it gets a guard of
// its own: if `_` ever became punctuation, `foo.bar(baz_qux)` would stop at `qux` and
// every row in the conformance table would shift.
#[test]
fn underscore_is_not_punctuation() {
    let text = read("crates/lca-tui/src/engine/text.rs");
    let line = text
        .lines()
        .find(|l| l.contains("pub const PUNCTUATION"))
        .expect("the punctuation set");
    assert!(
        !line.contains('_'),
        "`_` is a word character, so it must not be in the punctuation set: {line}"
    );
}
