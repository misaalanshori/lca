//! Edit tool tests: exact matching, the fuzzy fallback (gh #114),
//! and the BOM/CRLF pipeline (gh #115).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lca_protocol::ToolResultStatus;
use lca_tools::{NativeOps, ToolExecutor};

fn scratch(name: &str) -> PathBuf {
    lca_testkit::scratch_path(name)
}

fn executor(workspace: &Path) -> ToolExecutor {
    ToolExecutor::new(
        Arc::new(NativeOps::default()),
        workspace.to_path_buf(),
        workspace.to_path_buf(),
        65536,
        Some(Duration::from_secs(120)),
    )
}

fn call(name: &str, args: serde_json::Value) -> lca_protocol::ToolCall {
    lca_protocol::ToolCall {
        call_id: "c1".to_string(),
        name: name.to_string(),
        arguments: args.to_string(),
        parent_call_id: None,
    }
}

async fn run(exec: &mut ToolExecutor, call: &lca_protocol::ToolCall) -> lca_protocol::ToolResult {
    exec.execute(call, &mut |_| {}, &lca_tools::CancelFlag::new())
        .await
}

// Verifies: FR-TOOL-2 (an edit of a file that changed since the last read
// is rejected with an error back to the model)
#[tokio::test]
async fn edit_rejects_a_file_that_changed_since_the_last_read() {
    let ws = scratch("edit-stale");
    std::fs::write(ws.join("a.txt"), "original").expect("write");
    let mut exec = executor(&ws);

    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "a.txt",
                "edits": [{"oldText": "original", "newText": "edited"}]
            }),
        ),
    )
    .await;
    assert_eq!(
        result.status,
        ToolResultStatus::Error,
        "no read yet: {}",
        result.content
    );
    assert!(result.content.contains("read"), "{}", result.content);

    let read = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "a.txt"})),
    )
    .await;
    assert_eq!(read.status, ToolResultStatus::Ok);

    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "a.txt",
                "edits": [{"oldText": "original", "newText": "edited"}]
            }),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(
        std::fs::read_to_string(ws.join("a.txt")).expect("read"),
        "edited"
    );

    // Something else changes the file after our read.
    let read = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "a.txt"})),
    )
    .await;
    assert_eq!(read.status, ToolResultStatus::Ok);
    std::fs::write(ws.join("a.txt"), "externally changed").expect("external write");
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "a.txt",
                "edits": [{"oldText": "externally", "newText": "locally"}]
            }),
        ),
    )
    .await;
    assert_eq!(
        result.status,
        ToolResultStatus::Error,
        "stale: {}",
        result.content
    );
    assert!(
        result.content.contains("changed since"),
        "{}",
        result.content
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("a.txt")).expect("read"),
        "externally changed",
        "no write happened"
    );
}

// Verifies: EFG-014 (phase 1) - an edit's result carries a unified diff
// of the change it applied: pi's `generateUnifiedPatch` shape (`---`/
// `+++`/`@@`), context lines around the change, and the summary line a
// human reads stays exactly what it was.
#[tokio::test]
async fn edit_result_carries_a_unified_diff_of_the_applied_change() {
    let ws = scratch("edit-diff");
    let original = "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n";
    std::fs::write(ws.join("a.rs"), original).expect("write");
    let mut exec = executor(&ws);
    let _ = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "a.rs"})),
    )
    .await;

    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "a.rs",
                "edits": [{"oldText": "fn b() {}", "newText": "fn bb() {}"}]
            }),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(
        result.content, "Successfully replaced 1 block(s) in a.rs.",
        "the summary a human reads is unchanged"
    );
    let diff = result
        .extras
        .get("diff")
        .expect("a successful edit carries its diff");
    assert!(
        diff.starts_with("--- a.rs\n+++ a.rs\n"),
        "unified headers name the file: {diff}"
    );
    assert!(
        diff.lines().any(|line| line.starts_with("@@ ")),
        "at least one hunk: {diff}"
    );
    assert!(
        diff.contains("-fn b() {}") && diff.contains("+fn bb() {}"),
        "the removed and added lines are both there: {diff}"
    );
    assert!(
        diff.contains(" fn c() {}"),
        "unchanged lines around the change ride as context: {diff}"
    );
    // The file on disk is what the diff describes - display only.
    assert_eq!(
        std::fs::read_to_string(ws.join("a.rs")).expect("read"),
        "fn a() {}\nfn bb() {}\nfn c() {}\nfn d() {}\n",
        "the edit applied exactly as before"
    );
}

