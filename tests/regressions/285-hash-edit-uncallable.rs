//! GitHub #285: `hash_edit` was uncallable, and its writes skipped the
//! read-before-edit gate.
//!
//! Two defects, one tool.
//!
//! **1. No producer for the hashes the consumer demands.** `hash_edit` resolves
//! line ranges by 3-character FNV-1a anchors (`start_hash`/`end_hash`). The
//! only function that produces them, `annotate`, had **zero callers outside the
//! crate's tests**: the built-in `read` emits decimal line numbers, and this
//! extension exported no `tool_result` hook and no `hash_read` tool. So the
//! model could never *see* an anchor, and a tool whose arguments cannot be
//! constructed is a tool that cannot be used. It shipped, was listed, and did
//! nothing.
//!
//! **2. The write bypassed the read tracker.** The built-in `edit` refuses to
//! write a file this session has not read (`ToolExecutor`'s `ReadTracker`,
//! gated by `tool.edit_requires_read`). `run_hash_edit` writes through
//! `Capabilities::fs_write` directly, so that gate never applies to it.
//!
//! ## What the fix is, and what it honestly is not
//!
//! Defect 1 is fixed by the seam the authoring guide already documents: the
//! extension now exports the `hooks-tool-result` world, and its handler
//! annotates a successful built-in `read` with a hashed gutter — so the model
//! sees anchors on the read it was already going to perform. That is the whole
//! point of the extension, and it needs no new tool, capability, or ABI change.
//!
//! Defect 2 is **not** fixed by this crate, and the rows below say so rather
//! than implying otherwise. The tracker is private to `ToolExecutor`
//! (`crates/lca-tools/src/lib.rs`: `struct ReadTracker`, a field, no accessor),
//! `ToolExecutor` is never handed to an extension, and the only reachable
//! delivery mode for this extension is the WASM guest, which has no tracker at
//! all. Adding an accessor is a `crates/lca-tools/**` change, which this
//! cycle's boundary forbids. What *is* true, and is pinned below, is that the
//! tracker's purpose — "never write over contents you have not seen" — holds by
//! construction for this tool: an anchor only exists because a read produced
//! it, and if the file changes the anchor stops resolving and the edit refuses.
//!
//! Verifies: NFR-24 (a shipped defect's guard), GitHub issue #285,
//! gh #285 acceptance 1 and 3, with acceptance 2's limit recorded.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use hash_edit::{Edit, EditError, annotate_read_result, apply_edits, line_hash};

/// One anchored edit.
fn edit(start: &str, end: &str, new_text: &str) -> Edit {
    Edit {
        start_hash: start.to_string(),
        end_hash: end.to_string(),
        new_text: new_text.to_string(),
    }
}

/// The exact per-line shape the built-in `read` emits
/// (`format!("{:6}\t{line}\n")` in `crates/lca-tools/src/lib.rs`).
fn numbered(body: &str) -> String {
    body.split_inclusive('\n')
        .enumerate()
        .map(|(index, line)| {
            let line = line.trim_end_matches('\n');
            format!("{:6}\t{line}\n", index + 1)
        })
        .collect()
}

/// The file the acceptance row edits, and its `read` output.
const SOURCE: &str = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";

// ---------------------------------------------------------------------------
// Defect 1: the model can obtain anchors and the edit lands
// ---------------------------------------------------------------------------

/// **The issue's core row.** An ordinary `read` result, put through the hook,
/// carries an anchor the tool then resolves against the real file bytes. This
/// is the loop that did not exist before: read → annotated output → edit.
#[test]
fn a_read_gives_the_model_an_anchor_the_edit_uses() {
    let annotated = annotate_read_result("read", &numbered(SOURCE)).expect("a read annotates");

    // The model copies the anchor it sees in the gutter.
    let anchor = line_hash("    let x = 1;");
    assert!(
        annotated.contains(&anchor),
        "the gutter carries the line's hash: {annotated}"
    );

    // And the tool accepts it, on the real file.
    let updated = apply_edits(
        SOURCE.as_bytes(),
        &[edit(&anchor, &anchor, "    let x = 2;")],
    )
    .expect("the annotated anchor resolves");
    assert_eq!(
        String::from_utf8(updated).unwrap(),
        "fn main() {\n    let x = 2;\n    println!(\"{x}\");\n}\n"
    );
}

