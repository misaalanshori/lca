//! Gh #395 gap 1: threaded forks in `/resume`.
//!
//! `/resume` was a flat chronological list. A project with a fork family - the
//! ordinary case after a few `/fork`s - showed the parent and its children as
//! unrelated rows, so "which of these came from that one" was invisible and the
//! only ordering was last-write time. The picker becomes a tree.
//!
//! # The linkage is not pi's field name
//!
//! The issue names pi's `parentSessionPath`. ADR-0046's design records a fork very
//! differently: the child is its own directory, and the parent is named by a
//! `fork-point` **record** in the child's log
//! (`Record::ForkPoint { parent_session, record_id, .. }`). The store already
//! reads that record into `SessionSummary::parent_session` when it rebuilds the
//! index, and `Store::ancestors` already refuses a cycle with `Error::ForkCycle`.
//!
//! So the linkage exists and needs no storage change - it simply never reached the
//! picker. This module builds the tree from `parent_session`.
//!
//! # The four properties worth pinning
//!
//! 1. **Subtree order is recursive latest activity.** A parent with a busy fork
//!    sorts by the fork's activity, not its own last write. That is the point of a
//!    tree: the thing you are looking for may be a child.
//! 2. **Connectors are right at every depth** - `└─` for the last child, `├─`
//!    otherwise, vertical bars only where a sibling follows. Off-by-one here is the
//!    classic tree bug and it is what makes a tree unreadable.
//! 3. **A query flattens.** A filtered tree spends rows on ancestors that did not
//!    match, so an active search collapses to the scored list - and clearing it
//!    brings the tree back.
//! 4. **A tree must never be worse than a list.** A session whose parent is gone is
//!    a *root*, not a hidden orphan; and a malformed self/cycle linkage is detected,
//!    not looped - a picker that hangs is worse than a flat list.
//!
//! Verifies: gh #395 gap 1, FR-SESS-3, NFR-24.

use crate::resume::{ResumePicker, SessionEntry};

/// One entry, with the fields the tree needs.
fn entry(id: &str, title: &str, modified_ms: u64, parent: Option<&str>) -> SessionEntry {
    SessionEntry {
        id: id.to_string(),
        title: title.to_string(),
        messages: 1,
        age: "now".to_string(),
        labels: Vec::new(),
        modified_ms,
        parent: parent.map(str::to_string),
    }
}

/// The three-session family the study's screenshots show: a root with two forks.
fn family() -> Vec<SessionEntry> {
    vec![
        entry("root", "Fix the parser", 1_000, None),
        entry("fork-a", "Try the other approach", 2_000, Some("root")),
        entry("fork-b", "Docs pass", 3_000, Some("root")),
    ]
}

/// The rendered rows for a picker, as the painter produces them (the connector
/// column included), so the rows assert what a reader sees.
fn tree_rows(picker: &ResumePicker) -> Vec<String> {
    picker.paint_rows()
}

// ---------------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------------

// Verifies: gh #395 gap 1 - **a fork family renders as a tree**: the parent on its
// own row and its children under it with connectors. This is the row that is red
// against the flat list, and it is the whole defect: two forks read as two
// unrelated sessions.
#[test]
fn a_fork_family_renders_as_a_tree() {
    let picker = ResumePicker::new(family());
    let rows = tree_rows(&picker);
    assert_eq!(rows.len(), 3, "three sessions, three rows: {rows:?}");
    assert!(
        rows[0].starts_with("Fix the parser"),
        "the parent is the block's head: {:?}",
        rows[0]
    );
    // Children order by *their own* activity, newest first: `Docs pass` at 3000
    // comes before `Try the other approach` at 2000. That is the tree working - the
    // ordering inside a family is activity, not insertion order.
    assert!(
        rows[1].starts_with("├─ Docs pass"),
        "the fresher child leads, and is not the last so it gets ├─: {:?}",
        rows[1]
    );
    assert!(
        rows[2].starts_with("└─ Try the other approach"),
        "the last child gets └─: {:?}",
        rows[2]
    );
}