// Verifies: EFG-014 (phase 1) - changes far apart are separate hunks,
// so a big file does not turn into one wall of context.
#[tokio::test]
async fn edits_far_apart_produce_separate_hunks() {
    let ws = scratch("edit-diff-hunks");
    let mut body = String::new();
    for i in 0..20 {
        body.push_str(&format!("line {i}\n"));
    }
    std::fs::write(ws.join("big.txt"), &body).expect("write");
    let mut exec = executor(&ws);
    let _ = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "big.txt"})),
    )
    .await;
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "big.txt",
                "edits": [
                    {"oldText": "line 0", "newText": "LINE 0"},
                    {"oldText": "line 19", "newText": "LINE 19"},
                ]
            }),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    let diff = result.extras.get("diff").expect("diff");
    let hunks = diff.lines().filter(|line| line.starts_with("@@ ")).count();
    assert_eq!(hunks, 2, "two changes, two hunks: {diff}");
}

/// One fuzzy edit through the executor: `read` first (the staleness
/// tracker), then `edit`, returning the result and the file content.
#[allow(clippy::expect_used)] // a test helper: an unreadable scratch file is the failure the rows report.
async fn fuzzy_edit(
    ws: &std::path::Path,
    name: &str,
    old: &str,
    new: &str,
) -> (lca_protocol::ToolResult, String) {
    let mut exec = executor(ws);
    let _ = run(&mut exec, &call("read", serde_json::json!({"path": name}))).await;
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({"path": name, "edits": [{"oldText": old, "newText": new}]}),
        ),
    )
    .await;
    let content = std::fs::read_to_string(ws.join(name)).expect("read back");
    (result, content)
}

// Verifies: gh #114 (pi's `test-fuzzy-1`): trailing whitespace on the
// file's lines still matches a clean oldText.
#[tokio::test]
async fn fuzzy_edit_strips_trailing_whitespace() {
    let ws = scratch("fuzzy-trailing");
    std::fs::write(ws.join("a.txt"), "line one   \nline two  \nline three\n").expect("write");
    let (result, content) = fuzzy_edit(&ws, "a.txt", "line one\nline two\n", "replaced\n").await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(content, "replaced\nline three\n");
}

// Verifies: gh #114 (pi's fullwidth + compatibility cases): NFKC
// equivalents match - fullwidth punctuation and composed characters.
#[tokio::test]
async fn fuzzy_edit_matches_nfkc_equivalents() {
    let ws = scratch("fuzzy-nfkc");
    std::fs::write(
        ws.join("a.txt"),
        "\u{FF21}\u{FF22}\u{FF23}\u{FF11}\u{FF12}\u{FF13}\ncafe\u{301}\n",
    )
    .expect("write");
    let (result, content) =
        fuzzy_edit(&ws, "a.txt", "ABC123\ncaf\u{e9}\n", "XYZ789\ncoffee\n").await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(content, "XYZ789\ncoffee\n");
}

