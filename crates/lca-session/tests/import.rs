//! Session import (gh #57): JSONL lines (one record per line, the
//! log's own shape) become a new resumable session.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_session::{SessionStore, ViewMode};

fn lines() -> Vec<String> {
    vec![
        r#"{"t":"session-start","v":1,"ts":1,"agent_version":"0.6.0","abi_version":"0.6","working_dir":"/tmp"}"#.to_string(),
        r#"{"t":"user","v":1,"ts":2,"id":"u1","content":"hello again","attachments":[]}"#.to_string(),
    ]
}

// Verifies: gh #57 - importing JSONL creates a session whose
// transcript reads back and appends (resumable, not a dead copy).
#[test]
fn imported_jsonl_resumes_as_a_readable_session() {
    let root = lca_testkit::scratch_path("lca-import");
    let store = SessionStore::new(root.join("data"));
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let owned = lines();
    let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
    let session = store
        .import_jsonl(&project, "imported", &borrowed)
        .expect("import");
    let read = store
        .read_with(&session, ViewMode::Audit)
        .expect("read back");
    assert_eq!(read.records.len(), 2, "both lines landed");
    // Resumable: a new turn appends onto the imported transcript.
    store
        .append(
            &session,
            lca_protocol::Record::User {
                v: 1,
                ts: 3,
                id: "u2".to_string(),
                parent: None,
                content: "and more".to_string(),
                attachments: Vec::new(),
                queue: None,
            },
        )
        .expect("append");
    let read = store
        .read_with(&session, ViewMode::Audit)
        .expect("read again");
    assert_eq!(read.records.len(), 3);
}

// Verifies: gh #57 - a corrupt line aborts the import with its line
// number, and no half-imported session is left behind.
#[test]
fn corrupt_lines_abort_before_any_session_exists() {
    let root = lca_testkit::scratch_path("lca-import-bad");
    let store = SessionStore::new(root.join("data"));
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let before = store.list_sessions(&project).expect("list").len();
    let owned = lines();
    let bad = vec![owned[0].as_str(), "not json at all"];
    let err = store
        .import_jsonl(&project, "bad", &bad)
        .expect_err("corrupt aborts");
    assert!(err.contains('2'), "names the line: {err}");
    assert_eq!(
        store.list_sessions(&project).expect("list").len(),
        before,
        "no half-imported session"
    );
}