// Verifies: gh #395 gap 1 - **a child is indented under its parent**, and the
// chronological order inside a family is by activity, not by the parent's own
// timestamp. `family()`'s parent has the *oldest* timestamp, so a flat newest-first
// list would have put it last; the tree puts it first because its subtree is the
// freshest thing in the project.
#[test]
fn the_tree_orders_families_by_their_subtree_activity() {
    let picker = ResumePicker::new(family());
    assert_eq!(
        picker.matches,
        vec![0, 2, 1],
        "the family is one block, its children in activity order (fork-b 3000, fork-a 2000)"
    );
    // A second, unrelated older session sorts after the whole family even though
    // it is newer than the family's root.
    let mut entries = family();
    entries.push(entry("old", "Ancient history", 500, None));
    let picker = ResumePicker::new(entries);
    let rows = tree_rows(&picker);
    assert_eq!(
        rows.last().map(String::as_str),
        Some("Ancient history"),
        "the quiet session sinks below the family: {rows:?}"
    );
}

// Verifies: gh #395 gap 1 - **recursive** activity: a busy grandchild lifts the
// whole chain. A parent sorting by its own last write would sink a family whose
// work happened one level deeper, which is exactly where a user's recent work is
// after forking twice.
#[test]
fn a_busy_grandchild_lifts_its_whole_chain() {
    let entries = vec![
        entry("quiet-root", "Quiet root", 1_000, None),
        entry("loud", "Loud unrelated", 2_000, None),
        entry("child", "Child", 1_500, Some("quiet-root")),
        entry("grandchild", "Grandchild", 9_000, Some("child")),
    ];
    let picker = ResumePicker::new(entries);
    let rows = tree_rows(&picker);
    assert_eq!(rows[0], "Quiet root", "the root heads its block: {rows:?}");
    assert_eq!(
        rows[1], "└─ Child",
        "the single child is also the last: {rows:?}"
    );
    assert_eq!(
        rows[2], "   └─ Grandchild",
        "and the grandchild indents one more level: {rows:?}"
    );
    assert_eq!(
        rows[3], "Loud unrelated",
        "the unrelated session sorts by its own activity, below the lifted family"
    );
}

// Verifies: gh #395 design point 3 - **connectors are correct at every depth**.
// The vertical bar exists only where a sibling follows, so a nested block under a
// last child draws spaces where an earlier sibling's block draws `│`. That is the
// off-by-one that makes a tree unreadable, so it is asserted explicitly.
#[test]
fn connectors_are_correct_through_three_levels() {
    // Activity is chosen so the *shape* is what the connectors are tested against:
    // `b` (4000) sorts before `a` (2000), and `b`'s own children sort above it
    // in the sibling list only by their own times, which are all higher.
    let entries = vec![
        entry("root", "Root", 1_000, None),
        entry("a", "First child", 2_000, Some("root")),
        entry("a1", "First child's only child", 3_000, Some("a")),
        entry("b", "Last child", 4_000, Some("root")),
        entry("b1", "Last child's first", 5_000, Some("b")),
        entry("b2", "Last child's last", 6_000, Some("b")),
    ];
    let picker = ResumePicker::new(entries);
    let rows = tree_rows(&picker);
    assert_eq!(rows[0], "Root");
    // Siblings order by subtree activity, so `b` (4000) leads and `a` (2000) is
    // the last child.
    // `b`'s children sort by activity too, so `b2` (6000) leads.
    assert_eq!(rows[1], "├─ Last child");
    assert_eq!(rows[2], "│  ├─ Last child's last");
    assert_eq!(rows[3], "│  └─ Last child's first");
    assert_eq!(rows[4], "└─ First child");
    // `a` is the root's LAST child, so the column under it is blank.
    assert_eq!(rows[5], "   └─ First child's only child");
}

// Verifies: gh #395 design point 4 - **a query flattens**, and clearing it brings
// the tree back. A filtered tree spends its rows on ancestors that did not match,
// so a search is a scored list; the transition is pinned both ways because a
// one-way flatten would silently lose the tree for the rest of the session.
#[test]
fn a_query_flattens_the_tree_and_clearing_it_restores_it() {
    let mut picker = ResumePicker::new(family());
    picker.query = "docs".to_string();
    picker.refilter();
    let rows = tree_rows(&picker);
    assert_eq!(
        rows,
        vec!["Docs pass"],
        "the match is a flat row, with no ancestor rows and no connector: {rows:?}"
    );
    picker.query.clear();
    picker.refilter();
    let rows = tree_rows(&picker);
    assert_eq!(
        rows,
        vec![
            "Fix the parser",
            "├─ Docs pass",
            "└─ Try the other approach"
        ],
        "clearing the query restores the tree: {rows:?}"
    );
}

