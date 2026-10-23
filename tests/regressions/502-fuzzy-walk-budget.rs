//! NFR-24 guard for gh #502: the `@`-file fuzzy walk must reach the file a query
//! names, even when a wide shallow directory sits beside it.
//!
//! The defect was a budget counting the wrong thing. `collect_files` stopped at
//! `depth > 6 || out.len() > 200`, where `out` holds **matches** - so a wide
//! directory at the root spent the entire budget on scattered-letter hits and the
//! walk never descended to the nested file. The issue's repro is exact: a tree of
//! `src/main.rs` + `README.md` + a populated `~/.lca/private/` answered `@mai`
//! with `mcp`, `openai-compatible`, `compaction-default` and never `main.rs`,
//! while bare `@` *did* list it.
//!
//! So the matcher was right and the **walk order** was wrong, which is the
//! diagnosis that decides the fix: bound the work, not the discovery. Re-tuning
//! the scorer at the same time would have made the result unattributable.
//!
//! # What is pinned, and where the rest lives
//!
//! The behaviour rows live beside the code they exercise, in
//! `autocomplete.rs`'s own test module - they need the private `provider()` and
//! the private budget constants, so a copy in another crate would test a copy.
//! This file guards the two properties that are about the *build* rather than one
//! call:
//!
//! 1. **The walk is bounded** - a row cannot see "did it stop", because a stop
//!    and a finish look the same from the result. So the constants are asserted
//!    to exist and to be finite, and the file is asserted to spend them.
//! 2. **The walk is breadth-first and sorted** - the two properties that make the
//!    fix deterministic. A future edit that went back to depth-first recursion
//!    would keep every behaviour row green on the issue's tree while restoring
//!    the intermittent cross-machine bug the issue describes.
//!
//! Verifies: gh #502, FR-UI-1's `@` completion contract, and NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The autocomplete source, so the walk's shape can be read.
fn autocomplete_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-tui/src/widgets/autocomplete.rs"
    ))
    .expect("autocomplete.rs is readable")
}

/// One function's body, from its signature to the next top-level `fn` or the
/// closing brace at column zero.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("the function `{signature}` is present"));
    let rest = &source[start..];
    match rest.find("\n}\n") {
        Some(end) => &rest[..end],
        None => rest,
    }
}

// Verifies: gh #502's diagnosis - **the cap bounds work, not matches**. The old
// budget was `out.len() > 200`, where `out` held matches; that is what let a wide
// shallow directory starve a deep one. The body must no longer consult the match
// count, and must spend a budget of its own.
//
// Asserted on the source because "did the walk stop early" is invisible from the
// result - a starved walk and a completed walk both return a `Suggestions`.
#[test]
fn the_walk_budget_does_not_count_matches() {
    let source = autocomplete_source();
    let body = function_body(&source, "fn collect_files(");

    assert!(
        !body.contains("out.len() >"),
        "the budget no longer stops on how many *matches* were found - that is \
         gh #502's exact defect:\n{body}"
    );
    assert!(
        body.contains("WALK_ENTRY_BUDGET") && body.contains("WALK_DIR_BUDGET"),
        "and it spends explicit visit budgets instead:\n{body}"
    );
    // A budget that is checked must also be incremented, or it never binds.
    assert!(
        body.contains("budget.entries +=") && body.contains("budget.dirs +="),
        "the budgets are incremented as the walk proceeds:\n{body}"
    );
}

// Verifies: gh #502 design point 4 - **the bound is still a bound**. The fix
// moved the budget rather than removing it; a walk with no cap at all would be
// worse than the bug, because these run at keystroke time.
#[test]
fn the_walk_is_still_bounded() {
    let source = autocomplete_source();
    for constant in ["WALK_ENTRY_BUDGET", "WALK_DIR_BUDGET", "WALK_PER_DIR_CAP"] {
        let declaration = source
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("const {constant}")))
            .unwrap_or_else(|| panic!("`{constant}` is declared"));
        assert!(
            declaration.contains("usize"),
            "`{constant}` is a finite count: {declaration}"
        );
    }
    let body = function_body(&source, "fn collect_files(");
    assert!(
        body.contains("WALK_PER_DIR_CAP"),
        "a single enormous directory cannot spend the whole budget:\n{body}"
    );
}

// Verifies: gh #502 design point 5 - **the walk is breadth-first and sorted**.
// These are the two properties that make the fix deterministic across machines;
// the old walk was depth-first in `read_dir` order, which is why the bug was
// intermittent. A future edit that restored recursion would keep every
// behaviour row green on the issue's tree and reintroduce the flakiness.
#[test]
fn the_walk_is_breadth_first_and_sorted() {
    let source = autocomplete_source();
    let body = function_body(&source, "fn collect_files(");

    assert!(
        body.contains("VecDeque"),
        "the walk uses a queue, so a level is finished before it descends:\n{body}"
    );
    assert!(
        body.contains("pop_front"),
        "and takes from the front (depth-first would be `pop`):\n{body}"
    );
    assert!(
        body.contains("push_back"),
        "while appending children to the back:\n{body}"
    );
    assert!(
        body.contains("names.sort()"),
        "each level's entries are sorted, so the budget is spent in a stable \
         order rather than `read_dir`'s:\n{body}"
    );
    assert!(
        !body.contains("collect_files(root, &path"),
        "the recursion is gone - that is what made order filesystem-dependent:\n{body}"
    );
}

// Verifies: gh #502's walk root - **the walk is rooted at the workspace**, so the
// data home only appears when it is genuinely inside the workspace.
//
// The issue's repro has `~/.lca/private/` as a sibling *under a scratch HOME*, so
// the entries appeared because the scratch project sat at that HOME's root. This
// row records what the root is, so a future reader does not have to rediscover
// it: `options.workspace`, never `$HOME` and never the process cwd.
#[test]
fn the_walk_is_rooted_at_the_workspace() {
    // The construction site moved to its own module when `chat_commands.rs`
    // reached the 1,200-line ceiling (gh #408); the property is unchanged, so this
    // follows it rather than pinning a file path that a split can silently orphan.
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/autocomplete_provider.rs"
    ))
    .expect("the provider's construction site is readable");
    assert!(
        source.contains("CombinedAutocompleteProvider::new(commands, options.workspace.clone())"),
        "the provider is built with the session's workspace, not a home directory"
    );
}
