//! The session vocabulary v2 (gh #47 / EFG-013): the new record types
//! append and read back, the unknown-`t` skip rule holds with them
//! present, a malformed known-new-type line is corruption (not a skip),
//! and the export rules strip the audit-only types by default.
//!
//! Split into its own file the way `model_change.rs` was: `log.rs`
//! already crossed the ceiling once.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{ExportOptions, ReadOutcome, SessionStore};

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

fn store(name: &str) -> SessionStore {
    SessionStore::new(scratch(name))
}

fn usage_record(id: &str) -> Record {
    Record::Usage {
        v: FORMAT_VERSION,
        ts: 2,
        id: id.to_string(),
        parent: None,
        kind: "cache_warm".into(),
        provider: Some("openai-compatible".into()),
        model: Some("m1".into()),
        usage: lca_protocol::Usage::default(),
    }
}

// Verifies: gh #47 (the v2 vocabulary appends): every new record type
// writes as one line and reads back as itself.
#[test]
fn the_new_vocabulary_appends_and_reads_back() {
    let store = store("vocabulary");
    let project = scratch("vocabulary-project");
    let session = store.create_session(&project, "test").expect("create");
    let records = vec![
        Record::ThinkingLevelChange {
            v: FORMAT_VERSION,
            ts: 2,
            id: "t1".into(),
            parent: None,
            level: "high".into(),
        },
        usage_record("u1"),
        Record::Label {
            v: FORMAT_VERSION,
            ts: 3,
            id: "l1".into(),
            parent: None,
            target_id: "r1".into(),
            label: Some("checkpoint-1".into()),
        },
        Record::SessionInfo {
            v: FORMAT_VERSION,
            ts: 4,
            id: "s1".into(),
            parent: None,
            name: "Refactor auth module".into(),
        },
        Record::Custom {
            v: FORMAT_VERSION,
            ts: 5,
            id: "c1".into(),
            parent: None,
            custom_type: "my-extension".into(),
            data: serde_json::json!({"count": 42}),
        },
        Record::CustomMessage {
            v: FORMAT_VERSION,
            ts: 6,
            id: "m1".into(),
            parent: None,
            custom_type: "my-extension".into(),
            content: "Injected context...".into(),
            display: true,
            details: None,
        },
        Record::ContextEdit {
            v: FORMAT_VERSION,
            ts: 7,
            id: "e1".into(),
            parent: None,
            target_id: "r1".into(),
            replacement: None,
        },
    ];
    for record in &records {
        store.append(&session, record.clone()).expect("append");
    }
    let ReadOutcome {
        records: read,
        truncated,
        skipped_unknown,
        ..
    } = store.read(&session).expect("read");
    assert!(!truncated);
    assert_eq!(skipped_unknown, 0, "the new types are known");
    let mut expected = vec![store.raw_start(&session).expect("start")];
    expected.extend(records);
    assert_eq!(read, expected);
}

// Verifies: gh #47 (the unknown-`t` skip rule is what makes this
// additive): a log mixing old types, new types, and a genuinely unknown
// tag loads everything known and skips exactly the unknown line, with
// truncation behavior unchanged.
#[test]
fn new_types_parse_while_genuinely_unknown_types_skip() {
    let store = store("vocabulary-skip");
    let project = scratch("vocabulary-skip-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(
            &session,
            Record::User {
                v: FORMAT_VERSION,
                ts: 1,
                id: "r1".into(),
                parent: None,
                content: "kept".into(),
                attachments: vec![],
                queue: None,
            },
        )
        .expect("append");
    store.append(&session, usage_record("u1")).expect("append");
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
        .append(
            &session,
            Record::Label {
                v: FORMAT_VERSION,
                ts: 4,
                id: "l1".into(),
                parent: None,
                target_id: "r1".into(),
                label: None,
            },
        )
        .expect("append");

    let ReadOutcome {
        records,
        truncated,
        skipped_unknown,
        ..
    } = store.read(&session).expect("read");
    assert!(!truncated, "unknown types are not corruption");
    assert_eq!(skipped_unknown, 1, "only the future tag skips");
    assert_eq!(records.len(), 4, "start, user, usage, label");
    assert!(records.iter().any(|r| r.id() == Some("u1")));
    assert!(records.iter().any(|r| r.id() == Some("l1")));
}

