use super::cut::previous_kept_start;
use super::*;
use lca_protocol::Usage;

fn user(id: &str) -> Record {
    Record::User {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        content: id.to_string(),
        attachments: Vec::new(),
        queue: None,
    }
}

fn assistant(id: &str, prompt_tokens: u64) -> Record {
    Record::Assistant {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        content: Vec::new(),
        reasoning: None,
        reasoning_signature: None,
        provider_thinking_level: None,
        model: None,
        provider: None,
        usage: Some(Usage {
            input: prompt_tokens,
            ..Default::default()
        }),
    }
}

fn call(id: &str, call_id: &str) -> Record {
    Record::ToolCall {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        call_id: call_id.to_string(),
        name: "read".to_string(),
        arguments: "{}".to_string(),
        source: lca_protocol::ToolSource::Builtin,
    }
}

fn result(id: &str, call_id: &str) -> Record {
    Record::ToolResult {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        call_id: call_id.to_string(),
        status: lca_protocol::ToolResultStatus::Ok,
        content: None,
        attachment: None,
        truncated: false,
        duration_ms: None,
        exit_code: None,
        nested: Vec::new(),
        full_output_path: None,
        images: Vec::new(),
    }
}

// Verifies: gh #36 phase 1 - the trigger is strict on both sides
// of `window - reserve`, and reserve 0 derives the stopgap's
// fraction (the old behavior by construction).
#[test]
fn the_trigger_fires_strictly_past_window_minus_reserve() {
    assert!(compaction_fires(5001, 10_000, 5000));
    assert!(!compaction_fires(5000, 10_000, 5000), "equal does not fire");
    assert!(!compaction_fires(4999, 10_000, 5000));
    assert_eq!(
        compaction_reserve(0.5, 0, 10_000),
        5000,
        "reserve 0 derives"
    );
    assert_eq!(compaction_reserve(0.8, 0, 128_000), 25_600);
    assert_eq!(compaction_reserve(0.5, 4096, 10_000), 4096, "absolute wins");
}

// Verifies: gh #36 phase 1 - the keep-recent window keeps the
// latest exchange verbatim and summarizes the rest.
#[test]
fn the_keep_recent_window_holds_the_latest_exchange() {
    // Eras: u1/a1 at 30k, u2/a2 at 70k, u3/a3 at 100k; keep 20k.
    let records = vec![
        user("u1"),
        assistant("a1", 30_000),
        user("u2"),
        assistant("a2", 70_000),
        user("u3"),
        assistant("a3", 100_000),
    ];
    let plan = plan_cut(&records, 6, 20_000, 100_000);
    assert_eq!(plan.start, 0, "no previous compaction");
    assert_eq!(plan.kept_start, 4, "u3/a3 stay verbatim");
    assert_eq!(plan.first_kept_id, "u3");
}

// Verifies: gh #36 phase 1 - a cut that would strand a tool result
// retreats so the call and its results stay on one side.
#[test]
fn a_cut_never_splits_a_tool_call_from_its_result() {
    let records = vec![
        user("u1"),
        assistant("a1", 30_000),
        call("c1", "k1"),
        result("r1", "k1"),
        user("u2"),
        assistant("a2", 100_000),
    ];
    // keep_recent lands the head on the result without the rule.
    let plan = plan_cut(&records, 6, 60_000, 100_000);
    let kept: Vec<&str> = records[plan.kept_start..6]
        .iter()
        .filter_map(Record::id)
        .collect();
    assert!(
        !(kept.contains(&"r1") && !kept.contains(&"c1")),
        "no stranded result: {kept:?}"
    );
    assert!(
        !(kept.contains(&"c1") && !kept.contains(&"r1")),
        "no stranded call: {kept:?}"
    );
}

// Verifies: gh #36 phase 2 - the previous summary is found for the
// next compaction (latest compaction record before the range with a
// non-empty summary), or absent when there is nothing to iterate.
#[test]
fn the_previous_summary_is_found_or_absent() {
    let records = vec![
        user("u1"),
        assistant("a1", 30_000),
        Record::Compaction {
            v: FORMAT_VERSION,
            ts: 1,
            id: "cmp1".to_string(),
            parent: None,
            replaced_from: "u1".to_string(),
            replaced_to: "a1".to_string(),
            first_kept_id: "u2".to_string(),
            read_files: Vec::new(),
            modified_files: Vec::new(),
            system_prompt: None,
            summary: "S1".to_string(),
            strategy: "x".to_string(),
            usage: None,
        },
        user("u2"),
        assistant("a2", 100_000),
    ];
    assert_eq!(previous_summary_text(&records, 5), Some("S1".to_string()));
    assert_eq!(previous_summary_text(&records, 0), None, "nothing before");
    let mut empty = records.clone();
    if let Record::Compaction { summary, .. } = &mut empty[2] {
        summary.clear();
    }
    assert_eq!(
        previous_summary_text(&empty, 5),
        None,
        "an empty summary is nothing to iterate"
    );
}