// Verifies: gh #114 (pi's `test-fuzzy-2`/`test-fuzzy-3`): curly quotes
// in the file match straight quotes in oldText.
#[tokio::test]
async fn fuzzy_edit_matches_smart_quotes() {
    let ws = scratch("fuzzy-quotes");
    std::fs::write(ws.join("a.txt"), "console.log(\u{2018}hello\u{2019});\n").expect("write");
    let (result, content) = fuzzy_edit(
        &ws,
        "a.txt",
        "console.log('hello');",
        "console.log('world');",
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(content.contains("world"), "{content}");

    std::fs::write(
        ws.join("b.txt"),
        "const msg = \u{201C}Hello World\u{201D};\n",
    )
    .expect("write");
    let (result, content) = fuzzy_edit(
        &ws,
        "b.txt",
        "const msg = \"Hello World\";",
        "const msg = \"Goodbye\";",
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(content.contains("Goodbye"), "{content}");
}

// Verifies: gh #114 (pi's `test-fuzzy-4`): en/em dashes match `-`.
#[tokio::test]
async fn fuzzy_edit_matches_unicode_dashes() {
    let ws = scratch("fuzzy-dashes");
    std::fs::write(ws.join("a.txt"), "range: 1\u{2013}5\nbreak\u{2014}here\n").expect("write");
    let (result, content) = fuzzy_edit(
        &ws,
        "a.txt",
        "range: 1-5\nbreak-here",
        "range: 10-50\nbreak--here",
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(content.contains("10-50"), "{content}");
}

// Verifies: gh #114 (pi's `test-fuzzy-5`): NBSP matches a space.
#[tokio::test]
async fn fuzzy_edit_matches_nbsp() {
    let ws = scratch("fuzzy-nbsp");
    std::fs::write(ws.join("a.txt"), "hello\u{a0}world\n").expect("write");
    let (result, content) = fuzzy_edit(&ws, "a.txt", "hello world", "hello universe").await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(content.contains("universe"), "{content}");
}

// Verifies: gh #114 (pi's `test-fuzzy-6`): an exact match always wins
// over a fuzzy one.
#[tokio::test]
async fn exact_match_always_beats_fuzzy() {
    let ws = scratch("fuzzy-exact-first");
    std::fs::write(ws.join("a.txt"), "const x = 'exact';\nconst y = 'other';\n").expect("write");
    let (result, content) =
        fuzzy_edit(&ws, "a.txt", "const x = 'exact';", "const x = 'changed';").await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(content, "const x = 'changed';\nconst y = 'other';\n");
}

// Verifies: gh #114 (pi's `test-fuzzy-7`/`test-fuzzy-8`): no match is
// still an error, and two fuzzy hits are an ambiguity error, not a
// guess.
#[tokio::test]
async fn fuzzy_misses_and_ambiguities_are_errors() {
    let ws = scratch("fuzzy-errors");
    std::fs::write(ws.join("a.txt"), "completely different content\n").expect("write");
    let (result, _) = fuzzy_edit(&ws, "a.txt", "this does not exist", "x").await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(result.content.contains("not found"), "{}", result.content);

    std::fs::write(ws.join("b.txt"), "hello world   \nhello world\n").expect("write");
    let (result, _) = fuzzy_edit(&ws, "b.txt", "hello world", "replaced").await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(
        result.content.contains("more than once"),
        "ambiguity, not a guess: {}",
        result.content
    );
}

// Verifies: gh #115 (pi's CRLF cases): LF oldText matches a CRLF
// file, and the file keeps CRLF after the write.
#[tokio::test]
async fn crlf_files_match_lf_oldtext_and_keep_crlf() {
    let ws = scratch("edit-crlf");
    std::fs::write(ws.join("a.txt"), "line one\r\nline two\r\nline three\r\n").expect("write");
    let (result, _) = fuzzy_edit(&ws, "a.txt", "line two\n", "replaced line\n").await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    let bytes = std::fs::read(ws.join("a.txt")).expect("read");
    assert_eq!(
        bytes,
        b"line one\r\nreplaced line\r\nline three\r\n".as_slice()
    );

    std::fs::write(ws.join("b.txt"), "first\nsecond\nthird\n").expect("write");
    let (result, _) = fuzzy_edit(&ws, "b.txt", "second\n", "REPLACED\n").await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(
        std::fs::read(ws.join("b.txt")).expect("read"),
        b"first\nREPLACED\nthird\n".as_slice()
    );
}

// Verifies: gh #115 (pi's mixed-endings case): one logical match in
// each ending style is two occurrences, and two is an error.
#[tokio::test]
async fn duplicates_across_line_ending_variants_are_an_error() {
    let ws = scratch("edit-crlf-dups");
    std::fs::write(ws.join("a.txt"), "hello\r\nworld\r\n---\r\nhello\nworld\n").expect("write");
    let (result, _) = fuzzy_edit(&ws, "a.txt", "hello\nworld\n", "replaced\n").await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(
        result.content.contains("more than once"),
        "{}",
        result.content
    );
}

// Verifies: gh #115 (pi's BOM cases): a BOM file edits fine and keeps
// its BOM, including CRLF + multi-edit.
#[tokio::test]
async fn bom_files_keep_their_bom() {
    let ws = scratch("edit-bom");
    std::fs::write(ws.join("a.txt"), "\u{FEFF}first\r\nsecond\r\nthird\r\n").expect("write");
    let (result, _) = fuzzy_edit(&ws, "a.txt", "second\n", "REPLACED\n").await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(
        std::fs::read(ws.join("a.txt")).expect("read"),
        "\u{FEFF}first\r\nREPLACED\r\nthird\r\n".as_bytes()
    );

    std::fs::write(
        ws.join("b.txt"),
        "\u{FEFF}first\r\nsecond\r\nthird\r\nfourth\r\n",
    )
    .expect("write");
    let mut exec = executor(&ws);
    let _ = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "b.txt"})),
    )
    .await;
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({"path": "b.txt", "edits": [
                {"oldText": "second\n", "newText": "SECOND\n"},
                {"oldText": "fourth\n", "newText": "FOURTH\n"},
            ]}),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(
        std::fs::read(ws.join("b.txt")).expect("read"),
        "\u{FEFF}first\r\nSECOND\r\nthird\r\nFOURTH\r\n".as_bytes()
    );
}

