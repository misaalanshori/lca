//! FR-SESS-6's rows: a corrupt or partial line keeps the prefix and reports
//! truncation, an unknown tag is a skip, and a known tag with bad fields is not.
//!
//! These live together because they are one rule seen from five angles: the
//! reader never invents a record, never throws the prefix away, and tells the
//! difference between "a newer agent wrote this" and "this line is broken".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::common::{scratch, store, user_record};
use lca_session::ReadOutcome;

// Verifies: FR-SESS-6 (a record that fails to parse stops the read and the
// records before it load)
#[test]
fn corrupt_record_keeps_the_prefix_and_reports_truncation() {
    let store = store("corrupt");
    let project = scratch("corrupt-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "kept"))
        .expect("append");
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("open");
        file.write_all(b"{\"v\":1,\"t\":\"user\", oops\n")
            .expect("write junk");
    }
    store
        .append(&session, user_record("r2", "never loads"))
        .expect("append after junk");

    let ReadOutcome {
        records, truncated, ..
    } = store.read(&session).expect("read");
    assert!(truncated, "truncation reported");
    assert_eq!(
        records.len(),
        2,
        "session-start and r1 load; nothing after the junk"
    );
    assert_eq!(records[1].id(), Some("r1"));
}

// Verifies: FR-SESS-6 (a crash mid-write leaves a partial final line; the
// reader discards it)
#[test]
fn partial_final_line_is_discarded_as_the_longest_valid_prefix() {
    let store = store("partial");
    let project = scratch("partial-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "kept"))
        .expect("append");
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("open");
        file.write_all(br#"{"v":1,"t":"user","ts":2,"id":"r2","cont"#)
            .expect("write partial");
    }
    let ReadOutcome {
        records, truncated, ..
    } = store.read(&session).expect("read");
    assert!(truncated);
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].id(), Some("r1"));
}

// Unknown record types are skipped so a newer agent's log stays readable
// (docs/session-log-format.md, reading and error handling).
#[test]
fn unknown_record_types_are_skipped_not_fatal() {
    let store = store("unknown");
    let project = scratch("unknown-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "kept"))
        .expect("append");
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("open");
        file.write_all(b"{\"v\":1,\"t\":\"from-the-future\",\"ts\":3,\"id\":\"r2\"}\n")
            .expect("write");
    }
    store
        .append(&session, user_record("r3", "also kept"))
        .expect("append");

    let ReadOutcome {
        records,
        truncated,
        skipped_unknown,
        ..
    } = store.read(&session).expect("read");
    assert!(!truncated, "unknown types are not corruption");
    assert_eq!(skipped_unknown, 1);
    assert_eq!(records.len(), 3, "records around the unknown one load");
    assert_eq!(records[2].id(), Some("r3"));
}

// Verifies: FR-SESS-6 - a *known* record type that fails to deserialize is
// corruption, so the reader stops and reports truncation rather than
// silently dropping the record (the bug the old text-split tag classifier
// hid, because it always yielded the key `t` instead of the type value).
#[test]
fn a_known_record_type_with_bad_fields_reports_truncation() {
    let store = store("known-bad");
    let project = scratch("known-bad-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "kept"))
        .expect("append");
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("open");
        // `user` requires `content`; omitting it makes deserialization fail.
        file.write_all(b"{\"v\":1,\"t\":\"user\",\"ts\":3,\"id\":\"r2\"}\n")
            .expect("write");
    }
    store
        .append(&session, user_record("r3", "after"))
        .expect("append");

    let ReadOutcome {
        records,
        truncated,
        skipped_unknown,
        warnings,
    } = store.read(&session).expect("read");
    assert!(truncated, "a known type with bad fields is corruption");
    assert_eq!(skipped_unknown, 0, "it is not an unknown type");
    assert_eq!(records.len(), 2, "records before the bad line are kept");
    assert_eq!(records[1].id(), Some("r1"));
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("known record type")),
        "the warning names the known type: {warnings:?}"
    );
}

#[test]
fn records_with_a_higher_version_are_skipped_with_a_warning() {
    let store = store("future-v");
    let project = scratch("future-v-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "kept"))
        .expect("append");
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("open");
        file.write_all(b"{\"v\":99,\"t\":\"user\",\"ts\":3,\"id\":\"r2\",\"content\":\"x\"}\n")
            .expect("write");
    }
    let ReadOutcome {
        records,
        truncated,
        skipped_unknown,
        ..
    } = store.read(&session).expect("read");
    assert!(!truncated);
    assert_eq!(skipped_unknown, 1);
    assert_eq!(records.len(), 2);
}
