//! `meta.json` rows: the stored working directory is the display form, the
//! model/provider last used is recorded, and a rename updates meta and the
//! index together.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::common::{scratch, store, user_record};

// Session lookup by id for resume/fork/export; a missing or foreign session
// is an error the CLI maps to exit code 6 (docs/headless.md).
#[test]
fn looks_up_a_session_by_id_within_the_project() {
    let store = store("lookup");
    let project = scratch("lookup-project");
    let session = store.create_session(&project, "mine").expect("create");
    let found = store.session(&project, session.id()).expect("found");
    assert_eq!(found.id(), session.id());
    let err = store
        .session(&project, "no-such-session")
        .expect_err("missing");
    assert!(err.to_string().contains("no-such-session"), "{err}");
}

// rename updates the title where a reader looks: meta.json and the index.
#[test]
fn renaming_updates_meta_and_the_index() {
    let store = store("rename");
    let project = scratch("rename-project");
    let session = store.create_session(&project, "old title").expect("create");
    store
        .append(&session, user_record("r1", "hi"))
        .expect("append");
    store.rename(&session, "new title").expect("rename");
    assert_eq!(store.meta(&session).expect("meta").title, "new title");
    let listed = store.list_sessions(&project).expect("list");
    assert_eq!(listed[0].title, "new title");
}

// Verifies: FR-SESS-9 (R7b) - what is stored is the display form of the
// working directory: a Windows `\\?\` verbatim path is stripped (and a
// `\\?\UNC\server\share` restored), while an ordinary path is untouched.
#[test]
fn stored_working_dirs_are_the_display_form() {
    use lca_session::display_path;
    use std::path::Path;
    assert_eq!(
        display_path(Path::new(r"\\?\C:\Users\misapw\pi")),
        r"C:\Users\misapw\pi"
    );
    assert_eq!(
        display_path(Path::new(r"\\?\UNC\server\share\dir")),
        r"\\server\share\dir"
    );
    assert_eq!(display_path(Path::new("/home/me/proj")), "/home/me/proj");
    assert_eq!(display_path(Path::new(r"C:\plain")), r"C:\plain");
}

// Verifies: FR-SESS-9 (R7b) - the session-start record and meta.json carry
// the display path, not the canonicalized (possibly verbatim) one.
#[test]
fn session_start_records_the_display_working_dir() {
    let store = store("display-dir");
    let project = scratch("display-dir-project");
    let session = store
        .create_session(&project, lca_session::DEFAULT_TITLE)
        .expect("create");
    let meta = store.meta(&session).expect("meta");
    assert!(
        !meta.working_dir.starts_with(r"\\?\"),
        "the stored path is the display form: {}",
        meta.working_dir
    );
    let read = store.read(&session).expect("read");
    match &read.records[0] {
        lca_protocol::Record::SessionStart { working_dir, .. } => assert!(
            !working_dir.starts_with(r"\\?\"),
            "the record's working_dir is the display form: {working_dir}"
        ),
        other => panic!("first record is the session start: {other:?}"),
    }
}

// Verifies: docs/session-log-format.md §meta.json - the metadata carries
// the model and provider last used (documented since the format's writing
// and never actually stored), updates when they change, and the
// zero-provider state does not overwrite a real one with an empty model.
#[test]
fn meta_records_the_model_and_provider_last_used() {
    let store = store("meta-model");
    let project = scratch("meta-model-project");
    let session = store.create_session(&project, "test").expect("create");

    store
        .record_model_used(&session, "openai-compatible", "m1")
        .expect("record");
    let meta = store.meta(&session).expect("meta");
    assert_eq!(meta.model.as_deref(), Some("m1"));
    assert_eq!(meta.provider.as_deref(), Some("openai-compatible"));

    // A new model replaces it; the same values are a no-op.
    store
        .record_model_used(&session, "openai-compatible", "m1")
        .expect("record again");
    store
        .record_model_used(&session, "openai-compatible", "m2")
        .expect("record2");
    assert_eq!(
        store.meta(&session).expect("meta").model.as_deref(),
        Some("m2")
    );

    // An empty model is the zero-provider state, not "last used".
    store
        .record_model_used(&session, "openai-compatible", "")
        .expect("empty ignored");
    assert_eq!(
        store.meta(&session).expect("meta").model.as_deref(),
        Some("m2")
    );
}