// Verifies: gh #395 design point 5 - **a session whose parent is gone is a root**,
// not a hidden orphan. A deleted parent must not make its children vanish: they
// are still resumable sessions, and hiding them would lose work with no error.
#[test]
fn a_session_whose_parent_is_gone_is_a_root() {
    let entries = vec![
        entry("live", "Live root", 1_000, None),
        entry("orphan", "Orphaned fork", 2_000, Some("deleted-parent")),
    ];
    let picker = ResumePicker::new(entries);
    let rows = tree_rows(&picker);
    assert_eq!(rows.len(), 2, "both sessions get rows: {rows:?}");
    assert_eq!(
        rows[0], "Orphaned fork",
        "the orphan is a top-level block, ordered by its own activity"
    );
    assert_eq!(rows[1], "Live root");
}

// Verifies: gh #395 design point 6 - **a malformed linkage does not hang**. A
// session that names itself as its parent, or two sessions that name each other,
// must be detected: a picker that loops forever is worse than the flat list it
// replaced. Bounded by the test harness's own timeout as well as the assertion.
#[test]
fn a_self_parent_and_a_two_cycle_do_not_hang() {
    let entries = vec![
        entry("selfish", "Names itself", 1_000, Some("selfish")),
        entry("ping", "Ping", 2_000, Some("pong")),
        entry("pong", "Pong", 3_000, Some("ping")),
        entry("fine", "Unrelated", 500, None),
    ];
    let picker = ResumePicker::new(entries);
    let rows = tree_rows(&picker);
    assert_eq!(
        rows.len(),
        4,
        "every session still gets exactly one row - a cycle is broken, not looped: {rows:?}"
    );
    // Each row appears once; no infinite expansion.
    for id in ["Names itself", "Ping", "Pong", "Unrelated"] {
        assert_eq!(
            rows.iter().filter(|row| row.contains(id)).count(),
            1,
            "`{id}` appears exactly once: {rows:?}"
        );
    }
}

// Verifies: gh #395 test policy - **sorting is stable for equal activity**. Two
// sessions that last changed at the same millisecond must keep a deterministic
// order, or the list reshuffles between opens for no reason a reader can see.
#[test]
fn equal_activity_sorts_deterministically() {
    let entries = vec![
        entry("b", "Bravo", 1_000, None),
        entry("a", "Alpha", 1_000, None),
    ];
    let first = tree_rows(&ResumePicker::new(entries.clone()));
    for _ in 0..5 {
        assert_eq!(
            tree_rows(&ResumePicker::new(entries.clone())),
            first,
            "the same input gives the same order every time"
        );
    }
}

// Verifies: gh #395 test policy - **the existing grammar still works**, including
// through the flatten. The tree changes ordering; it must not change what matches.
#[test]
fn the_search_grammar_still_matches_through_the_flatten() {
    let mut picker = ResumePicker::new(family());
    picker.query = "re:/^try/".to_string();
    picker.refilter();
    assert_eq!(tree_rows(&picker), vec!["Try the other approach"]);
    picker.query = "\"docs pass\"".to_string();
    picker.refilter();
    assert_eq!(tree_rows(&picker), vec!["Docs pass"]);
    picker.query = "re:(".to_string();
    picker.refilter();
    assert!(
        tree_rows(&picker).is_empty() || tree_rows(&picker) == vec!["(no matches)"],
        "an invalid regex matches nothing and does not abort: {:?}",
        tree_rows(&picker)
    );
}

// Verifies: gh #395 - **the highlight follows the tree row, not the entry index**.
// The picker's `selected` is an index into `matches`, and the tree changes the
// order of `matches`; a painter that indexed `entries` by `selected` would
// highlight whatever happened to sit there.
#[test]
fn the_selected_row_is_the_one_the_cursor_names() {
    let mut picker = ResumePicker::new(family());
    picker.selected = 1;
    assert_eq!(
        picker.selected_entry().map(|entry| entry.id.as_str()),
        Some("fork-b"),
        "the second tree row is the fresher child, `fork-b`"
    );
    let rows = tree_rows(&picker);
    assert_eq!(rows.len(), 3);
}
