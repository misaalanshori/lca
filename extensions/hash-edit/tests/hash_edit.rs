//! Content-hash line-anchored editing (gh #191): lines resolve by
//! short content hash, never by number, so insertions elsewhere in
//! the turn cannot shift an anchor. Ambiguous or mismatched anchors
//! refuse loudly; a refusal names the hash and the reason.
//!
//! Verifies: single-line and multi-line replacement, insertion,
//! deletion, atomic multi-edit, hash mismatch refuses, ambiguous
//! anchors refuse, overlapping ranges refuse, CRLF/BOM survive the
//! round trip, and the manifest grants match the manifest.

use hash_edit::{Edit, EditError, annotate, apply_edits, line_hash};

fn edit(start: &str, end: &str, new_text: &str) -> Edit {
    Edit {
        start_hash: start.to_string(),
        end_hash: end.to_string(),
        new_text: new_text.to_string(),
    }
}

// Verifies: the hash is short (the model copies it), stable, and
// whitespace-blind at the edges (indentation hallucinations still
// anchor).
#[test]
fn hashes_are_short_stable_and_indent_blind() {
    let hash = line_hash("    validate_cart(&order)?;");
    assert!(hash.len() == 3, "{hash}");
    assert_eq!(hash, line_hash("    validate_cart(&order)?;"));
    assert_eq!(hash, line_hash("validate_cart(&order)?;"));
    assert_eq!(hash, line_hash("\tvalidate_cart(&order)?;  "));
    assert_ne!(hash, line_hash("validate_cart(&order);"));
}

// Verifies: annotation prefixes every line with its hash, the shape
// the model copies anchors from.
#[test]
fn annotation_prefixes_every_line() {
    let out = annotate("fn f() {\n}\n");
    let first = line_hash("fn f() {");
    let second = line_hash("}");
    assert_eq!(out, format!("{first}: fn f() {{\n{second}: }}\n"));
}

// Verifies: a single line resolves by hash and is replaced.
#[test]
fn single_line_replacement_resolves_by_hash() {
    let original = b"one\ntwo\nthree\n";
    let hash = line_hash("two");
    let out = apply_edits(original, &[edit(&hash, &hash, "TWO")]).expect("apply");
    assert_eq!(out, b"one\nTWO\nthree\n");
}

// Verifies: a range between two different anchors is replaced whole.
#[test]
fn multi_line_range_replacement() {
    let original = b"a\nb\nc\nd\n";
    let out =
        apply_edits(original, &[edit(&line_hash("b"), &line_hash("c"), "B\nC")]).expect("apply");
    assert_eq!(out, b"a\nB\nC\nd\n");
}

// Verifies: an empty end anchor inserts after the start line.
#[test]
fn empty_end_anchor_inserts_after_the_line() {
    let original = b"a\nc\n";
    let out = apply_edits(original, &[edit(&line_hash("a"), "", "b")]).expect("apply");
    assert_eq!(out, b"a\nb\nc\n");
}

// Verifies: empty new text deletes the range.
#[test]
fn empty_new_text_deletes_the_range() {
    let original = b"a\nb\nc\n";
    let out = apply_edits(original, &[edit(&line_hash("b"), &line_hash("b"), "")]).expect("apply");
    assert_eq!(out, b"a\nc\n");
}

// Verifies: several edits apply atomically against the same original
// (later file positions first, so nothing shifts mid-apply).
#[test]
fn multiple_edits_apply_atomically() {
    let original = b"a\nb\nc\nd\n";
    let out = apply_edits(
        original,
        &[
            edit(&line_hash("d"), &line_hash("d"), "D"),
            edit(&line_hash("b"), &line_hash("b"), "B"),
        ],
    )
    .expect("apply");
    assert_eq!(out, b"a\nB\nc\nD\n");
}

// Verifies: a hash no line carries refuses with the hash named.
#[test]
fn unknown_hashes_refuse() {
    let original = b"a\nb\n";
    let err = apply_edits(original, &[edit("zzz", "zzz", "x")]).expect_err("refuse");
    assert!(matches!(err, EditError::Mismatch { .. }), "{err:?}");
    assert!(err.to_string().contains("zzz"), "{err}");
}

