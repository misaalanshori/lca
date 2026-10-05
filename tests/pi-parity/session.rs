//! Session-log parity: JSONL framing, header-first, truncation recovery,
//! forward-compatible reads, and append-only compaction.

use lca_protocol::{ContentBlock, Record, Usage};
use lca_session::{SessionStore, ViewMode};

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(&format!("pi-parity-session-{name}"))
}

fn setup(name: &str) -> (SessionStore, lca_session::Session) {
    let root = scratch(name);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir project");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "parity").expect("session");
    (store, session)
}

fn user(id: &str, content: &str) -> Record {
    Record::User {
        v: 1,
        ts: 1,
        id: id.to_string(),
        content: content.to_string(),
        attachments: vec![],
        queue: None,
    }
}

fn assistant(id: &str) -> Record {
    Record::Assistant {
        v: 1,
        ts: 2,
        id: id.to_string(),
        content: vec![ContentBlock::Text {
            text: "done".to_string(),
        }],
        reasoning: None,
        model: Some("m".to_string()),
        provider: Some("p".to_string()),
        usage: Some(Usage::default()),
    }
}

// Verifies: pi:packages/coding-agent/docs/session-format.md#sessionheader
// (the first line of the file is the session header).
#[test]
fn pi_parity_session_log_is_json_lines_with_header_first() {
    let (store, session) = setup("header");
    store.append(&session, user("u1", "hello")).expect("append");
    let raw = std::fs::read_to_string(session.log_path()).expect("log bytes");
    assert!(!raw.is_empty(), "the log holds records");
    let mut lines = raw.lines();
    let first: serde_json::Value =
        serde_json::from_str(lines.next().expect("first line")).expect("line parses");
    assert_eq!(
        first.get("t").and_then(|t| t.as_str()),
        Some("session-start"),
        "LCA's header-first rule matches pi's header-first rule (shape differs: see docs/pi-parity.md)"
    );
    for line in lines {
        let value: serde_json::Value =
            serde_json::from_str(line).expect("every line is one JSON object");
        assert!(value.get("t").is_some(), "every record names its type");
    }
}

// Verifies: pi:packages/coding-agent/docs/json.md#framing-and-process-io
// (strict LF framing; a reader splits on LF).
#[test]
fn pi_parity_jsonl_framing_is_lf_delimited() {
    let (store, session) = setup("framing");
    store
        .append(&session, user("u1", "line1\nline2"))
        .expect("append");
    let raw = std::fs::read(session.log_path()).expect("log bytes");
    assert!(!raw.contains(&b'\r'), "no carriage returns in the framing");
    let text = String::from_utf8(raw).expect("utf8");
    for line in text.lines() {
        assert!(!line.is_empty(), "no blank framing lines");
        serde_json::from_str::<serde_json::Value>(line)
            .expect("each LF-delimited line is one object");
    }
}

// Verifies: pi:packages/coding-agent/docs/session-format.md#session-version
// (older sessions load; a corrupt tail keeps what came before).
#[test]
fn pi_parity_partial_tail_is_discarded_with_prefix_kept() {
    let (store, session) = setup("tail");
    store
        .append(&session, user("u1", "keep me"))
        .expect("append");
    {
        use std::io::Write as _;
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("open log");
        write!(log, "{{\"v\":1,\"t\":\"user\",").expect("partial write");
    }
    let outcome = store.read(&session).expect("read");
    assert!(outcome.truncated, "the cut tail is reported");
    assert!(
        outcome
            .records
            .iter()
            .any(|r| matches!(r, Record::User { content, .. } if content == "keep me")),
        "records before the failure survive"
    );
}

// Verifies: pi:packages/coding-agent/docs/session-format.md#session-version
// (existing sessions migrate forward; unknown content never kills the read).
#[test]
fn pi_parity_unknown_record_type_is_skipped_not_fatal() {
    let (store, session) = setup("unknown");
    store
        .append(&session, user("u1", "before"))
        .expect("append");
    {
        use std::io::Write as _;
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("open log");
        writeln!(
            log,
            "{{\"v\":99,\"t\":\"future-thing\",\"ts\":3,\"id\":\"f1\"}}"
        )
        .expect("write");
    }
    store.append(&session, user("u2", "after")).expect("append");
    let outcome = store.read(&session).expect("read");
    assert!(!outcome.truncated, "an unknown type is not corruption");
    assert!(outcome.skipped_unknown >= 1, "the skip is counted");
    assert!(!outcome.warnings.is_empty(), "the skip is explained");
    assert_eq!(
        outcome
            .records
            .iter()
            .filter(|r| matches!(r, Record::User { .. }))
            .count(),
        2,
        "records on both sides survive"
    );
}

// Verifies: pi:packages/coding-agent/docs/sessions.md#manage-conversation-context
// (compaction adds a summary; it does not delete the original entries).
#[test]
fn pi_parity_compaction_appends_marker_and_keeps_originals() {
    let (store, session) = setup("compact");
    store
        .append(&session, user("u1", "old question"))
        .expect("append");
    store.append(&session, assistant("a1")).expect("append");
    store
        .append(
            &session,
            Record::Compaction {
                v: 1,
                ts: 3,
                id: "c1".to_string(),
                replaced_from: "u1".to_string(),
                replaced_to: "a1".to_string(),
                summary: "the user asked; the model answered".to_string(),
                strategy: "parity-harness".to_string(),
                usage: None,
            },
        )
        .expect("append");
    let display = store
        .read_with(&session, ViewMode::Display)
        .expect("display read");
    assert!(
        display.records.iter().any(|r| matches!(r, Record::Compaction { summary, .. } if summary.contains("the model answered"))),
        "the display view substitutes the summary"
    );
    assert!(
        !display
            .records
            .iter()
            .any(|r| matches!(r, Record::User { content, .. } if content == "old question")),
        "the replaced range leaves the display view"
    );
    let audit = store
        .read_with(&session, ViewMode::Audit)
        .expect("audit read");
    assert!(
        audit
            .records
            .iter()
            .any(|r| matches!(r, Record::User { content, .. } if content == "old question")),
        "the audit view keeps the original records: nothing is rewritten"
    );
}
