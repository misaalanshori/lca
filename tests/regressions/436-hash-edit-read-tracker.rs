//! GitHub #436: `tool.edit_requires_read` had a hole exactly one extension wide.
//!
//! `hash_edit` writes through `Capabilities::fs_write`, which never consulted
//! the `ReadTracker` the built-in `edit` checks — so with the flag on, an
//! extension edit landed without the fresh-read guarantee the setting
//! promises.
//!
//! ## Why this is route A (tracker governs), not route B (document the exception)
//!
//! The issue's design note argued B might be correct: `hash_edit`'s anchors
//! come *from* a read, so maybe an anchor already prevents a blind edit. Three
//! reachable paths say otherwise, all measured (see
//! `extensions/hash-edit/tests/hash_edit.rs`'s residual rows and this file):
//!
//! 1. **`line_hash` is 12 bits of the trimmed line text and nothing else** — no
//!    file identity, no position, no session. An anchor the model obtained from
//!    file A resolves against file B whenever the two share that line. Proven:
//!    `an_anchor_read_from_one_file_edits_another`.
//! 2. **A stale anchor revives.** If the file is changed and then reverted, the
//!    text that produces the hash is back, so an anchor from a read hours (or
//!    sessions) ago resolves again. An anchor therefore proves "this text was
//!    in *some* read", never "the file is unchanged since my last read".
//! 3. **Duplicate lines are refused** — the one case anchors *do* catch, which
//!    is why B looked plausible.
//!
//! FR-TOOL-2's guarantee is a *recent* read, and paths 1 and 2 defeat the
//! anchor-only story. So B is false and the tracker must govern.
//!
//! Verifies: NFR-24 (gh #436), gh #436 acceptance 1, 2, 3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use hash_edit::{Edit, line_hash};

/// The refusal `edit` gives when the gate is on and the read is not fresh.
/// The extension must produce **this** string, not a near-copy: two editors
/// answering differently for the same rule is the defect this fixes.
const EDIT_REFUSAL: &str = "The file {path} changed since it was last read (or was never read this session). \
                            Read it again before editing.";

fn anchored(line: &str, new_text: &str) -> Edit {
    let hash = line_hash(line);
    Edit {
        start_hash: hash.clone(),
        end_hash: hash,
        new_text: new_text.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The evidence that route B is false
// ---------------------------------------------------------------------------

/// **Path 1.** `line_hash` sees only the trimmed line, so an anchor obtained
/// from one file resolves in another. An anchor is not a promise about *which*
/// file was read.
#[test]
fn an_anchor_read_from_one_file_edits_another() {
    let file_a = "fn main() {\n    let x = 1;\n}\n";
    let file_b = "an unrelated file someone else wrote\n    let x = 1;\nmore of it\n";
    let anchor = line_hash("    let x = 1;");
    assert!(
        file_a.contains("let x = 1;") && file_b.contains("let x = 1;"),
        "the two files share the line, and nothing else about them"
    );
    let edited = hash_edit::apply_edits(
        file_b.as_bytes(),
        &[Edit {
            start_hash: anchor.clone(),
            end_hash: anchor,
            new_text: "    let x = 999;".to_string(),
        }],
    )
    .expect("an anchor from file A edits file B: the hash carries no file identity");
    let text = String::from_utf8(edited).unwrap();
    assert!(text.contains("let x = 999;"), "{text}");
    assert!(
        text.contains("an unrelated file"),
        "and it edited the file nobody read: {text}"
    );
}

/// **Path 2.** A change-then-revert restores the hash, so an anchor from a read
/// long ago resolves again. "Some read" is not "a recent read".
#[test]
fn an_anchor_survives_a_change_and_revert() {
    let seen = "alpha\n    let x = 1;\n";
    let changed = "alpha\n    let x = 2;\n";
    let reverted = "alpha\n    let x = 1;\n";
    let anchor = line_hash("    let x = 1;");

    // While changed, the anchor refuses — the one thing anchors do catch.
    assert!(
        hash_edit::apply_edits(
            changed.as_bytes(),
            &[anchored("    let x = 1;", "    let x = 3;")]
        )
        .is_err(),
        "a changed line invalidates the anchor"
    );
    // Reverted: the same anchor is valid again, though the read is ancient.
    let edited = hash_edit::apply_edits(
        reverted.as_bytes(),
        &[Edit {
            start_hash: anchor.clone(),
            end_hash: anchor,
            new_text: "    let x = 3;".to_string(),
        }],
    )
    .expect("a revert restores the hash, so a stale anchor resolves");
    assert!(String::from_utf8(edited).unwrap().contains("let x = 3;"));
    let _ = seen;
}

// ---------------------------------------------------------------------------
// Acceptance 1: the tracker governs, with edit's own message
// ---------------------------------------------------------------------------

/// A `hash_edit` on a file this session never read is refused when the gate is
/// on — with `edit`'s exact words. Equality, not "contains": the two editors
/// must answer identically or the user learns two messages for one rule.
#[test]
fn an_unread_file_is_refused_with_the_built_in_edits_message() {
    let dir = std::env::temp_dir().join(format!("lca-436-unread-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("never_read.txt");
    std::fs::write(&path, "alpha\n    let x = 1;\n").unwrap();

    let refused = hash_edit::checked_edit(
        &dir,
        "never_read.txt",
        &[anchored("    let x = 1;", "    let x = 2;")],
        true,
    )
    .expect_err("gate on, nothing read: refused");
    assert_eq!(
        refused,
        EDIT_REFUSAL.replace("{path}", "never_read.txt"),
        "the built-in edit's message, verbatim"
    );
    // And the file is untouched.
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "alpha\n    let x = 1;\n",
        "a refused edit writes nothing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// With the gate **off** (pi parity, the product default), a blind edit lands —
/// exactly as the built-in `edit` behaves.
#[test]
fn a_blind_edit_lands_when_the_gate_is_off() {
    let dir = std::env::temp_dir().join(format!("lca-436-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("unread.txt");
    std::fs::write(&path, "alpha\n    let x = 1;\n").unwrap();

    hash_edit::checked_edit(
        &dir,
        "unread.txt",
        &[anchored("    let x = 1;", "    let x = 5;")],
        false,
    )
    .expect("gate off: a blind anchored edit is allowed");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "alpha\n    let x = 5;\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