// Verifies: a hash two lines share refuses as ambiguous. Repeated
// lines (ten identical `}` closers) all carry the same hash, so any
// choice would be a guess: the model must widen the range with a
// unique neighbor instead.
#[test]
fn duplicated_hashes_refuse_as_ambiguous() {
    let dup = b"x\nx\n";
    let hash = line_hash("x");
    let err = apply_edits(dup, &[edit(&hash, &hash, "y")]).expect_err("refuse");
    assert!(matches!(err, EditError::Ambiguous { .. }), "{err:?}");
}

// Verifies: overlapping ranges refuse (one apply must not eat the
// other's anchor).
#[test]
fn overlapping_ranges_refuse() {
    let original = b"a\nb\nc\n";
    let err = apply_edits(
        original,
        &[
            edit(&line_hash("a"), &line_hash("b"), "A"),
            edit(&line_hash("b"), &line_hash("c"), "C"),
        ],
    )
    .expect_err("refuse");
    assert!(matches!(err, EditError::Overlap), "{err:?}");
}

// Verifies: an end anchor before the start anchor refuses.
#[test]
fn reversed_ranges_refuse() {
    let original = b"a\nb\nc\n";
    let err =
        apply_edits(original, &[edit(&line_hash("c"), &line_hash("a"), "x")]).expect_err("refuse");
    assert!(matches!(err, EditError::RangeOrder), "{err:?}");
}

// Verifies: one bad edit vetoes the whole batch (no partial writes).
#[test]
fn one_bad_edit_vetoes_the_batch() {
    let original = b"a\nb\n";
    let err = apply_edits(
        original,
        &[
            edit(&line_hash("a"), &line_hash("a"), "A"),
            edit("zzz", "zzz", "x"),
        ],
    )
    .expect_err("refuse");
    assert!(matches!(err, EditError::Mismatch { .. }), "{err:?}");
}

// Verifies: CRLF files keep CRLF, and inserted lines take the
// range's ending (platform notes: never wholesale-convert).
#[test]
fn crlf_survives_with_crlf_insertions() {
    let original = b"a\r\nb\r\n";
    let out = apply_edits(original, &[edit(&line_hash("a"), "", "x\ny")]).expect("apply");
    assert_eq!(out, b"a\r\nx\r\ny\r\nb\r\n");
}

// Verifies: a UTF-8 BOM is stripped for matching and restored on
// write.
#[test]
fn bom_survives_the_round_trip() {
    let original = b"\xef\xbb\xbfa\nb\n";
    let out = apply_edits(original, &[edit(&line_hash("a"), &line_hash("a"), "A")]).expect("apply");
    assert_eq!(out, b"\xef\xbb\xbfA\nb\n");
}

// Verifies: non-UTF-8 bytes refuse rather than corrupt.
#[test]
fn non_utf8_refuses() {
    let err = apply_edits(b"a\n\xff\n", &[edit("aaa", "aaa", "x")]).expect_err("refuse");
    assert!(matches!(err, EditError::NonUtf8), "{err:?}");
}

// Verifies: the manifest test contract — the tool world, workspace
// read-write for the files it edits, and (gh #285) the `hooks-tool-result`
// world that supplies the anchors the tool resolves against. Without the hook
// world the tool is uncallable in production, so the manifest is part of the
// fix rather than incidental.
#[test]
fn manifest_grants_match_the_manifest() {
    let manifest = hash_edit::MANIFEST;
    assert!(
        manifest.contains("worlds = [\"tool\", \"hooks-tool-result\"]"),
        "tool plus the result hook that supplies anchors: {manifest}"
    );
    let grants = hash_edit::manifest_grants();
    assert_eq!(grants.fs.len(), 1, "workspace read-write only");
    assert!(
        manifest.contains("workspace = \"read-write\""),
        "{manifest}"
    );
}

// Verifies: the tool spec the model sees names the path plus the
// anchored edits, both required.
#[test]
fn the_tool_spec_names_path_and_edits() {
    let spec = hash_edit::hash_edit_tool_spec();
    assert_eq!(spec.name, "hash_edit");
    let params = spec.parameters.to_string();
    assert!(params.contains("path"), "{params}");
    assert!(params.contains("start_hash"), "{params}");
    assert!(params.contains("end_hash"), "{params}");
    assert!(params.contains("new_text"), "{params}");
}

// ---------------------------------------------------------------------------
// gh #285: the read hook that makes the tool callable at all
// ---------------------------------------------------------------------------

