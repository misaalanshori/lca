//! The default strategy against its two paths: the model answered
//! through `completion`, and the mechanical fallback when it is not
//! (ADR-0015: neither path is special).
//!
//! Verifies: FR-SESS-5 (compaction only through the world, default
//! enabled), FR-CTX-1's content (what the summary is made of), and the
//! capability's consent text shape (capability catalog `completion`).

use compaction_default::{Excerpt, MANIFEST, mechanical_summary, record_text, run_compact};

fn excerpts() -> Vec<Excerpt> {
    vec![
        (
            "user".to_string(),
            r#"{"v":1,"t":"user","id":"01","content":"add a parser"}"#.to_string(),
        ),
        (
            "assistant".to_string(),
            r#"{"v":1,"t":"assistant","id":"02","content":[{"type":"text","text":"done"}]}"#
                .to_string(),
        ),
        (
            "tool-call".to_string(),
            r#"{"v":1,"t":"tool-call","id":"03","name":"write","arguments":"{}"}"#.to_string(),
        ),
        (
            "tool-result".to_string(),
            r#"{"v":1,"t":"tool-result","id":"04","status":"ok","content":"wrote it"}"#.to_string(),
        ),
    ]
}

// Verifies: the record renderers agree across kinds - the same text
// both delivery modes put in the prompt.
#[test]
fn renders_each_record_kind() {
    assert_eq!(record_text("user", r#"{"content":"hello"}"#), "hello");
    assert_eq!(
        record_text(
            "assistant",
            r#"{"content":[{"type":"text","text":"hi "},{"type":"text","text":"there"}]}"#
        ),
        "hi there"
    );
    assert_eq!(
        record_text("tool-call", r#"{"name":"read","arguments":"{\"p\":1}"}"#),
        "-> read({\"p\":1})"
    );
    assert!(
        record_text("tool-result", r#"{"status":"error","content":"boom"}"#)
            .contains("[error] boom")
    );
}

// Verifies: ADR-0015's mechanical path works with no capability at all:
// user requests and the last exchange survive.
#[test]
fn the_mechanical_fallback_summarizes_without_a_model() {
    let summary = mechanical_summary(&excerpts());
    assert!(summary.contains("add a parser"), "{summary}");
    assert!(summary.contains("(4 messages)"), "{summary}");
    assert!(summary.contains("Last:"), "{summary}");
}

// Verifies: when the completion capability answers, its text is the
// summary (the flagged path), and an empty or failed answer degrades to
// the mechanical one instead of refusing the compaction.
#[test]
fn the_model_answer_wins_and_failures_degrade() {
    let used = run_compact(
        &excerpts(),
        Some(&|_prompt| Ok("MODEL SUMMARY".to_string())),
    );
    assert_eq!(used, "MODEL SUMMARY");

    let empty = run_compact(&excerpts(), Some(&|_prompt| Ok("   ".to_string())));
    assert!(empty.contains("add a parser"), "empty degrades: {empty}");

    let failed = run_compact(
        &excerpts(),
        Some(&|_prompt| Err("permission denied".to_string())),
    );
    assert!(failed.contains("add a parser"), "denial degrades: {failed}");

    let absent = run_compact(&excerpts(), None);
    assert!(absent.contains("add a parser"), "no backend: {absent}");
}

// Verifies: FR-SESS-5's manifest - the default strategy declares
// exactly the completion capability with the catalog's required reason,
// and nothing else (the consent text names the reason verbatim).
#[test]
fn the_manifest_declares_only_completion_with_its_reason() {
    let manifest: toml::Value = MANIFEST.parse().expect("MANIFEST parses");
    assert_eq!(
        manifest["capabilities"]["completion"]["reason"].as_str(),
        Some("Summarizes older parts of the conversation when compacting.")
    );
    let capabilities = manifest["capabilities"]
        .as_table()
        .expect("capabilities table");
    assert_eq!(
        capabilities.keys().collect::<Vec<_>>(),
        vec!["completion"],
        "a strategy needs nothing else: no fs, no net, no process"
    );
    let worlds = manifest["worlds"].as_array().expect("worlds");
    assert_eq!(worlds.len(), 1);
    assert_eq!(worlds[0].as_str(), Some("compaction"));
    let grants = compaction_default::manifest_grants();
    assert!(grants.completion);
    assert!(!grants.credentials && grants.fs.is_empty() && grants.net.is_empty());
}