// Verifies: gh #36 phase 2 - a span straddling the cut splits at
// the documented point: the span head and prefix summarize, the
// tail stays verbatim, and no tool pair splits across the cut.
#[test]
fn a_straddling_span_splits_at_the_documented_point() {
    // One span [u1..a2] exceeding the budget; the walk stops mid-span.
    let records = vec![
        user("u1"),
        assistant("a1", 30_000),
        call("c1", "k1"),
        result("r1", "k1"),
        assistant("a2", 100_000),
    ];
    let plan = plan_cut(&records, 5, 20_000, 100_000);
    assert_eq!(plan.start, 0, "no previous compaction");
    assert_eq!(plan.kept_start, 2, "the tail stays verbatim");
    let summarized: Vec<&str> = records[plan.start..plan.kept_start]
        .iter()
        .filter_map(Record::id)
        .collect();
    assert_eq!(
        summarized,
        vec!["u1", "a1"],
        "the span head and prefix summarize"
    );
    let kept: Vec<&str> = records[plan.kept_start..5]
        .iter()
        .filter_map(Record::id)
        .collect();
    assert!(
        kept.contains(&"c1") == kept.contains(&"r1"),
        "no orphan: {kept:?}"
    );
    assert_eq!(plan.first_kept_id, "c1");
}

// Verifies: gh #36 phase 1 - the next compaction starts at the
// previous kept boundary, not at the session start.
#[test]
fn the_next_compaction_starts_at_the_previous_kept_boundary() {
    let records = vec![
        user("u1"),
        assistant("a1", 30_000),
        Record::Compaction {
            v: FORMAT_VERSION,
            ts: 1,
            id: "cmp1".to_string(),
            parent: None,
            replaced_from: "u1".to_string(),
            replaced_to: "a1".to_string(),
            first_kept_id: "u2".to_string(),
            read_files: Vec::new(),
            modified_files: Vec::new(),
            system_prompt: None,
            summary: "s".to_string(),
            strategy: "x".to_string(),
            usage: None,
        },
        user("u2"),
        assistant("a2", 100_000),
    ];
    let plan = plan_cut(&records, 5, 20_000, 100_000);
    assert_eq!(plan.start, 3, "starts at the kept boundary");
}

fn file_call(id: &str, name: &str, path: Option<&str>) -> Record {
    let arguments = match path {
        Some(path) => format!("{{\"path\": \"{path}\"}}"),
        None => "{}".to_string(),
    };
    Record::ToolCall {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        call_id: format!("k-{id}"),
        name: name.to_string(),
        arguments,
        source: lca_protocol::ToolSource::Builtin,
    }
}

fn compaction_with_files(id: &str, kept: &str, read: &[&str], modified: &[&str]) -> Record {
    Record::Compaction {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        replaced_from: "u0".to_string(),
        replaced_to: "a0".to_string(),
        first_kept_id: kept.to_string(),
        read_files: read.iter().map(ToString::to_string).collect(),
        modified_files: modified.iter().map(ToString::to_string).collect(),
        system_prompt: None,
        summary: "s".to_string(),
        strategy: "x".to_string(),
        usage: None,
    }
}

// Verifies: gh #36 phase 3 - file extraction reads tool calls
// (records and assistant blocks): reads stay reads, writes and
// edits count as modified, a file both read and modified counts
// as modified, and calls without a path never count.
#[test]
fn file_extraction_sorts_reads_from_modifications() {
    let candidate = vec![
        file_call("c1", "read", Some("b.md")),
        file_call("c2", "edit", Some("a.rs")),
        file_call("c3", "write", Some("b.md")),
        file_call("c4", "read", None),
        file_call("c5", "bash", Some("run.sh")),
        // Pi tracks read/write/edit only: a grep read never counts.
        file_call("c6", "grep", Some("x")),
        file_call("c7", "read", Some("notes.md")),
    ];
    let (read, modified) = accumulate_files(&candidate, &[]);
    assert_eq!(read, vec!["notes.md".to_string()], "read-only survivors");
    assert_eq!(
        modified,
        vec!["a.rs".to_string(), "b.md".to_string()],
        "edited plus written, sorted"
    );
}

// Verifies: gh #36 phase 3 - earlier compactions accumulate: the
// next record's lists union the candidate's with the priors'.
#[test]
fn file_lists_union_with_earlier_compactions() {
    let candidate = vec![file_call("c1", "read", Some("new.md"))];
    let priors = vec![compaction_with_files(
        "cmp0",
        "u1",
        &["old.md"],
        &["main.rs"],
    )];
    let (read, modified) = accumulate_files(&candidate, &priors);
    assert_eq!(read, vec!["new.md".to_string(), "old.md".to_string()]);
    assert_eq!(modified, vec!["main.rs".to_string()]);
}