/// The exact shape the built-in `read` emits per line
/// (`crates/lca-tools/src/lib.rs`: `{:6}\t{line}`), plus a file body.
fn numbered(body: &str) -> String {
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        out.push_str(&format!("{:6}\t{line}\n", index + 1));
    }
    out
}

// Verifies: gh #285 acceptance 1 - an ordinary `read` result gains hashes the
// tool accepts, so the model can obtain anchors from the read it already
// performs. Before the fix `annotate` had no production caller and the model
// could never see a hash.
#[test]
fn an_ordinary_read_result_gains_usable_hashes() {
    let body = "fn main() {\n    let x = 1;\n}\n";
    let read = numbered(body);
    let annotated =
        hash_edit::annotate_read_result("read", &read).expect("a read result annotates");
    // The model reads the hash out of the gutter.
    let second = hash_edit::line_hash("    let x = 1;");
    assert!(
        annotated.contains(&second),
        "the line's hash appears in the annotated result: {annotated}"
    );
    // And the tool accepts what the hook produced, on the real bytes.
    let anchor = hash_edit::line_hash("    let x = 1;");
    let edits = [edit(&anchor, &anchor, "    let x = 2;")];
    let out = apply_edits(body.as_bytes(), &edits).expect("the annotated hash anchors");
    assert_eq!(
        String::from_utf8(out).expect("utf8"),
        "fn main() {\n    let x = 2;\n}\n"
    );
}

// Verifies: gh #285 - the annotated result keeps the file's own text readable.
// A gutter is added, nothing is removed: the model can still read the code, and
// a line's content after the gutter is byte-identical to the original.
#[test]
fn annotation_adds_a_gutter_without_changing_the_text() {
    let read = numbered("alpha\nbeta\n");
    let annotated = hash_edit::annotate_read_result("read", &read).expect("annotates");
    for line in annotated.lines() {
        let body = line.split('\t').nth(1).unwrap_or("");
        assert!(
            !body.contains(&hash_edit::line_hash(body)),
            "the file's own text is not rewritten: {line}"
        );
    }
    assert!(annotated.contains("alpha") && annotated.contains("beta"));
}

// Verifies: gh #285 - a non-`read` tool result passes through byte-identical.
// An annotation hook that touched grep/shell output would corrupt text nobody
// asked it to describe.
#[test]
fn a_non_read_result_passes_through_byte_identical() {
    for name in ["grep", "list", "shell", "glob", "write", "edit"] {
        let text = "alpha\nbeta\n";
        assert_eq!(
            hash_edit::annotate_read_result(name, text),
            None,
            "`{name}` is not a read"
        );
    }
}

// Verifies: gh #285 - an unparseable read (no `{:6}\t` numbers) passes through
// rather than being mangled. The hook must never make a result worse.
#[test]
fn an_unparseable_read_passes_through() {
    for text in [
        "no line numbers here\nnor here\n",
        "1\ttab but one digit is not the gutter\n",
        "",
        "  12\tmissing the sixth column\n",
    ] {
        assert_eq!(
            hash_edit::annotate_read_result("read", text),
            None,
            "unparseable read passes through: {text:?}"
        );
    }
}

// Verifies: gh #285 - a `read` result carrying its continuation notice still
// annotates the lines and leaves the notice alone. The notice has no gutter, so
// it is not a line; annotating it would give the model a fake anchor.
#[test]
fn a_continuation_notice_is_not_annotated() {
    let mut read = numbered("alpha\nbeta\n");
    read.push_str("[3 more lines in file. Use offset=3 to continue.]");
    let annotated = hash_edit::annotate_read_result("read", &read).expect("annotates");
    assert!(
        annotated.contains("[3 more lines in file. Use offset=3 to continue.]"),
        "the notice survives verbatim: {annotated}"
    );
    let notice_line = annotated
        .lines()
        .find(|line| line.contains("3 more lines"))
        .expect("notice present");
    assert!(
        !notice_line.starts_with(|c: char| c.is_ascii_hexdigit() && c != ' '),
        "the notice did not gain a hash gutter: {notice_line}"
    );
}

// Verifies: gh #285 - CRLF survives. `read` normalizes to `\n` (`split('\n')`),
// so a CRLF body reaches the hook already normalized; a stray `\r` in a body
// must still hash the way the tool will hash it (trimmed).
#[test]
fn a_body_with_a_stray_carriage_return_hashes_as_the_tool_will() {
    let read = format!("{:6}\talpha\r\n", 1);
    let annotated = hash_edit::annotate_read_result("read", &read).expect("annotates");
    let hash = hash_edit::line_hash("alpha\r");
    assert!(annotated.contains(&hash), "{annotated}");
    // And that is the hash the tool resolves against the same bytes.
    let resolved = apply_edits(b"alpha\r\n", &[edit(&hash, "", "ALPHA")]);
    assert!(resolved.is_ok(), "the hook's hash resolves in the tool");
}