/// A multi-line range taken from two gutter hashes replaces exactly those
/// lines — the shapes the tool's own spec advertises.
#[test]
fn a_range_taken_from_two_gutter_hashes_replaces_that_range() {
    let annotated = annotate_read_result("read", &numbered(SOURCE)).expect("annotates");
    let start = line_hash("    let x = 1;");
    let end = line_hash("    println!(\"{x}\");");
    assert!(annotated.contains(&start) && annotated.contains(&end));

    let updated =
        apply_edits(SOURCE.as_bytes(), &[edit(&start, &end, "    let y = 2;")]).expect("resolves");
    assert_eq!(
        String::from_utf8(updated).unwrap(),
        "fn main() {\n    let y = 2;\n}\n"
    );
}

/// The gutter leaves the file's own text readable and unchanged after the
/// second tab. An annotation that rewrote the content would make the model
/// quote something the file does not contain.
#[test]
fn the_gutter_preserves_the_original_text_byte_for_byte() {
    let read = numbered(SOURCE);
    let annotated = annotate_read_result("read", &read).expect("annotates");
    let original_texts: Vec<&str> = read
        .lines()
        .map(|line| line.split_once('\t').map(|(_, rest)| rest).unwrap_or(""))
        .collect();
    let annotated_texts: Vec<&str> = annotated
        .lines()
        .map(|line| line.splitn(3, '\t').nth(2).unwrap_or(""))
        .collect();
    assert_eq!(
        original_texts, annotated_texts,
        "the file text is untouched"
    );
}

// ---------------------------------------------------------------------------
// Pass-through: the hook must never make a result worse
// ---------------------------------------------------------------------------

/// Only a successful built-in `read`. Every other tool is the identity patch,
/// so the hook cannot touch grep/shell/list output it has no business in.
#[test]
fn only_a_read_is_annotated() {
    for tool in [
        "grep",
        "list",
        "shell",
        "glob",
        "write",
        "edit",
        "hash_edit",
        "webfetch",
        "",
    ] {
        assert_eq!(
            annotate_read_result(tool, SOURCE),
            None,
            "`{tool}` is not the built-in read"
        );
    }
}

/// A read this hook cannot parse passes through untouched: no gutter, wrong
/// width, a continuation notice, an empty body. Byte-identical, not "roughly
/// the same".
#[test]
fn an_unparseable_read_is_byte_identical() {
    for text in [
        "",
        "no gutter at all\n",
        "1\ttab but one digit\n",
        "      1234567\tseven digits\n",
        "abcdef\tnot numeric\n",
        "[3 more lines in file. Use offset=4 to continue.]",
    ] {
        assert_eq!(
            annotate_read_result("read", text),
            None,
            "pass-through: {text:?}"
        );
    }
}

/// The continuation notice a truncated read appends is not a line, so it gains
/// no anchor — a fake anchor would send the model after text that is not in the
/// file.
#[test]
fn a_continuation_notice_gains_no_anchor() {
    let mut read = numbered("alpha\nbeta\n");
    read.push_str("[7 more lines in file. Use offset=3 to continue.]");
    let annotated = annotate_read_result("read", &read).expect("annotates");
    let notice = annotated
        .lines()
        .find(|line| line.contains("more lines"))
        .expect("notice survives");
    assert_eq!(
        notice, "[7 more lines in file. Use offset=3 to continue.]",
        "the notice is verbatim"
    );
}

/// An image result is text only in the tool-result envelope (the bytes ride
/// separately), and `read`'s image path returns a description rather than a
/// numbered listing — so it has no gutter and is left alone.
#[test]
fn an_image_read_result_gains_no_anchor() {
    let described = "Read image file [image/png]\n1200x800, resized to 1000x667 for this model";
    assert_eq!(annotate_read_result("read", described), None);
}

