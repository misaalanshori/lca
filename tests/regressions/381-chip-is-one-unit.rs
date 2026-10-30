//! Gh #381 gaps 1-2: a paste chip is **one unit** for every motion and delete path,
//! so the orphaned-marker corruption is unreachable.
//!
//! The predicate already existed - `engine::word::atomic_span_at`, added by gh #383
//! for word motion. What did not consult it: plain `Left`/`Right`, `Up`/`Down`, the
//! line wrapper, and `Backspace`. The consequences the issue records:
//!
//! - `Left` stepped **21 keystrokes** through `[paste #1 +123 lines]`;
//! - `Alt+Left/Right` were already fine (that is where the predicate came from);
//! - `Up`/`Down` could land **inside** a chip;
//! - a wrap split `paste`/`#1` across rows;
//! - one `Backspace` on `]` left `[paste #1 +123 lines`, which no longer matches the
//!   marker pattern - so `expand_paste_markers` passed it to the **model literally**.
//!
//! The behavioural rows live in `crates/lca-tui/src/widgets/editor_atomic_tests.rs`.
//! This file is the NFR-24 guard, and it pins the seam rather than the rows: **one
//! predicate, every motion path asks it**. A fix that repaired the orphan instead of
//! preventing it, or that grew a second notion of "unit", passes the behavioural rows
//! and fails here.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

/// Source with `//` comment lines removed, so a row about *code* is neither
/// satisfied nor defeated by prose.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of one function, found by brace matching from its signature.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    return &source[start..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("{signature} has a balanced body");
}

// Verifies: gh #381 gap 1 - every motion path consults the one predicate.
//
// The design rule from the brief: "one predicate, every motion path asks it". The
// predicate is `engine::word::atomic_span_at` (gh #383); the editor reaches it
// through exactly one helper, `chip_span`. So the guard is that `cursor_left`,
// `cursor_right`, `backspace`, `delete_forward` and `move_to_visual_row` all route
// through `chip_span`-derived helpers, and that `editor.rs` contains no second
// marker recogniser.
#[test]
fn every_motion_path_consults_the_one_atomic_predicate() {
    // The chip helpers were split into `editor_atomic.rs` at the 1,200-line ceiling;
    // the guard reads the whole editor surface rather than one file, so the split
    // does not weaken it.
    let source = format!(
        "{}\n{}",
        code_only(&read("crates/lca-tui/src/widgets/editor.rs")),
        code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"))
    );

    // One recogniser, in `word.rs`. The editor must not re-implement it.
    assert!(
        !source.contains("[paste #\"]"),
        "the editor does not spell the marker shape itself"
    );
    let predicate_calls = source.matches("engine::word::atomic_span_at").count()
        + source
            .matches("engine::word::atomic_start_ending_at")
            .count();
    assert!(
        predicate_calls >= 2,
        "the editor reaches the shared predicate: {predicate_calls} call sites"
    );

    // Each motion/delete path routes through a chip-aware helper.
    for (signature, helper) in [
        ("pub fn cursor_left(", "atomic_boundary_left"),
        ("pub fn cursor_right(", "atomic_boundary_right"),
        ("pub fn backspace(", "chip_span"),
        ("pub fn delete_forward(", "chip_span"),
        ("fn move_to_visual_row(", "snap_out_of_chip"),
    ] {
        let body = function_body(&source, signature);
        assert!(
            body.contains(helper),
            "{signature} asks `{helper}`, which is the chip-aware path: {body}"
        );
    }

    // And each helper's chip branch must actually *do* something: a mutation that
    // keeps the call but makes the chip arm fall through to the cluster path leaves
    // the row above green (it did, before this assertion). The two boundary helpers
    // are the ones the defect lived in, so their `Some` arms are pinned literally.
    let left = function_body(&source, "fn atomic_boundary_left(");
    assert!(
        left.contains("Some((start, _)) => start,"),
        "left returns the chip's start on a chip: {left}"
    );
    let right = function_body(&source, "fn atomic_boundary_right(");
    assert!(
        right.contains("Some((start, end)) if col == start => end,"),
        "right returns the chip's end on a chip: {right}"
    );
    assert!(
        left.contains("None => Self::prev_cluster_boundary")
            && right.contains("None => Self::next_cluster_boundary"),
        "and the cluster snap is only the non-chip fallback"
    );
}

