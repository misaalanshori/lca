//! gh #557 - the shell analyzer split into `src/shell/` without moving a
//! single export.
//!
//! `shell.rs` was 11 lines from gate 11's ceiling and declared two out-of-line
//! modules by `#[path]` (`shell_grammar.rs`, `shell_deny_tests.rs`), so it was a
//! barrel in disguise. The split is `shell/{mod,tokens,paths,grammar,tests}.rs`,
//! where `tokens.rs` is the lexical layer, `paths.rs` holds **every containment
//! decision**, `grammar.rs` is the per-tool grammar, and `mod.rs` is the
//! decision.
//!
//! Two invariants outlive the line counts, and only these two rows guard them:
//!
//! 1. **Every public path resolves as before** - proven by *compiling* the
//!    names (`lca_permissions::shell::workspace_scoped`,
//!    `…::reviewable_alternative`), which no `pub use` churn can quietly break.
//! 2. **The containment decisions are in one file** - the security-review
//!    argument for this split. A row that greps one filename would be disarmed
//!    by the next move, so it walks the module (`crate::module_sources`, the
//!    gh #574/#543 rule) and asserts *which file* defines each of them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use lca_permissions::shell::{reviewable_alternative, workspace_scoped};

/// Where each item is defined, across the whole `shell` module.
fn defining_file(item: &str) -> Vec<String> {
    let sources = crate::module_sources("crates/lca-permissions/src/shell/mod.rs");
    // A generic item (`fn redirection_target<'a>(`) means the needle cannot
    // include the parenthesis; it also must not match a longer name that merely
    // starts the same (`workspace_scoped` vs `workspace_scoped_for_test`).
    let needle = format!("fn {item}(");
    let needle_generic = format!("fn {item}<");
    let mut found = Vec::new();
    for (path, text) in sources {
        if text.contains(&needle) || text.contains(&needle_generic) {
            found.push(path);
        }
    }
    found
}

// Verifies: gh #557 - the one public entry still answers, through the path a
// callers names. Compiling this row at all is half the assertion; the
// behaviour is the other half, so a split that kept the name and dropped the
// body is still red.
#[test]
fn every_public_shell_path_still_resolves() {
    let ws = std::path::PathBuf::from("/work/proj");
    assert!(workspace_scoped("cargo build", &ws, &ws));
    assert!(!workspace_scoped("curl https://example.com", &ws, &ws));
    // The refused shape's reviewable form (gh #458) is reachable by the same
    // path it was before the move, with the advice intact.
    let advice =
        reviewable_alternative("vim -c ':!id' f").expect("the editor -c refusal has advice");
    assert!(
        advice.contains("-S"),
        "the advice names the script-file form: {advice}"
    );
    assert!(reviewable_alternative("cargo build").is_none());
}

// Verifies: gh #557 - **every containment decision lives in one file**. The
// point of the split is that a security reviewer can read `paths.rs` and see
// all of it: what counts as a path, where a redirection points, and the
// lexical-plus-canonical answer. If one of these drifts back into `mod.rs` (or
// into `tokens.rs`), the review argument silently stops holding.
#[test]
fn every_containment_decision_lives_in_the_same_file() {
    for item in [
        "path_ok",
        "redirection_target",
        "looks_like_path",
        "canonical",
    ] {
        let files = defining_file(item);
        assert_eq!(
            files,
            vec!["crates/lca-permissions/src/shell/paths.rs".to_string()],
            "{item} must be defined in paths.rs and nowhere else, found {files:?}"
        );
    }
    // And the decisions themselves are not delegated elsewhere: `path_ok` is
    // what the analyzer consults, `tokens.rs` decides no containment at all.
    let tokens = crate::module_text("crates/lca-permissions/src/shell/tokens.rs");
    for decision in [
        "path_ok",
        "looks_like_path",
        "redirection_target",
        "canonical",
    ] {
        assert!(
            !tokens.contains(&format!("fn {decision}("))
                && !tokens.contains(&format!("fn {decision}<")),
            "the lexical layer decides nothing about containment ({decision})"
        );
    }
}

// Verifies: gh #557 - the split did not become a second implementation. The
// analyzer's entry and its per-segment classification stay in `mod.rs`, and no
// file defines a duplicate of either.
#[test]
fn the_decision_lives_in_the_module_root_exactly_once() {
    for item in ["workspace_scoped", "segment_ok"] {
        let files = defining_file(item);
        assert_eq!(
            files,
            vec!["crates/lca-permissions/src/shell/mod.rs".to_string()],
            "{item} is defined once, in mod.rs, found {files:?}"
        );
    }
    // The deny tables the decision consults moved with it, so a reviewer reads
    // the policy and its use in one place.
    let root = crate::module_text("crates/lca-permissions/src/shell/mod.rs");
    for table in [
        "DENY_PROGRAMS",
        "GIT_EGRESS",
        "SENSITIVE_VARS",
        "INTERPRETER_EVAL_FLAGS",
    ] {
        assert!(
            root.contains(&format!("const {table}")),
            "the deny table {table} stays beside the decision that consults it"
        );
    }
}