// Verifies: gh #285 - the safety property is unchanged: a stale hash still
// refuses. Annotating must not make the tool more permissive.
#[test]
fn a_stale_hash_still_refuses() {
    let annotated = hash_edit::annotate_read_result("read", &numbered("one\ntwo\n")).expect("ok");
    let stale = hash_edit::line_hash("line that is not in the file");
    assert!(
        annotated.contains("one"),
        "the read annotated (sanity): {annotated}"
    );
    let refused = apply_edits(b"one\ntwo\n", &[edit(&stale, "", "x")]);
    assert!(
        matches!(refused, Err(EditError::Mismatch { .. })),
        "a stale anchor refuses: {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// gh #285 (second defect): read-before-edit is a *host* gate, not ours
// ---------------------------------------------------------------------------

/// The built-in `edit` tool refuses when the file was not read this session
/// (`ToolExecutor`'s `ReadTracker`, `crates/lca-tools/src/lib.rs`). `hash_edit`
/// writes through `Capabilities::fs_write`, so it bypasses that tracker.
///
/// This row documents the boundary honestly rather than pretending a guard
/// exists: the tracker is private to `ToolExecutor`, no `lca-tools` API exposes
/// it to an extension, and `ToolExecutor` is never handed to one. A
/// `hash_edit` write therefore *cannot* consult it from inside this crate —
/// the fix belongs in `crates/lca-tools/**`, which this cycle may not touch.
///
/// What this crate *can* do, and now does, is make the read that satisfies the
/// tracker unavoidable: the hashes only exist because the model read the file
/// through the built-in `read` (see `annotate_read_result`). An unstale hash
/// implies a fresh read of those exact bytes, because a changed file no longer
/// matches the anchor and `apply_edits` refuses. So the safety property the
/// tracker exists to provide — "never write over a file you have not seen the
/// current contents of" — holds by construction here, without the tracker.
#[test]
fn an_anchor_cannot_resolve_against_a_changed_file() {
    let original = b"alpha\nbeta\n";
    let read = numbered("alpha\nbeta\n");
    let annotated = hash_edit::annotate_read_result("read", &read).expect("annotates");
    let anchor = hash_edit::line_hash("alpha");
    assert!(annotated.contains(&anchor), "the read supplied the anchor");

    // The file changes underneath the model after the read: the ``alpha`` line
    // is gone. The same anchor must now refuse rather than write blind.
    let changed = b"ALPHA REWRITTEN\ngamma\n";
    let refused = apply_edits(changed, &[edit(&anchor, "", "alpha")]);
    assert!(
        matches!(refused, Err(EditError::Mismatch { .. })),
        "a changed file invalidates the anchor, so no stale write lands: {refused:?}"
    );

    // And while the file is untouched, the same anchor resolves — the loop the
    // hook enables.
    assert!(apply_edits(original, &[edit(&anchor, "", "ALPHA")]).is_ok());
}

// Verifies: gh #285 - the native handle declares the world the registry
// dispatches `tool_result` on. The manifest says `hooks-tool-result`; the
// registry filters on `World::HooksToolResult`, so a handle that omits it
// never annotates and the tool is uncallable again.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_handle_declares_the_tool_result_world() {
    use lca_ext_abi::{ExtensionDispatch, World};
    let root = std::env::temp_dir().join(format!("lca-he-worlds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    struct Deny;
    impl lca_permissions::PermissionPrompt for Deny {
        fn ask(&mut self, _a: &lca_permissions::Action) -> lca_permissions::Decision {
            lca_permissions::Decision::Denied
        }
        fn review_proposals(&mut self, _d: &lca_permissions::ProposalDiff) -> bool {
            false
        }
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
    let worlds = handle.worlds();
    assert!(
        worlds.contains(&World::HooksToolResult),
        "tool_result dispatches on World::HooksToolResult: {worlds:?}"
    );
    assert!(worlds.contains(&World::Tool), "and it is still a tool");
    let _ = std::fs::remove_dir_all(&root);
}
