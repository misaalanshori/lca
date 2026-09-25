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

// A project with no sessions yet must list an empty set, not fail while
// creating the index cache (FR-SESS-2's first-run case).
#[test]
fn listing_a_project_with_no_sessions_is_empty_not_an_error() {
    let store = store("empty-list");
    let project = scratch("empty-list-project");
    let listed = store.list_sessions(&project).expect("empty project lists");
    assert!(listed.is_empty(), "no sessions yet: {listed:?}");
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

// Verifies: docs/session-log-format.md's attachments sidecar: a record's
// attachment hash is listed in the export's `attachments` map with its
// sidecar path, and a hash with no file on disk is not listed.
#[test]
fn export_lists_attachment_hashes_that_exist() {
    let store = store("export-attachments");
    let project = scratch("export-attachments-project");
    let session = store.create_session(&project, "test").expect("create");
    let dir = session.dir().join("attachments");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let present = "a".repeat(64);
    let missing = "b".repeat(64);
    std::fs::write(dir.join(&present), "full text").expect("write");
    for (id, call_id, hash) in [("t1", "c1", &present), ("t2", "c2", &missing)] {
        store
            .append(
                &session,
                Record::ToolResult {
                    v: FORMAT_VERSION,
                    ts: 2,
                    id: id.into(),
                    call_id: call_id.into(),
                    status: lca_protocol::ToolResultStatus::Ok,
                    content: Some("truncated".into()),
                    attachment: Some(hash.clone()),
                    truncated: true,
                },
            )
            .expect("append");
    }

    let path = store
        .export(&session, ExportOptions::default())
        .expect("export");
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json");
    let attachments = value
        .get("attachments")
        .and_then(|v| v.as_object())
        .expect("the export carries an attachments map");
    assert_eq!(
        attachments.get(&present).and_then(|v| v.as_str()),
        Some(format!("attachments/{present}").as_str())
    );
    assert!(
        attachments.get(&missing).is_none(),
        "a referenced hash with no file is not listed"
    );
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
    store.rename(&session, "new title").expect("rename");
    assert_eq!(store.meta(&session).expect("meta").title, "new title");
    let listed = store.list_sessions(&project).expect("list");
    assert_eq!(listed[0].title, "new title");
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
            assert_eq!(abi_version, "0.2");
        }
        other => panic!("not a session-start: {other:?}"),
    }
}

fn attachment_record(id: &str, hash: &str) -> Record {
    Record::ToolResult {
        v: FORMAT_VERSION,
        ts: 2,
        id: id.to_string(),
        call_id: format!("{id}-call"),
        status: lca_protocol::ToolResultStatus::Ok,
        content: Some("[full output in an attachment]".to_string()),
        attachment: Some(hash.to_string()),
        truncated: true,
    }
}

fn write_attachment(session: &lca_session::Session, hash: &str, bytes: &str) {
    let dir = session.dir().join("attachments");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join(hash), bytes).expect("write attachment");
}

// Verifies: docs/session-log-format.md § Fork — a fork copies records, not
// the content they reference; resolution walks the chain to the ancestor
// that wrote the attachment, and the export names its real home.
#[test]
fn a_forks_records_resolve_the_parents_attachment() {
    let store = store("fork-attachments");
    let project = scratch("fork-attachments-project");
    let parent = store.create_session(&project, "parent").expect("create");
    let hash = "a".repeat(64);
    write_attachment(&parent, &hash, "the full output");
    store
        .append(&parent, attachment_record("r1", &hash))
        .expect("append");

    let child = store.fork(&parent, "r1").expect("fork");
    assert!(
        !child.dir().join("attachments").join(&hash).exists(),
        "a fork does not copy the parent's attachment bytes"
    );
    let resolved = store.attachment_path(&child, &hash).expect("resolve");
    assert_eq!(
        resolved,
        parent.dir().join("attachments").join(&hash),
        "resolution finds the home session's file"
    );

    let path = store
        .export(&child, ExportOptions::default())
        .expect("export");
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json");
    let listed = value
        .get("attachments")
        .and_then(|v| v.get(&hash))
        .and_then(|v| v.as_str());
    assert_eq!(
        listed,
        Some(format!("../{}/attachments/{hash}", parent.id()).as_str()),
        "the child's export points at the ancestor that owns the bytes"
    );
}

// Verifies: D5 — the mark-and-sweep deletes an attachment only a
// compaction-suppressed record still referenced, and never one a resolved
// record list still references.
#[test]
fn gc_collects_compaction_orphans_and_keeps_referenced_files() {
    let store = store("gc-compaction");
    let project = scratch("gc-compaction-project");
    let session = store.create_session(&project, "test").expect("create");
    let orphan = "c".repeat(64);
    let kept = "d".repeat(64);
    write_attachment(&session, &orphan, "compacted away");
    write_attachment(&session, &kept, "still referenced");
    store
        .append(&session, attachment_record("r1", &orphan))
        .expect("a");
    store
        .append(&session, attachment_record("r2", &kept))
        .expect("a");
    store
        .append(
            &session,
            Record::Compaction {
                v: FORMAT_VERSION,
                ts: 3,
                id: "c1".into(),
                replaced_from: "r1".into(),
                replaced_to: "r1".into(),
                summary: "r1 summarized".into(),
                strategy: "s".into(),
                usage: None,
            },
        )
        .expect("a");

    let deleted = store.gc(&session).expect("gc");
    assert_eq!(deleted, vec![orphan.clone()], "only the orphan is swept");
    assert!(
        !session.dir().join("attachments").join(&orphan).exists(),
        "the compaction orphan is gone"
    );
    assert!(
        session.dir().join("attachments").join(&kept).exists(),
        "a referenced attachment survives"
    );
}

// Verifies: D5 — the sweep is chain-aware: GC on a child keeps an ancestor's
// referenced attachment (a sibling branch may need it) and still collects an
// orphan in the ancestor's own directory.
#[test]
fn gc_from_a_child_keeps_an_ancestors_referenced_attachment() {
    let store = store("gc-child");
    let project = scratch("gc-child-project");
    let parent = store.create_session(&project, "parent").expect("create");
    let referenced = "e".repeat(64);
    let orphan = "f".repeat(64);
    write_attachment(&parent, &referenced, "referenced");
    write_attachment(&parent, &orphan, "orphan");
    store
        .append(&parent, attachment_record("r1", &referenced))
        .expect("a");
    let child = store.fork(&parent, "r1").expect("fork");

    let deleted = store.gc(&child).expect("gc");
    assert_eq!(deleted, vec![orphan.clone()], "the parent orphan is swept");
    assert!(
        parent.dir().join("attachments").join(&referenced).exists(),
        "the referenced ancestor attachment survives a child's GC"
    );
    assert!(
        !parent.dir().join("attachments").join(&orphan).exists(),
        "the orphan in the ancestor directory is collected"
    );
}
