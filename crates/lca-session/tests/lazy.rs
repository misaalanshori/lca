//! Lazy session creation rows (gh #122, EFG-034): no directory
//! until the first record lands, then full materialization.
//!
//! Every test runs against a scratch directory.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::{scratch, store, user_record};
use lca_protocol::Record;
use lca_session::ViewMode;

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

// Verifies: gh #339 finding 6 - a pending session's log lands **under the sessions
// root**, never next to the process.
//
// The defect was a fallback: `new_pending` used to be
// `new_pending_with_id(..).unwrap_or_else(|_| Session::new(String::new(),
// PathBuf::new()))`, and an empty directory joins to the *relative* `log.jsonl`. The
// first append would then have written a session log into whatever directory the
// process stood in - a data-location bug with no error and no session root above it.
//
// The rows below pin the property rather than the shape: the last one asserts the
// *relative* path really is what the old handle produced, so a future refactor back to
// a bare `Session` handle cannot pass by the property quietly becoming vacuous.
#[test]
fn a_pending_session_lands_under_the_sessions_root() {
    let store = store("lazy-scope");
    let project = scratch("lazy-scope-project");
    let session = store.new_pending(&project, "test");
    assert!(
        session.dir().starts_with(store.sessions_dir()),
        "the handle's directory sits under the sessions root: {:?} not under {:?}",
        session.dir(),
        store.sessions_dir()
    );
    assert!(
        session.log_path().is_absolute(),
        "so the log path is absolute, never a bare `log.jsonl` in the CWD: {:?}",
        session.log_path()
    );
    assert!(
        session.log_path().starts_with(store.sessions_dir()),
        "the log lands in the session, not beside it: {:?}",
        session.log_path()
    );
    assert!(
        lca_session::valid_session_id(session.id()),
        "the generated id is valid by construction, which is why the fallback is gone: {:?}",
        session.id()
    );
}

// Verifies: gh #339 finding 6 - the *other* half: validation stays on the path that
// takes an id from outside, because that one can still fail. Removing the fallback
// must not have removed the refusal with it.
#[test]
fn a_chosen_id_still_validates_before_any_handle_exists() {
    let store = store("lazy-invalid");
    let project = scratch("lazy-invalid-project");
    for bad in ["", "../escape", "a/b", "-leading"] {
        assert!(
            store.new_pending_with_id(&project, bad, "t").is_err(),
            "an id that escapes the project root is refused: {bad:?}"
        );
    }
}