// Verifies: gh #381 gap 2 - the delete removes the **span**, not "a character that
// leaves invalidity".
//
// The brief's distinction is what makes the orphan unreachable rather than
// repaired: a repair would inspect the text after the fact and fix a bad shape, and
// would still ship if a future path forgot. The guard is that the removed range is
// the chip's own `(start, end)`.
#[test]
fn the_delete_removes_the_chip_span_rather_than_repairing_text() {
    let source = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    let backspace = function_body(&source, "pub fn backspace(");
    assert!(
        backspace.contains("Some((start, end)) => (start, end)"),
        "backspace removes the chip's whole span: {backspace}"
    );
    assert!(
        !backspace.contains("if !self.text().contains(\"[paste")
            && !backspace.contains("trim_end_matches"),
        "it does not post-hoc repair the text: {backspace}"
    );
    let forward = function_body(&source, "pub fn delete_forward(");
    assert!(
        forward.contains("Some((start, end)) => (start, end)"),
        "delete_forward removes the whole span too: {forward}"
    );
}

// Verifies: gh #381 gap 2 - renumbering exists, and it moves labels rather than
// payloads.
//
// `insert_paste` derives the id from `pastes.len() + 1`, so after a delete the
// surviving chip's label and its payload index diverge - and the next paste takes a
// colliding id. `renumber_pastes` rebuilds `pastes` in document order, so each
// marker's payload is re-registered under its new position: a renumber can change a
// label but never what it expands to.
#[test]
fn the_renumber_rebinds_payloads_in_document_order() {
    let atomic = code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"));
    let body = function_body(&atomic, "fn renumber_pastes(");
    assert!(
        body.contains("payloads.push(self.pastes[id - 1].clone())"),
        "each marker's payload is carried over as the label is rewritten: {body}"
    );
    assert!(
        body.contains("self.pastes = payloads"),
        "and `pastes` is replaced by that document-ordered list: {body}"
    );
    // Both delete paths call it.
    let source = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    for signature in ["pub fn backspace(", "pub fn delete_forward("] {
        let body = function_body(&source, signature);
        assert!(
            body.contains("renumber_pastes()"),
            "{signature} renumbers after removing a chip: {body}"
        );
    }
}

// Verifies: gh #381 gap 1 - the wrapper treats a chip as unbreakable.
//
// The same requirement as "do not split a wide glyph", and the reason `paste`/`#1`
// broke across rows. One mechanism only: two guards for one property means either
// can mask the other (my first attempt had two, and the mutation that removed one
// passed - a guard that cannot fail proves nothing).
#[test]
fn the_wrapper_treats_a_chip_as_unbreakable() {
    let source = code_only(&read("crates/lca-tui/src/widgets/editor_rows.rs"));
    let body = function_body(&source, "pub fn wrap_row(");
    assert!(
        body.contains("engine::word::atomic_span_at"),
        "the wrapper asks the same predicate the editor does: {body}"
    );
    assert!(
        body.contains("chip_end"),
        "and carries a chip's span so its inner graphemes cannot break: {body}"
    );
    // The chip must actually start a fresh row when it would overflow, or it is
    // simply allowed to split.
    assert!(
        body.contains(
            "if current_width > 0 && current_width + chip_width > width && chunk_start < *start {"
        ),
        "and moves to a new row when the chip would overflow: {body}"
    );
    // And the whole mechanism must be reachable: a guard that cannot fail proves
    // nothing, so a condition that has been neutered (`&& false` / `|| true`) is
    // rejected outright. This is what my first version of this row missed - it
    // asserted shapes that a `&& false` mutation left intact.
    for neutered in ["&& false", "|| true"] {
        assert!(
            !body.contains(neutered),
            "the unbreakable-chip path is not disabled ({neutered}): {body}"
        );
    }
}

// Verifies: gh #381 gap 2 - the model never receives marker text.
//
// The user-visible harm, and the reason this is the row that matters: a marker is a
// display stand-in, so any marker text reaching `expand_paste_markers` *unexpanded*
// means the model read a placeholder instead of the content. The behavioural rows
// drive real deletes; this guard pins the declaration they rely on.
#[test]
fn expansion_is_the_only_way_a_marker_leaves_the_editor() {
    // `expand_paste_markers` moved with the chip helpers; read both.
    let source = format!(
        "{}\n{}",
        code_only(&read("crates/lca-tui/src/widgets/editor.rs")),
        code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"))
    );
    // `expand_paste_markers` is the only public exit that reads the buffer for a
    // submission, and it is called from the submit paths.
    assert!(
        source.contains("expand_paste_markers(&self.text(), &self.pastes)"),
        "the submission path expands markers: {source}"
    );
    // An unrecognised marker is left literal *by design* (a user may type the
    // shape); that is why the editor must never produce an unrecognised one.
    let expand = function_source(&source, "fn expand_paste_markers(");
    assert!(
        expand.contains("out.push('[')"),
        "a hand-typed marker staying literal is deliberate: {expand}"
    );
}

/// The whole text of a top-level function (from its signature to the line before the
/// next top-level item), for functions that are not methods.
fn function_source<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n}\n").map(|at| at + 2).unwrap_or(rest.len());
    &rest[..end]
}
