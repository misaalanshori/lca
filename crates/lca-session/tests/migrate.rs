//! Session migration rows (gh #98): version detection, linkage
//! backfill with backup, lossless round-trips, and refusals.
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

/// A pre-linkage log: lines written straight to disk, parents never
/// stamped (what `append` would have filled since gh #37).
fn write_raw_log(session: &lca_session::Session, lines: &[String]) {
    let mut text = lines.join("\n");
    text.push('\n');
    std::fs::write(session.log_path(), text).expect("write log");
}

fn record_line(record: &Record) -> String {
    serde_json::to_string(record).expect("serialize")
}

// Verifies: gh #98 - a current log reports its stamps with nothing
// to do: meta, header, and max record versions agree at current.
#[test]
fn version_state_reports_a_current_log() {
    let store = store("migrate-current");
    let project = scratch("migrate-current-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("a", "hi"))
        .expect("append");

    let state = store.version_state(&session).expect("state");
    assert_eq!(state.meta_version, Some(FORMAT_VERSION));
    assert_eq!(state.header_version, Some(FORMAT_VERSION));
    assert_eq!(state.max_record_version, FORMAT_VERSION);
    assert_eq!(state.parentless, 0, "appends stamp linkage");
    assert!(!state.truncated);
    assert!(state.is_current());
}

// Verifies: gh #98 - a pre-linkage log is detected: linkable records
// without parents count up.
#[test]
fn version_state_spots_a_pre_linkage_log() {
    let store = store("migrate-old");
    let project = scratch("migrate-old-project");
    let session = store.create_session(&project, "test").expect("create");
    let start = store.raw_start(&session).expect("start");
    write_raw_log(
        &session,
        &[
            record_line(&start),
            record_line(&user_record("a", "first")),
            record_line(&user_record("b", "second")),
        ],
    );

    let state = store.version_state(&session).expect("state");
    assert_eq!(
        state.parentless, 1,
        "only the second row has an id to point at"
    );
    assert!(!state.is_current());
}

// Verifies: gh #98 - a meta/header version mismatch is reported,
// never silently picked.
#[test]
fn version_state_flags_a_meta_header_mismatch() {
    let store = store("migrate-mismatch");
    let project = scratch("migrate-mismatch-project");
    let session = store.create_session(&project, "test").expect("create");
    let mut meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(session.meta_path()).expect("read meta"))
            .expect("parse meta");
    meta["format_version"] = serde_json::json!(FORMAT_VERSION + 1);
    std::fs::write(
        session.meta_path(),
        serde_json::to_string_pretty(&meta).expect("serialize"),
    )
    .expect("write meta");

    let state = store.version_state(&session).expect("state");
    assert!(state.mismatched, "the versions disagree");
}

// Verifies: gh #98 - unknown-future records count up while the rest
// of the log stays readable (warn-and-skip, pinned).
#[test]
fn version_state_counts_unknown_future_records() {
    let store = store("migrate-future");
    let project = scratch("migrate-future-project");
    let session = store.create_session(&project, "test").expect("create");
    let start = store.raw_start(&session).expect("start");
    let future = format!(
        r#"{{"v":{},"t":"user","ts":3,"id":"f","content":"from tomorrow"}}"#,
        FORMAT_VERSION + 1
    );
    write_raw_log(
        &session,
        &[
            record_line(&start),
            record_line(&user_record("a", "today")),
            future,
        ],
    );

    let state = store.version_state(&session).expect("state");
    assert_eq!(state.unknown_future, 1);
    let outcome = store.read(&session).expect("read");
    assert_eq!(outcome.records.len(), 2, "the future line skips");
    assert!(!outcome.warnings.is_empty(), "the skip warns");
}

