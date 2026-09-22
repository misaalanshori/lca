//! Session storage tests: append-only log, truncation recovery, listing,
//! forking, the compaction view, and export.
//!
//! Every test runs against a scratch directory; the shared sandboxed-HOME
//! fixture in `lca-testkit` covers environment isolation for tests that
//! touch process state.

use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{ExportOptions, ReadOutcome, SessionStore, ViewMode};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-session-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn store(name: &str) -> SessionStore {
    SessionStore::new(scratch(name))
}

fn user_record(id: &str, content: &str) -> Record {
    Record::User {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        content: content.to_string(),
        attachments: vec![],
    }
}

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

// Verifies: FR-SESS-2 (resume lists sessions for the project, newest first)
#[test]
fn lists_sessions_newest_first() {
    let store = store("list");
    let project = scratch("list-project");
    let _first = store.create_session(&project, "first").expect("create");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = store.create_session(&project, "second").expect("create");
    store
        .append(&second, user_record("r1", "hi"))
        .expect("append");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let _third = store.create_session(&project, "third").expect("create");

    let listed = store.list_sessions(&project).expect("list");
    let titles: Vec<&str> = listed.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, vec!["third", "second", "first"]);
    let second_entry = listed.iter().find(|s| s.title == "second").expect("second");
    assert_eq!(
        second_entry.message_count, 1,
        "index carries the message count"
    );
    assert_eq!(second_entry.id, second.id());
}

// The index.json file is a cache: delete it and listing still works
// (docs/session-log-format.md, layout on disk).
#[test]
fn index_is_rebuilt_when_missing() {
    let store = store("index-rebuild");
    let project = scratch("index-rebuild-project");
    let session = store.create_session(&project, "only").expect("create");
    store
        .append(&session, user_record("r1", "hi"))
        .expect("append");
    std::fs::remove_file(store.index_path(&project)).expect("remove index");
    let listed = store.list_sessions(&project).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].message_count, 1);
}

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

// The resolved view hides records inside a compacted range and shows the
// summary instead; nothing outside the range disappears.
#[test]
fn compaction_view_hides_only_the_replaced_range() {
    let store = store("compact-view");
    let project = scratch("compact-view-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "old one"))
        .expect("a");
    store
        .append(&session, user_record("r2", "old two"))
        .expect("a");
    store
        .append(&session, user_record("r3", "old three"))
        .expect("a");
    store
        .append(
            &session,
            Record::Compaction {
                v: FORMAT_VERSION,
                ts: 4,
                id: "c1".into(),
                replaced_from: "r1".into(),
                replaced_to: "r3".into(),
                summary: "three olds summarized".into(),
                strategy: "compaction-default".into(),
                usage: None,
            },
        )
        .expect("a");
    store.append(&session, user_record("r4", "new")).expect("a");

    let view = store.read_with(&session, ViewMode::Display).expect("view");
    let kinds: Vec<&str> = view.records.iter().map(|r| r.type_tag()).collect();
    assert_eq!(kinds, vec!["session-start", "compaction", "user"]);
    match &view.records[1] {
        Record::Compaction { summary, .. } => assert_eq!(summary, "three olds summarized"),
        other => panic!("expected compaction summary, got {other:?}"),
    }
    assert_eq!(view.records[2].id(), Some("r4"));

    // Audit walks everything: FR-SESS-7 depends on this.
    let audit = store.read_with(&session, ViewMode::Audit).expect("audit");
    assert_eq!(
        audit.records.len(),
        6,
        "session-start + four records + compaction"
    );
}

#[test]
fn nested_compaction_still_resolves_to_one_coherent_view() {
    let store = store("nested");
    let project = scratch("nested-project");
    let session = store.create_session(&project, "test").expect("create");
    for id in ["r1", "r2"] {
        store.append(&session, user_record(id, id)).expect("a");
    }
    store
        .append(
            &session,
            Record::Compaction {
                v: FORMAT_VERSION,
                ts: 3,
                id: "c1".into(),
                replaced_from: "r1".into(),
                replaced_to: "r2".into(),
                summary: "first pair".into(),
                strategy: "s".into(),
                usage: None,
            },
        )
        .expect("a");
    for id in ["r3", "r4"] {
        store.append(&session, user_record(id, id)).expect("a");
    }
    // The outer range covers the inner marker itself (r1 .. c1).
    store
        .append(
            &session,
            Record::Compaction {
                v: FORMAT_VERSION,
                ts: 6,
                id: "c2".into(),
                replaced_from: "r1".into(),
                replaced_to: "c1".into(),
                summary: "everything before r3".into(),
                strategy: "s".into(),
                usage: None,
            },
        )
        .expect("a");

    let view = store.read_with(&session, ViewMode::Display).expect("view");
    // Inner summary is itself replaced by the outer one.
    let summaries: Vec<&str> = view
        .records
        .iter()
        .filter_map(|r| match r {
            Record::Compaction { summary, .. } => Some(summary.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(summaries, vec!["everything before r3"]);
    let users: Vec<&str> = view
        .records
        .iter()
        .filter_map(|r| match r {
            Record::User { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        users,
        vec!["r3", "r4"],
        "records outside every range survive"
    );
}

// Verifies: FR-SESS-7 (export strips permission and extension-event records
// unless the audit flag is passed)
#[test]
fn export_strips_sensitive_records_unless_audited() {
    let store = store("export");
    let project = scratch("export-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "hello"))
        .expect("a");
    store
        .append(
            &session,
            Record::Permission {
                v: FORMAT_VERSION,
                ts: 2,
                id: "p1".into(),
                action: "run rm -rf /".into(),
                decision: lca_session::PermissionDecision::Always,
                pattern: Some("rm -rf *".into()),
            },
        )
        .expect("a");
    store
        .append(
            &session,
            Record::ExtensionEvent {
                v: FORMAT_VERSION,
                ts: 3,
                id: "e1".into(),
                extension: "evil".into(),
                event: "denied".into(),
                detail: "tried credentials".into(),
            },
        )
        .expect("a");

    let plain = store
        .export(&session, ExportOptions::default())
        .expect("export");
    let text = std::fs::read_to_string(&plain).expect("read export");
    assert!(text.contains("hello"));
    assert!(!text.contains("rm -rf"), "permission records stripped");
    assert!(
        !text.contains("tried credentials"),
        "extension events stripped"
    );

    let audited = store
        .export(&session, ExportOptions { audit: true })
        .expect("export");
    let text = std::fs::read_to_string(&audited).expect("read export");
    assert!(text.contains("rm -rf"));
    assert!(text.contains("tried credentials"));
}

// Verifies: FR-CFG-5 (nothing in the credential store path is written to the
// session log; guards the export shape against regressions)
#[test]
fn export_carries_meta_records_and_no_extra_state() {
    let store = store("export-shape");
    let project = scratch("export-shape-project");
    let session = store.create_session(&project, "test").expect("create");
    store.append(&session, user_record("r1", "hi")).expect("a");
    let path = store
        .export(&session, ExportOptions::default())
        .expect("export");
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json");
    assert!(value.get("meta").is_some(), "metadata present");
    assert!(value.get("records").is_some(), "resolved records present");
    for key in ["credentials", "tokens", "api_key"] {
        assert!(value.get(key).is_none(), "{key} never exported");
    }
}