// ---------------------------------------------------------------------------
// The safety property: an anchor cannot outlive the file it describes
// ---------------------------------------------------------------------------

/// Defect 2's honest limit, stated as a test. The read tracker is private to
/// `ToolExecutor` and unreachable from an extension, so this crate cannot
/// consult it. What it *can* guarantee is the property the tracker exists for:
/// a stale anchor refuses, because the file no longer hashes to it. No write
/// lands over contents the model has not seen.
#[test]
fn an_anchor_never_resolves_against_changed_contents() {
    let annotated = annotate_read_result("read", &numbered("alpha\nbeta\n")).expect("annotates");
    let anchor = line_hash("alpha");
    assert!(annotated.contains(&anchor), "the read supplied the anchor");

    // Untouched: the anchor resolves, so the loop works.
    assert!(apply_edits(b"alpha\nbeta\n", &[edit(&anchor, &anchor, "ALPHA")]).is_ok());

    // Changed underneath the model: the same anchor refuses.
    let changed = b"gamma\ndelta\n";
    let refused = apply_edits(changed, &[edit(&anchor, &anchor, "ALPHA")]);
    assert!(
        matches!(refused, Err(EditError::Mismatch { .. })),
        "a changed file invalidates the anchor: {refused:?}"
    );
}

/// A wholly invented anchor still refuses. The hook did not make the tool
/// permissive: only hashes a real read could have produced resolve.
#[test]
fn an_invented_anchor_refuses() {
    let refused = apply_edits(b"alpha\nbeta\n", &[edit("zz9", "zz9", "x")]);
    assert!(
        matches!(refused, Err(EditError::Mismatch { .. })),
        "{refused:?}"
    );
}

/// An ambiguous anchor still refuses, unchanged: two identical lines hash the
/// same, and the tool will not guess which one was meant. The hook does not
/// disambiguate by line number, and should not — that would reintroduce the
/// positional anchors the tool exists to avoid.
#[test]
fn an_ambiguous_anchor_still_refuses() {
    let body = "same\nsame\n";
    let annotated = annotate_read_result("read", &numbered(body)).expect("annotates");
    let anchor = line_hash("same");
    assert!(annotated.contains(&anchor));
    let refused = apply_edits(body.as_bytes(), &[edit(&anchor, &anchor, "x")]);
    assert!(
        matches!(refused, Err(EditError::Ambiguous { .. })),
        "two identical lines refuse: {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// The world the registry actually dispatches on
// ---------------------------------------------------------------------------

/// `compose_tool_result` filters on `World::HooksToolResult`, so the handle
/// must declare it. A manifest that says `hooks-tool-result` while the handle
/// omits the world is exactly the kind of dead wiring this defect was.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_native_handle_declares_the_world_the_registry_dispatches_on() {
    use lca_ext_abi::{ExtensionDispatch, World};

    struct Deny;
    impl lca_permissions::PermissionPrompt for Deny {
        fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
            lca_permissions::Decision::Denied
        }
        fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
            false
        }
    }

    let root = std::env::temp_dir().join(format!("lca-he-285-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let prompt: std::sync::Arc<std::sync::Mutex<dyn lca_permissions::PermissionPrompt>> =
        std::sync::Arc::new(std::sync::Mutex::new(Deny));
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        lca_permissions::GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let cap = std::sync::Arc::new(lca_tools::Capabilities::new(
        "hash-edit",
        hash_edit::manifest_grants(),
        lca_permissions::ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        prompt,
        store,
        root.join("project"),
        None,
    ));
    let handle = hash_edit::HashEdit::new(cap);
    assert!(
        handle.worlds().contains(&World::HooksToolResult),
        "the anchor producer rides this world: {:?}",
        handle.worlds()
    );
    assert!(handle.worlds().contains(&World::Tool));
    let _ = std::fs::remove_dir_all(&root);
}
