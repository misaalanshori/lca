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

// A re-compaction's candidate range includes the previous compaction record.
// Rendering it as a placeholder dropped every fact the earlier summary held,
// so a codeword vanished the second time compaction fired.
#[test]
fn a_prior_summary_is_carried_into_the_next_one() {
    let body = r#"{"v":1,"t":"compaction","summary":"The codeword is quartz-77."}"#;
    assert_eq!(
        record_text("compaction", body),
        "The codeword is quartz-77."
    );
}

// The no-model fallback must not drop a prior summary either.
#[test]
fn the_mechanical_fallback_carries_a_prior_summary() {
    let excerpts = vec![
        (
            "compaction".to_string(),
            r#"{"summary":"codeword quartz-77"}"#.to_string(),
        ),
        ("user".to_string(), r#"{"content":"next"}"#.to_string()),
    ];
    let summary = mechanical_summary(&excerpts);
    assert!(summary.contains("quartz-77"), "{summary}");
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

// Verifies: gh #36 phase 2 - the host's previous-summary marker renders
// as the summary text (not a placeholder), so the second compaction
// refines instead of restarting.
#[test]
fn a_previous_summary_marker_renders_its_text() {
    let body = r#"{"v":1,"t":"custom","id":"previous-summary","custom_type":"previous-summary","data":{"text":"Goal met: parser done."}}"#;
    assert_eq!(record_text("custom", body), "Goal met: parser done.");
    assert_eq!(
        record_text("custom", r#"{"custom_type":"other","data":{}}"#),
        "[custom]"
    );
}

// Verifies: gh #36 phase 2 - the prompt wraps the previous summary in
// its tags with an iteration instruction, and caps it: a 9000-char
// old summary arrives truncated, head kept.
#[test]
fn the_prompt_iterates_a_capped_previous_summary() {
    let long = "s".repeat(9000);
    let body = format!(
        r#"{{"v":1,"t":"custom","id":"previous-summary","custom_type":"previous-summary","data":{{"text":"{long}"}}}}"#
    );
    let prompt = std::cell::RefCell::new(String::new());
    run_compact(
        &[
            ("custom".to_string(), body),
            (
                "user".to_string(),
                r#"{"v":1,"t":"user","id":"01","content":"keep going"}"#.to_string(),
            ),
        ],
        Some(&|seen: &str| {
            prompt.borrow_mut().push_str(seen);
            Ok("MODEL SUMMARY".to_string())
        }),
    );
    let prompt = prompt.into_inner();
    assert!(
        prompt.contains("<previous-summary>"),
        "the old summary is tagged: {prompt}",
    );
    assert!(
        prompt.contains("incorporate into the existing summary")
            || prompt.contains("existing summary"),
        "the model is told to refine: {prompt}",
    );
    let tagged: String = prompt
        .split("<previous-summary>")
        .nth(1)
        .unwrap_or_default()
        .split("</previous-summary>")
        .next()
        .unwrap_or_default()
        .to_string();
    let text = tagged
        .strip_prefix('\n')
        .unwrap_or(&tagged)
        .strip_prefix("custom: ")
        .unwrap_or(&tagged);
    let text = text.strip_suffix('\n').unwrap_or(text);
    assert!(
        text.len() <= 4000,
        "the summary text is capped: {}",
        text.len()
    );
    let tagged = text;
    assert!(tagged.starts_with("ssss"), "head kept, tail cut");
}

// ---- gh #281: the mechanical path must carry the host's marker ----
//
// The host carries the previous summary forward as a `custom` record whose
// `custom_type` is `lca_protocol::PREVIOUS_SUMMARY_TYPE` ("previous-summary"),
// with the text under `data.text`. `mechanical_summary` filtered for those
// records correctly but then decoded every one of them with
// `record_text("compaction", body)`, and the `compaction` branch reads
// `summary` - a key a custom record does not have. Every carried summary
// therefore rendered as the empty string and vanished from the fallback,
// which is exactly the path that runs when the model is unavailable and a
// long session most needs its history.

/// The marker body the host writes (`previous_summary_marker` in
/// `crates/lca-core/src/compact.rs`), built here rather than through that
/// private constructor.
fn previous_summary_body(text: &str) -> String {
    serde_json::json!({
        "v": 1,
        "t": "custom",
        "id": "previous-summary",
        "custom_type": lca_protocol::PREVIOUS_SUMMARY_TYPE,
        "data": { "text": text },
    })
    .to_string()
}

// Verifies: gh #281 - a carried-forward summary in the host's marker record
// reaches the mechanical fallback. On the pre-fix tree this fails: the text
// decodes to "" and "Earlier summary" is absent entirely.
#[test]
fn the_mechanical_fallback_carries_the_hosts_previous_summary_marker() {
    let excerpts = vec![
        (
            "custom".to_string(),
            previous_summary_body("codeword quartz-77"),
        ),
        (
            "user".to_string(),
            r#"{"v":1,"t":"user","id":"02","content":"carry on"}"#.to_string(),
        ),
    ];
    let summary = mechanical_summary(&excerpts);
    assert!(
        summary.contains("quartz-77"),
        "the carried summary survives the fallback: {summary}"
    );
    assert!(
        summary.contains("Earlier summary:"),
        "it is labelled as one: {summary}"
    );
}

// Verifies: gh #281 - the negative that keeps the filter honest: an
// unrelated `custom` record is not a summary and must not be lifted into
// "Earlier summary".
#[test]
fn an_unrelated_custom_record_is_not_a_carried_summary() {
    let excerpts = vec![
        (
            "custom".to_string(),
            r#"{"v":1,"t":"custom","id":"05","custom_type":"system-prompt-change","data":{"text":"the prompt changed"}}"#
                .to_string(),
        ),
        (
            "user".to_string(),
            r#"{"v":1,"t":"user","id":"06","content":"go on"}"#.to_string(),
        ),
    ];
    let summary = mechanical_summary(&excerpts);
    assert!(
        !summary.contains("Earlier summary:"),
        "an unrelated custom record is not a summary: {summary}"
    );
    assert!(
        !summary.contains("the prompt changed"),
        "its payload stays out of the carried line: {summary}"
    );
}

// Verifies: gh #281 - the old filter's failure pinned from the other side: a
// record that merely mentions the word "compaction" was mistaken for a
// summary by a substring match. Text is not a type.
#[test]
fn a_record_mentioning_compaction_is_not_a_summary() {
    let excerpts = vec![
        (
            "user".to_string(),
            r#"{"v":1,"t":"user","id":"07","content":"what does compaction do here?"}"#
                .to_string(),
        ),
        (
            "assistant".to_string(),
            r#"{"v":1,"t":"assistant","id":"08","content":[{"type":"text","text":"Compaction summarizes old turns."}]}"#
                .to_string(),
        ),
    ];
    let summary = mechanical_summary(&excerpts);
    assert!(
        !summary.contains("Earlier summary:"),
        "prose about compaction is not a compaction record: {summary}"
    );
}

// Verifies: gh #281 - the fallback's own 600-char cap still bounds a carried
// summary, so one enormous earlier summary cannot crowd out the rest of the
// mechanical text (the shape is unchanged; only which records feed it is).
#[test]
fn an_oversized_carried_summary_still_truncates() {
    let long = "q".repeat(5000);
    let excerpts = vec![
        ("custom".to_string(), previous_summary_body(&long)),
        (
            "user".to_string(),
            r#"{"v":1,"t":"user","id":"09","content":"and then"}"#.to_string(),
        ),
    ];
    let summary = mechanical_summary(&excerpts);
    let carried = summary
        .split("Earlier summary: ")
        .nth(1)
        .expect("the carried line exists")
        .split('\n')
        .next()
        .unwrap_or_default();
    assert_eq!(
        carried.chars().count(),
        600,
        "the mechanical cap holds at 600 chars, not 5000"
    );
    assert!(carried.starts_with('q'), "head kept, tail cut");
}

// Verifies: gh #281 - a `compaction` record and a marker record in the same
// range both survive, in log order, joined as the mechanical text always
// joined them.
#[test]
fn a_compaction_record_and_a_marker_both_carry_forward() {
    let excerpts = vec![
        (
            "compaction".to_string(),
            r#"{"v":1,"t":"compaction","summary":"first pass: alpha"}"#.to_string(),
        ),
        (
            "custom".to_string(),
            previous_summary_body("second pass: beta"),
        ),
        (
            "user".to_string(),
            r#"{"v":1,"t":"user","id":"0a","content":"continue"}"#.to_string(),
        ),
    ];
    let summary = mechanical_summary(&excerpts);
    let earlier = summary
        .split("Earlier summary: ")
        .nth(1)
        .expect("the carried line exists")
        .split('\n')
        .next()
        .unwrap_or_default();
    assert!(
        earlier.contains("alpha") && earlier.contains("beta"),
        "both carry, in order: {earlier}"
    );
}

// Verifies: gh #281 - an empty or absent carried summary leaves no stray
// "Earlier summary" header behind (the design's rule 4).
#[test]
fn an_empty_carried_summary_leaves_no_header() {
    let excerpts = vec![
        ("custom".to_string(), previous_summary_body("")),
        (
            "user".to_string(),
            r#"{"v":1,"t":"user","id":"0b","content":"still here"}"#.to_string(),
        ),
    ];
    let summary = mechanical_summary(&excerpts);
    assert!(
        !summary.contains("Earlier summary:"),
        "no header for nothing: {summary}"
    );
}