// Verifies: gh #98 - backfill stamps linkage, keeps a backup, and
// reads back identical views (lossless round-trip).
#[test]
fn migrate_backfills_linkage_losslessly() {
    let store = store("migrate-backfill");
    let project = scratch("migrate-backfill-project");
    let session = store.create_session(&project, "test").expect("create");
    let start = store.raw_start(&session).expect("start");
    write_raw_log(
        &session,
        &[
            record_line(&start),
            record_line(&user_record("a", "first")),
            record_line(&user_record("b", "second")),
        ],
    );
    let before_display = store
        .resolved(&session, ViewMode::Display)
        .expect("display")
        .records
        .iter()
        .filter_map(|record| record.id().map(str::to_string))
        .collect::<Vec<_>>();
    let before_audit = store
        .read(&session)
        .expect("audit")
        .records
        .iter()
        .filter_map(|record| record.id().map(str::to_string))
        .collect::<Vec<_>>();

    let report = store.migrate(&session).expect("migrate");
    assert_eq!(
        report.stamped, 1,
        "the second row links; the first has no id to point at"
    );
    assert!(!report.already_current);
    let backup = report.backup.expect("a backup is kept");
    assert!(backup.is_file(), "the original survives");

    // The rewritten lines chain: a after start, b after a.
    let after = store.read(&session).expect("re-read").records;
    let parent_of = |id: &str| {
        after
            .iter()
            .find(|record| record.id() == Some(id))
            .and_then(|record| record.parent().map(str::to_string))
    };
    assert_eq!(
        parent_of("a"),
        None,
        "leading records stay parentless, like appends"
    );
    assert_eq!(parent_of("b").as_deref(), Some("a"));

    // Lossless: the same records in the same order everywhere.
    let after_display = store
        .resolved(&session, ViewMode::Display)
        .expect("display")
        .records
        .iter()
        .filter_map(|record| record.id().map(str::to_string))
        .collect::<Vec<_>>();
    let after_audit = after
        .iter()
        .filter_map(|record| record.id().map(str::to_string))
        .collect::<Vec<_>>();
    assert_eq!(before_display, after_display, "display identical");
    assert_eq!(before_audit, after_audit, "audit identical");

    // And the tree feature works on the migrated log now.
    let rows = store.entry_tree(&session).expect("tree");
    assert!(
        rows.iter().any(|row| row.depth > 0),
        "linkage nests: {rows:?}"
    );

    // A second run is a no-op.
    let again = store.migrate(&session).expect("migrate again");
    assert!(again.already_current);
    assert_eq!(again.stamped, 0);
}

// Verifies: gh #98 - mismatched and truncated logs refuse with a
// message instead of rewriting what cannot be verified.
#[test]
fn migrate_refuses_what_it_cannot_verify() {
    let store1 = store("migrate-refuse");
    let project = scratch("migrate-refuse-project");
    let session = store1.create_session(&project, "test").expect("create");
    let mut meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(session.meta_path()).expect("read meta"))
            .expect("parse meta");
    meta["format_version"] = serde_json::json!(FORMAT_VERSION + 1);
    std::fs::write(
        session.meta_path(),
        serde_json::to_string_pretty(&meta).expect("serialize"),
    )
    .expect("write meta");
    let err = store1.migrate(&session).expect_err("mismatch refuses");
    assert!(err.to_string().contains("mismatch"), "it says why: {err}");

    let store2 = store("migrate-truncated");
    let project = scratch("migrate-truncated-project");
    let session = store2.create_session(&project, "test").expect("create");
    let start = store2.raw_start(&session).expect("start");
    write_raw_log(
        &session,
        &[record_line(&start), "{\"v\":1,\"t\":\"user\"".to_string()],
    );
    let err = store2.migrate(&session).expect_err("truncation refuses");
    assert!(err.to_string().contains("truncat"), "it says why: {err}");
}

// Verifies: gh #98 - unknown-future lines ride a migration through
// byte-identical (kept, warned, never rewritten).
#[test]
fn migrate_preserves_unknown_future_lines_verbatim() {
    let store = store("migrate-keep-future");
    let project = scratch("migrate-keep-future-project");
    let session = store.create_session(&project, "test").expect("create");
    let start = store.raw_start(&session).expect("start");
    let future = format!(
        r#"{{"v":{},"t":"user","ts":3,"id":"f","content":"from tomorrow"}}"#,
        FORMAT_VERSION + 1
    );
    write_raw_log(
        &session,
        &[
            record_line(&start),
            record_line(&user_record("a", "today")),
            record_line(&user_record("b", "tomorrow")),
            future.clone(),
        ],
    );

    let report = store.migrate(&session).expect("migrate");
    assert_eq!(report.stamped, 1, "the second row still links");
    assert!(!report.warnings.is_empty(), "the keep warns");
    let text = std::fs::read_to_string(session.log_path()).expect("log");
    assert!(
        text.lines().any(|line| line == future),
        "the future line survives byte-identical"
    );
}