// Verifies: gh #36 phase 3 - the lists stay bounded: past
// `MAX_TRACKED_FILES` the record keeps the first sorted entries.
#[test]
fn file_lists_stay_bounded() {
    let candidate: Vec<Record> = (0..MAX_TRACKED_FILES + 50)
        .map(|n| file_call(&format!("c{n}"), "read", Some(&format!("f{n:04}.md"))))
        .collect();
    let (read, _) = accumulate_files(&candidate, &[]);
    assert_eq!(read.len(), MAX_TRACKED_FILES, "capped");
    assert_eq!(read[0], "f0000.md", "sorted head kept");
}

// Verifies: gh #36 phase 3 - the summary sections use pi's
// `<read-files>` / `<modified-files>` shape, and empty lists
// append nothing.
#[test]
fn file_sections_shape_or_vanish() {
    let shaped = format_file_lists(&["r.md".to_string()], &["m.rs".to_string()]);
    assert_eq!(
        shaped,
        "\n\n<read-files>\nr.md\n</read-files>\n\n<modified-files>\nm.rs\n</modified-files>"
    );
    assert_eq!(
        format_file_lists(&[], &[]),
        "",
        "a fileless range reads unchanged"
    );
}

// Verifies: gh #36 phase 3 - the overflow patterns from the #36
// discussion plus our budgeted cap message trigger recovery;
// anything else surfaces without compacting.
// Verifies: gh #52 - per-vendor overflow strings (z.ai CN and
// siblings, pi's overflow catalog) recover; rate-limit text never
// does, even when it names tokens (pi's non-overflow exclusions).
#[test]
fn overflow_errors_match_the_documented_patterns() {
    for message in [
        "generation hit the token cap (max_tokens 13107) before finishing",
        "This model's maximum context length is 200000 tokens, prompt is too long",
        "request exceeds token limit",
        "max_tokens 100 exceeds model maximum",
        "input exceeds the context window",
        "CONTEXT_LENGTH exceeded",
        "{\"code\":\"1261\",\"message\":\"Prompt too long\"}",
        "{\"code\":\"1261\",\"message\":\"Prompt exceeds max length\"}",
        "model_context_window_exceeded",
        "request_too_large: Request exceeds the maximum size",
        "This model's maximum context length is 8192 tokens",
        "input is too long for requested model",
        "Range of input length should be [1, 200000]",
        "token limit exceeded for this model",
    ] {
        assert!(is_overflow_error(message), "recovers: {message}");
    }
    for message in [
        "model not found",
        "max_tokens must be positive",
        "connection reset by peer",
        "invalid api key",
        "provider returned HTTP 429: slow down",
        "rate limit exceeded, retry after 20s",
        "ThrottlingException: Too many tokens, please wait before trying again",
    ] {
        assert!(!is_overflow_error(message), "surfaces: {message}");
    }
}

// Verifies: gh #36 phase 3 - the checkpoint detects a move: an
// unchanged prompt records nothing, a changed one records the
// detection, and a first compaction has nothing to compare.
#[test]
fn system_change_detects_only_a_move() {
    let plain = vec![user("u1"), assistant("a1", 10)];
    assert_eq!(detect_system_change(&plain, "prompt"), None);
    let mut checked = plain.clone();
    checked.push(compaction_with_files("cmp1", "u1", &[], &[]));
    // No checkpoint on the record: nothing to compare.
    assert_eq!(detect_system_change(&checked, "prompt"), None);
    let mut checkpointed = plain.clone();
    let mut first = compaction_with_files("cmp1", "u1", &[], &[]);
    if let Record::Compaction { system_prompt, .. } = &mut first {
        *system_prompt = Some("prompt A".to_string());
    }
    checkpointed.push(first);
    assert_eq!(
        detect_system_change(&checkpointed, "prompt A"),
        None,
        "unchanged prompt records nothing"
    );
    let change =
        detect_system_change(&checkpointed, "prompt B").expect("a changed prompt is recorded");
    match &change {
        Record::Custom { custom_type, .. } => {
            assert_eq!(custom_type, SYSTEM_PROMPT_CHANGE_TYPE);
        }
        other => panic!("a custom detection record, not {other:?}"),
    }
}

// Verifies: gh #36 phase 3 - a retain-none anchor (the record's
// own id, pi's shape) starts the next range after the entry
// itself; any other anchor still starts at the kept record.
#[test]
fn retain_none_starts_after_the_entry() {
    let records = vec![
        user("u1"),
        assistant("a1", 30_000),
        compaction_with_files("cmp1", "cmp1", &[], &[]),
        user("u2"),
        assistant("a2", 100_000),
    ];
    assert_eq!(
        previous_kept_start(&records, 5),
        3,
        "after the retain-none entry"
    );
}
