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
// read-write for the files it edits.
#[test]
fn manifest_grants_match_the_manifest() {
    let manifest = hash_edit::MANIFEST;
    assert!(manifest.contains("worlds = [\"tool\"]"), "{manifest}");
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
