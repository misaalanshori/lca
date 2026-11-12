//! Append-path rows (FR-SESS-1): one record is one JSON line, appending never
//! touches earlier lines, and the session-start/session-end markers carry the
//! contract's version and working directory.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::common::{scratch, store, user_record};

// Verifies: FR-SESS-1 (every session is an append-only log on disk)
#[test]
fn appends_records_as_one_json_line_each() {
    let store = store("append");
    let project = scratch("append-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "first"))
        .expect("append1");
    store
        .append(&session, user_record("r2", "second"))
        .expect("append2");

    let log = std::fs::read_to_string(session.log_path()).expect("read log");
    let lines: Vec<&str> = log.lines().collect();
    // session-start from create_session, then the two appends.
    assert_eq!(lines.len(), 3, "one line per record, no rewrites");
    assert!(
        lines[1].contains(r#""t":"user""#),
        "second line is the first appended record"
    );
    assert!(
        lines[0].ends_with('}'),
        "line terminated, no partial framing"
    );
    assert!(log.ends_with('\n'));
}

// Verifies: FR-SESS-1 (records are never rewritten in place)
#[test]
fn appending_never_touches_earlier_lines() {
    let store = store("immutable");
    let project = scratch("immutable-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "first"))
        .expect("append1");
    let before = std::fs::read_to_string(session.log_path()).expect("read");
    store
        .append(&session, user_record("r2", "second"))
        .expect("append2");
    let after = std::fs::read_to_string(session.log_path()).expect("read");
    assert!(after.starts_with(&before), "prefix preserved byte for byte");
}

// Verifies: session-log-format - a clean exit writes a `session-end` marker
// (its absence means the session ended without one, normal after a crash).
#[test]
fn close_writes_the_session_end_marker() {
    let store = store("session-end");
    let project = scratch("session-end-project");
    let session = store.create_session(&project, "test").expect("create");
    store.close(&session).expect("close");
    let read = store.read(&session).expect("read");
    assert!(
        read.records
            .iter()
            .any(|r| matches!(r, lca_protocol::Record::SessionEnd { .. })),
        "session-end is on record"
    );
}

// Verifies: session-log-format - the session-start record carries the
// extension ABI version from the contract crate. It used to be a stale local
// "0.1" while `lca:ext` was frozen at "1.0".
#[test]
fn session_start_records_the_contract_abi_version() {
    let store = store("abi-version");
    let project = scratch("abi-version-project");
    let session = store.create_session(&project, "test").expect("create");
    match store.raw_start(&session).expect("start") {
        lca_protocol::Record::SessionStart { abi_version, .. } => {
            assert_eq!(abi_version, lca_ext_abi::ABI_VERSION);
            assert_eq!(abi_version, "0.6");
        }
        other => panic!("not a session-start: {other:?}"),
    }
}
