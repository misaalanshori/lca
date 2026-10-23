//! NFR-24 guard for gh #395 gap 1: threaded forks in `/resume`.
//!
//! # The linkage, named
//!
//! The issue names pi's `parentSessionPath`. **This design has no such field.**
//! ADR-0046 records a fork as a child session *directory* whose log opens with a
//! `fork-point` record:
//!
//! ```text
//! Record::ForkPoint { v, ts, id, parent_session, record_id }
//! ```
//!
//! `Store::rebuild_index` reads that record into
//! `SessionSummary::parent_session`, and `Store::ancestors` already refuses a
//! malformed chain with `Error::ForkCycle`. So the linkage existed all along and
//! needed **no storage change** - it simply never reached the picker, whose
//! `SessionEntry` carried a title, a count and an age and nothing about lineage.
//!
//! That is the finding worth guarding: the next person to look at this should
//! start from `parent_session`, not go looking for a pi-shaped field that does not
//! exist and conclude the data is missing.
//!
//! # What this file checks, and what lives beside the code
//!
//! The behaviour rows are in `crates/lca-ui/src/chat_resume_tree_tests.rs`, because
//! they need the picker's private tree. This file guards the properties that are
//! about the *shape of the code*, which a behaviour row cannot see:
//!
//! 1. **The linkage is the store's `parent_session`**, surfaced read-only - not a
//!    new field, not pi's name.
//! 2. **The tree is built once per list change, not per frame** (`#389`'s lesson:
//!    no render path does work that scales with the session count).
//! 3. **A query is what flattens**, and the flatten and the un-flatten are one
//!    function - a one-way flatten would silently lose the tree.
//!
//! Verifies: gh #395 gap 1, FR-SESS-3, NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {path:?}: {error}"))
}

/// Strip `//`-prefixed comment lines so a rule named in prose cannot satisfy a
/// guard meant to check code.
fn code_lines(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: gh #395 - **the linkage is the store's `parent_session`**, carried
// read-only into the picker. Not a new record field, not pi's `parentSessionPath`.
//
// This is the row that answers the brief's stop condition: the record carries
// enough to identify the parent, so no storage change is warranted and the work is
// a picker fix.
#[test]
fn the_fork_linkage_is_the_stores_parent_session() {
    // The record the linkage actually lives in (ADR-0046).
    let record = read("crates/lca-protocol/src/record.rs");
    assert!(
        record.contains("ForkPoint {") && record.contains("parent_session: String"),
        "the fork-point record names the parent session"
    );

    // The store reads it into the summary the listing uses.
    let store = code_lines(&read("crates/lca-session/src/store.rs"));
    assert!(
        store.contains("parent_session: meta.parent_session"),
        "`rebuild_index` carries the parent into `SessionSummary`"
    );

    // The picker entry exposes it - and it is the store's field, not a pi-shaped
    // one invented for the picker.
    let resume = code_lines(&read("crates/lca-ui/src/resume.rs"));
    assert!(
        resume.contains("pub parent: Option<String>"),
        "the picker entry carries the parent"
    );
    for pi_name in ["parentSessionPath", "parent_session_path"] {
        assert!(
            !code_lines(&read("crates/lca-ui/src/resume.rs")).contains(pi_name),
            "no pi-shaped field `{pi_name}` is invented"
        );
    }

    // And the data source wires the summary through unchanged.
    let hooks = code_lines(&read("crates/lca-cli/src/tui/session_hooks.rs"));
    assert!(
        hooks.contains("parent: summary.parent_session"),
        "the picker's rows are built from the summary's own linkage"
    );
}

// Verifies: gh #395 design point 7 - **the tree is computed once per list change**.
//
// The build walks every entry's ancestry, so doing it in the painter would make
// the cost of opening a frame scale with the number of sessions. `#389` closed
// exactly that shape once already (a per-frame filesystem read in the footer), so
// the guard is that `paint_rows` is a pure read of a field the picker already has.
#[test]
fn the_tree_is_built_once_not_per_frame() {
    let resume = code_lines(&read("crates/lca-ui/src/resume.rs"));
    let painter = resume
        .split("pub fn paint_rows(")
        .nth(1)
        .and_then(|rest| rest.split("pub fn refilter(").next())
        .expect("paint_rows is present");
    assert!(
        !painter.contains("build_tree("),
        "the painter never builds the tree - it reads the one the picker holds:\n{painter}"
    );
    assert!(painter.contains("self.tree"), "it reads the cached tree");

    // The build happens where the list changes.
    let rebuild = resume
        .split("fn rebuild(&mut self)")
        .nth(1)
        .and_then(|rest| rest.split("pub fn paint_rows(").next())
        .expect("rebuild is present");
    assert!(
        rebuild.contains("self.tree = build_tree(&self.entries)"),
        "and it happens exactly once, in the one function that re-derives the view:\n{rebuild}"
    );
}

// Verifies: gh #395 design point 4 - **a query flattens and clearing it restores
// the tree**, through one function.
//
// A one-way flatten would silently lose the tree for the rest of the session - the
// reader clears their search and the hierarchy never comes back, with no error.
// Both directions and the same entry point is what prevents it.
#[test]
fn one_function_owns_the_flatten_and_the_restore() {
    let resume = code_lines(&read("crates/lca-ui/src/resume.rs"));
    let rebuild = resume
        .split("fn rebuild(&mut self)")
        .nth(1)
        .and_then(|rest| rest.split("pub fn paint_rows(").next())
        .expect("rebuild is present");
    assert!(
        rebuild.contains("build_tree(") && rebuild.contains("filter_sessions("),
        "the query-empty branch builds the tree and the other filters:\n{rebuild}"
    );
    // `refilter` - the only path a keystroke takes - goes through it.
    let refilter = resume
        .split("pub fn refilter(&mut self)")
        .nth(1)
        .expect("refilter is present");
    assert!(
        refilter.contains("self.rebuild()"),
        "every keystroke re-derives through the same function:\n{refilter}"
    );
}

// Verifies: gh #395 design point 6 - **a cycle is detected, not looped**.
//
// Source-level because the failure mode is a hang: a behaviour row that passes
// today would keep passing while a later edit reintroduced recursion without the
// visited set, and the symptom would be a frozen picker rather than a red test.
#[test]
fn the_tree_build_guards_against_a_cycle() {
    let resume = code_lines(&read("crates/lca-ui/src/resume.rs"));
    let build = resume
        .split("fn build_tree(")
        .nth(1)
        .and_then(|rest| rest.split("fn connector(").next())
        .expect("build_tree is present");
    assert!(
        build.contains("is_child") && build.contains("retain("),
        "a cycle edge is cut rather than followed:\n{build}"
    );
    assert!(
        build.contains("visited"),
        "and every session is emitted at most once:\n{build}"
    );
    // The dereference must be iterative or guarded: a recursive descent into a
    // malformed chain can overflow before the visit set helps.
    assert!(
        build.contains("while let Some("),
        "the walk is an explicit stack, not unbounded recursion:\n{build}"
    );
    // A self-parent is the degenerate case and is excluded before the walk.
    assert!(
        build.contains("parent == entry.id"),
        "a session naming itself as its parent is treated as a root:\n{build}"
    );
}
