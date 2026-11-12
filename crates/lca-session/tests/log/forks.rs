//! The fork/clone rows (FR-SESS-11): a child session links to the record it
//! branched at, listings count the resolved history, and a reference opens by
//! id or by path.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::common::{scratch, store, user_record};
use lca_protocol::Record;
use lca_session::ReadOutcome;

// Verifies: FR-SESS-3 (fork creates a new session sharing history up to a
// message; the parent's log is not copied)
#[test]
fn fork_creates_a_child_pointing_at_the_parent_record() {
    let store = store("fork");
    let project = scratch("fork-project");
    let parent = store.create_session(&project, "parent").expect("create");
    store
        .append(&parent, user_record("r1", "one"))
        .expect("append");
    store
        .append(&parent, user_record("r2", "two"))
        .expect("append");

    let child = store.fork(&parent, "r1").expect("fork");
    assert_ne!(child.id(), parent.id());
    let child_meta = store.meta(&child).expect("meta");
    assert_eq!(child_meta.parent_session.as_deref(), Some(parent.id()));
    assert_eq!(child_meta.parent_record.as_deref(), Some("r1"));

    let ReadOutcome { records, .. } = store.read(&child).expect("read child");
    assert_eq!(records.len(), 2, "session-start and fork-point only");
    assert!(matches!(&records[0], Record::SessionStart { .. }));
    match &records[1] {
        Record::ForkPoint {
            parent_session,
            record_id,
            ..
        } => {
            assert_eq!(parent_session, parent.id());
            assert_eq!(record_id, "r1");
        }
        other => panic!("expected fork-point, got {other:?}"),
    }

    let parent_log = std::fs::read_to_string(parent.log_path()).expect("parent log");
    assert_eq!(
        parent_log.lines().count(),
        3,
        "parent untouched by the fork"
    );
}

// Verifies: FR-SESS-3 (a fork's listing counts its resolved, inherited
// history, not just its own two framing records)
#[test]
fn a_forked_sessions_listing_counts_the_resolved_history() {
    let store = store("fork-listing");
    let project = scratch("fork-listing-project");
    let parent = store.create_session(&project, "parent").expect("create");
    store
        .append(&parent, user_record("r1", "one"))
        .expect("append");
    store
        .append(&parent, user_record("r2", "two"))
        .expect("append");
    let child = store.fork(&parent, "r1").expect("fork");

    let listed = store.list_sessions(&project).expect("list");
    let row = |id: &str| {
        listed
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("session {id} listed"))
    };
    assert_eq!(
        row(parent.id()).message_count,
        2,
        "the parent counts its two messages"
    );
    assert_eq!(
        row(child.id()).message_count,
        1,
        "the fork counts the inherited history (r1), not its own empty tail"
    );
}

// Verifies: gh #205 - `/clone` duplicates the session at its tip: the
// fork-point names the last record, the title defaults to
// "Clone of <parent>", and the parent's log is untouched.
#[test]
fn clone_duplicates_the_session_at_its_tip() {
    let store = store("clone");
    let project = scratch("clone-project");
    let parent = store.create_session(&project, "parent").expect("create");
    store
        .append(&parent, user_record("r1", "one"))
        .expect("append");
    store
        .append(&parent, user_record("r2", "two"))
        .expect("append");

    let child = store
        .clone_session(&parent, Some("experimental"))
        .expect("clone");
    assert_ne!(child.id(), parent.id());
    let child_meta = store.meta(&child).expect("meta");
    assert_eq!(child_meta.parent_session.as_deref(), Some(parent.id()));
    assert_eq!(child_meta.parent_record.as_deref(), Some("r2"));
    assert_eq!(store.meta(&child).expect("meta").title, "experimental");

    let ReadOutcome { records, .. } = store.read(&child).expect("read child");
    // session-start, fork-point, and the clone titling's session-info
    // (gh #37: renames ride entries now).
    assert_eq!(records.len(), 3, "start, fork-point, and session-info");
    match &records[1] {
        Record::ForkPoint { record_id, .. } => assert_eq!(record_id, "r2"),
        other => panic!("expected fork-point, got {other:?}"),
    }
    match &records[2] {
        Record::SessionInfo { name, .. } => assert_eq!(name, "experimental"),
        other => panic!("expected session-info, got {other:?}"),
    }
    let parent_log = std::fs::read_to_string(parent.log_path()).expect("parent log");
    assert_eq!(
        parent_log.lines().count(),
        3,
        "parent untouched by the clone"
    );
}

// Verifies: gh #205 - an unnamed clone titles itself after the parent.
#[test]
fn an_unnamed_clone_titles_itself_after_the_parent() {
    let store = store("clone-default");
    let project = scratch("clone-default-project");
    let parent = store.create_session(&project, "parent").expect("create");
    store
        .append(&parent, user_record("r1", "one"))
        .expect("append");
    let child = store.clone_session(&parent, None).expect("clone");
    assert_eq!(store.meta(&child).expect("meta").title, "Clone of parent");
}

// Verifies: gh #110 - a session reference opens by path (a directory
// holding meta.json) as well as by id.
#[test]
fn a_session_reference_opens_by_path_or_id() {
    let store = store("session-ref");
    let project = scratch("session-ref-project");
    let session = store.create_session(&project, "ref").expect("create");
    let by_id = store.session_ref(&project, session.id()).expect("by id");
    assert_eq!(by_id.id(), session.id());
    let dir = session
        .log_path()
        .parent()
        .expect("session dir")
        .to_string_lossy()
        .into_owned();
    let by_path = store.session_ref(&project, &dir).expect("by path");
    assert_eq!(by_path.id(), session.id());
    assert!(store.session_ref(&project, "nope").is_err(), "unknown");
}

// Verifies: gh #75 - trashing moves the session directory under a
// timestamped trash directory (recoverable, never `rm`), and the
// session lists no more.
#[test]
fn trash_moves_the_session_directory_to_recover() {
    let store = store("trash");
    let project = scratch("trash-project");
    let session = store.create_session(&project, "test").expect("create");
    let id = session.id().to_string();
    assert!(session.dir().is_dir());

    let trashed = store.trash(&session).expect("trash");
    assert!(!session.dir().exists(), "the session directory is gone");
    assert!(
        trashed.is_dir() && trashed.join("meta.json").is_file(),
        "the whole directory survives under trash: {}",
        trashed.display()
    );
    let ids: Vec<String> = store
        .list_sessions(&project)
        .expect("list")
        .into_iter()
        .map(|summary| summary.id)
        .collect();
    assert!(!ids.contains(&id), "trashed sessions list no more");
}
