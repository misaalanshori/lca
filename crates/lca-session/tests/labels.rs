//! Label bookmark tests (gh #37 phase 1, ADR-0046): bookmarks name
//! records, persist as `label` records, resolve latest-wins, and ride
//! the export.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{ExportOptions, SessionStore};

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

// Verifies: FR-SESS-10 (naming a bookmark persists a `label` record)
#[test]
fn setting_a_label_persists_a_label_record() {
    let store = store("label-set");
    let project = scratch("label-set-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "first"))
        .expect("append");
    store
        .set_label(&session, "r1", Some("checkpoint"))
        .expect("label");

    let outcome = store.read(&session).expect("read");
    let labeled = outcome
        .records
        .iter()
        .filter_map(|record| match record {
            Record::Label {
                target_id, label, ..
            } => Some((target_id.clone(), label.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        labeled,
        vec![("r1".to_string(), Some("checkpoint".to_string()))],
        "one label record naming the target"
    );
}

// Verifies: FR-SESS-10 (the latest label per record wins; absent clears)
#[test]
fn labels_resolve_latest_wins_and_clear() {
    let store = store("label-wins");
    let project = scratch("label-wins-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "first"))
        .expect("append");
    store.set_label(&session, "r1", Some("a")).expect("a");
    store.set_label(&session, "r1", Some("b")).expect("b");
    assert_eq!(
        store.labels(&session).expect("labels").get("r1"),
        Some(&"b".to_string()),
        "latest wins"
    );
    store.set_label(&session, "r1", None).expect("clear");
    assert!(
        !store.labels(&session).expect("relist").contains_key("r1"),
        "absent clears the bookmark"
    );
}

// Verifies: FR-SESS-10 (a bookmark on nothing fails loudly)
#[test]
fn labeling_a_missing_record_fails() {
    let store = store("label-missing");
    let project = scratch("label-missing-project");
    let session = store.create_session(&project, "test").expect("create");
    let err = store
        .set_label(&session, "nope", Some("x"))
        .expect_err("missing target must fail");
    assert!(
        err.to_string().contains("nope"),
        "the error names the missing record: {err}"
    );
}

// Verifies: FR-SESS-10 (a label name resolves to its record)
#[test]
fn resolve_label_finds_the_target_by_name() {
    let store = store("label-resolve");
    let project = scratch("label-resolve-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "first"))
        .expect("append1");
    store
        .append(&session, user_record("r2", "second"))
        .expect("append2");
    store
        .set_label(&session, "r2", Some("here"))
        .expect("label");
    assert_eq!(
        store.resolve_label(&session, "here").expect("resolve"),
        Some("r2".to_string())
    );
    assert_eq!(
        store.resolve_label(&session, "missing").expect("resolve"),
        None
    );
}

// Verifies: FR-SESS-10 (the label is readable in the session export)
#[test]
fn labels_ride_the_export() {
    let store = store("label-export");
    let project = scratch("label-export-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "first"))
        .expect("append");
    store
        .set_label(&session, "r1", Some("keep"))
        .expect("label");
    let path = store
        .export(&session, ExportOptions::default())
        .expect("export");
    let text = std::fs::read_to_string(&path).expect("read export");
    assert!(
        text.contains("\"keep\"") && text.contains("\"r1\""),
        "the export carries the bookmark:\n{text}"
    );
}
