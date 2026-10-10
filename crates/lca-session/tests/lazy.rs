//! Lazy session creation rows (gh #122, EFG-034): no directory
//! until the first record lands, then full materialization.
//!
//! Every test runs against a scratch directory.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{SessionStore, ViewMode};

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

fn store(name: &str) -> SessionStore {
    SessionStore::new(scratch(name))
}

fn user_record(id: &str, content: &str) -> Record {
    Record::User {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        content: content.to_string(),
        attachments: vec![],
        queue: None,
    }
}

// Verifies: gh #122 - resolving a fresh session writes nothing:
// launch-and-quit leaves no directory behind.
#[test]
fn fresh_sessions_leave_no_directory() {
    let store = store("lazy-absent");
    let project = scratch("lazy-absent-project");
    let session = store.new_pending(&project, "test");
    assert!(!session.dir().exists(), "no dir until the first record");
    assert!(
        !session.meta_path().exists(),
        "no meta until the first record"
    );
}

// Verifies: gh #122 - the first append materializes: directory,
// meta, session-start first, then the record with the pending title.
#[test]
fn first_append_materializes_with_start_first() {
    let store = store("lazy-materialize");
    let project = scratch("lazy-materialize-project");
    let session = store.new_pending(&project, "my title");
    store
        .append(&session, user_record("u", "hi"))
        .expect("append");

    assert!(session.dir().is_dir(), "the dir lands");
    let meta = store.meta(&session).expect("meta");
    assert_eq!(meta.title, "my title");
    let text = std::fs::read_to_string(session.log_path()).expect("log");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "start, then the record");
    assert!(lines[0].contains("\"t\":\"session-start\""), "start first");
    assert!(lines[1].contains("\"t\":\"user\""), "then the record");
    // The materialized log reads back whole.
    let outcome = store.read(&session).expect("read");
    assert_eq!(outcome.records.len(), 2);
}

// Verifies: gh #122 - renaming before the first message retitles
// the pending spec (no warning, no directory); the rename trails
// as session-info on materialize like any rename.
#[test]
fn rename_before_first_message_retitles_pending() {
    let store = store("lazy-rename");
    let project = scratch("lazy-rename-project");
    let session = store.new_pending(&project, "old");
    store.rename(&session, "new").expect("rename");
    store
        .append(&session, user_record("u", "hi"))
        .expect("append");

    let meta = store.meta(&session).expect("meta");
    assert_eq!(meta.title, "new", "the pending title applies");
    let names: Vec<String> = store
        .read(&session)
        .expect("read")
        .records
        .iter()
        .filter_map(|record| match record {
            Record::SessionInfo { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["new".to_string()], "the rename trails");
}

// Verifies: gh #122 - reads over an unmaterialized session are
// empty, never errors (the interface opens onto nothing, cleanly).
#[test]
fn reads_over_unmaterialized_sessions_are_empty() {
    let store = store("lazy-reads");
    let project = scratch("lazy-reads-project");
    let session = store.new_pending(&project, "test");
    let display = store
        .resolved(&session, ViewMode::Display)
        .expect("display");
    assert!(display.records.is_empty());
    assert!(!display.truncated);
    let audit = store.resolved(&session, ViewMode::Audit).expect("audit");
    assert!(audit.records.is_empty());
    let tree = store.entry_tree(&session).expect("tree");
    assert!(tree.is_empty());
    let labels = store.labels(&session).expect("labels");
    assert!(labels.is_empty());
}

// Verifies: gh #122 - closing an unmaterialized session is a no-op
// (no session-end conjures a directory).
#[test]
fn close_skips_unmaterialized_sessions() {
    let store = store("lazy-close");
    let project = scratch("lazy-close-project");
    let session = store.new_pending(&project, "test");
    store.close(&session).expect("close");
    assert!(
        !session.dir().exists(),
        "close writes nothing without records"
    );
}

// Verifies: gh #122 - a model choice before the first message rides
// the pending spec into the materialized meta.
#[test]
fn model_choice_before_first_message_rides_pending() {
    let store = store("lazy-model");
    let project = scratch("lazy-model-project");
    let session = store.new_pending(&project, "test");
    store
        .record_model_used(&session, "prov", "model")
        .expect("record");
    store
        .append(&session, user_record("u", "hi"))
        .expect("append");
    let meta = store.meta(&session).expect("meta");
    assert_eq!(meta.model.as_deref(), Some("model"));
    assert_eq!(meta.provider.as_deref(), Some("prov"));
}