// Verifies: gh #47 (known types are held to their shape): a malformed
// new-type line is corruption the reader stops at, exactly like the
// pre-existing types - and `model-change` (gh #8) finally joins that
// set instead of skipping silently.
#[test]
fn malformed_new_types_are_corruption_not_skips() {
    for (name, line) in [
        ("usage", "{\"v\":1,\"t\":\"usage\",\"ts\":3,\"id\":\"r2\"}"),
        (
            "model-change",
            "{\"v\":1,\"t\":\"model-change\",\"ts\":3,\"id\":\"r2\"}",
        ),
    ] {
        let store = store(&format!("vocabulary-bad-{name}"));
        let project = scratch(&format!("vocabulary-bad-{name}-project"));
        let session = store.create_session(&project, "test").expect("create");
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(session.log_path())
                .expect("open");
            file.write_all(line.as_bytes()).expect("write");
            file.write_all(b"\n").expect("write");
        }
        let ReadOutcome {
            truncated,
            skipped_unknown,
            warnings,
            ..
        } = store.read(&session).expect("read");
        assert!(truncated, "a malformed {name} line is corruption");
        assert_eq!(skipped_unknown, 0, "a malformed {name} line is not a skip");
        assert!(
            warnings.iter().any(|w| w.contains("known record type")),
            "the warning names the known type: {warnings:?}"
        );
        let _ = std::fs::remove_dir_all(scratch(&format!("vocabulary-bad-{name}")));
        let _ = std::fs::remove_dir_all(scratch(&format!("vocabulary-bad-{name}-project")));
    }
}

// Verifies: FR-SESS-7 (the per-type export rules): `usage` and `custom`
// are audit-only like `permission` and `extension-event`; the other new
// types survive a default export.
#[test]
fn export_strips_audit_only_vocabulary_unless_audited() {
    let store = store("vocabulary-export");
    let project = scratch("vocabulary-export-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(
            &session,
            Record::User {
                v: FORMAT_VERSION,
                ts: 1,
                id: "r1".into(),
                parent: None,
                content: "hello".into(),
                attachments: vec![],
                queue: None,
            },
        )
        .expect("a");
    store.append(&session, usage_record("u1")).expect("a");
    store
        .append(
            &session,
            Record::Custom {
                v: FORMAT_VERSION,
                ts: 3,
                id: "c1".into(),
                parent: None,
                custom_type: "my-extension".into(),
                data: serde_json::json!({"router": "state"}),
            },
        )
        .expect("a");
    store
        .append(
            &session,
            Record::CustomMessage {
                v: FORMAT_VERSION,
                ts: 4,
                id: "m1".into(),
                parent: None,
                custom_type: "my-extension".into(),
                content: "Injected context...".into(),
                display: false,
                details: None,
            },
        )
        .expect("a");
    store
        .append(
            &session,
            Record::ThinkingLevelChange {
                v: FORMAT_VERSION,
                ts: 5,
                id: "t1".into(),
                parent: None,
                level: "high".into(),
            },
        )
        .expect("a");
    store
        .append(
            &session,
            Record::Label {
                v: FORMAT_VERSION,
                ts: 6,
                id: "l1".into(),
                parent: None,
                target_id: "r1".into(),
                label: Some("checkpoint-1".into()),
            },
        )
        .expect("a");
    store
        .append(
            &session,
            Record::SessionInfo {
                v: FORMAT_VERSION,
                ts: 7,
                id: "s1".into(),
                parent: None,
                name: "Refactor auth module".into(),
            },
        )
        .expect("a");
    store
        .append(
            &session,
            Record::ContextEdit {
                v: FORMAT_VERSION,
                ts: 8,
                id: "e1".into(),
                parent: None,
                target_id: "r1".into(),
                replacement: None,
            },
        )
        .expect("a");

    let plain = store
        .export(&session, ExportOptions::default())
        .expect("export");
    let text = std::fs::read_to_string(&plain).expect("read export");
    assert!(!text.contains("cache_warm"), "usage records stripped");
    assert!(!text.contains("router"), "custom records stripped");
    for kept in [
        "Injected context...",
        "thinking-level-change",
        "checkpoint-1",
        "Refactor auth module",
        "context-edit",
    ] {
        assert!(text.contains(kept), "default export keeps: {kept}");
    }

    let audited = store
        .export(&session, ExportOptions { audit: true })
        .expect("export");
    let text = std::fs::read_to_string(&audited).expect("read export");
    assert!(text.contains("cache_warm"), "audit keeps usage");
    assert!(text.contains("router"), "audit keeps custom");
}