// Verifies: EFG-014 (phase 1) - pi's legacy `{oldText,newText}` input
// applies exactly like the `edits` array (models trained on pi still
// emit it), and a rejected edit carries no diff: the diff is display of
// a change that happened, never a claim about one that did not.
#[tokio::test]
async fn the_legacy_single_edit_input_applies_and_a_rejected_edit_has_no_diff() {
    let ws = scratch("edit-legacy");
    std::fs::write(ws.join("a.txt"), "one\ntwo\n").expect("write");
    let mut exec = executor(&ws);
    let _ = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "a.txt"})),
    )
    .await;

    // The legacy top-level form, no `edits` key at all.
    let legacy = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({"path": "a.txt", "oldText": "two", "newText": "TWO"}),
        ),
    )
    .await;
    assert_eq!(
        legacy.status,
        ToolResultStatus::Ok,
        "the legacy form applies: {}",
        legacy.content
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("a.txt")).expect("read"),
        "one\nTWO\n",
        "same write the `edits` array would have made"
    );
    assert!(
        legacy.extras.contains_key("diff"),
        "the legacy form's diff rides too"
    );

    // A rejected edit (stale file) carries no diff.
    std::fs::write(ws.join("a.txt"), "changed by something else\n").expect("write");
    let rejected = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({"path": "a.txt", "oldText": "one", "newText": "1"}),
        ),
    )
    .await;
    assert_eq!(
        rejected.status,
        ToolResultStatus::Error,
        "the staleness guard still rejects"
    );
    assert!(
        !rejected.extras.contains_key("diff"),
        "a rejected edit claims nothing: {:?}",
        rejected.extras
    );
}

// Edits match against the original file, never incrementally, and every
// oldText must be unique (the editing contract the model relies on).
#[tokio::test]
async fn edits_match_against_the_original_uniquely() {
    let ws = scratch("edit-unique");
    std::fs::write(ws.join("a.rs"), "fn a() {}\nfn b() {}\n").expect("write");
    let mut exec = executor(&ws);
    let _ = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "a.rs"})),
    )
    .await;

    // Non-unique old text is rejected.
    std::fs::write(ws.join("b.txt"), "x x").expect("write");
    let _ = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "b.txt"})),
    )
    .await;
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "b.txt",
                "edits": [{"oldText": "x", "newText": "y"}]
            }),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Error, "{}", result.content);

    // Two disjoint edits both see the original content.
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "a.rs",
                "edits": [
                    {"oldText": "fn a() {}", "newText": "fn aa() {}"},
                    {"oldText": "fn b() {}", "newText": "fn bb() {}"}
                ]
            }),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(
        std::fs::read_to_string(ws.join("a.rs")).expect("read"),
        "fn aa() {}\nfn bb() {}\n"
    );
}
