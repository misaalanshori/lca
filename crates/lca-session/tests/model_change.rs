//! The `model-change` record (gh #8 / EFG-013): its shape, and the
//! additive-reader argument that makes a new record type safe to ship.
//!
//! Split from `log.rs`, which crossed the workspace's 1,200-line ceiling
//! (gate 11) with these two rows in it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{ReadOutcome, SessionStore};

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
        content: content.to_string(),
        attachments: vec![],
        queue: None,
    }
}

// Verifies: gh #8 / EFG-013 (the `model-change` record) - every model
// change appends one, and the fields say which model left, which arrived,
// and which endpoint will bill for it: `from` (absent when the session
// started with no model), `to`, `provider`, and `profile` when the model
// belongs to a named profile (gh #31: routing follows the model, so this
// record is the log's witness of a cross-profile switch).
#[test]
fn model_change_records_name_the_model_and_the_endpoint_that_will_bill() {
    let store = store("model-change");
    let project = scratch("model-change-project");
    let session = store.create_session(&project, "test").expect("create");

    store
        .append(
            &session,
            Record::ModelChange {
                v: FORMAT_VERSION,
                ts: 10,
                id: "c1".to_string(),
                from: None,
                to: "zen-free".to_string(),
                provider: "openai-compatible".to_string(),
                profile: Some("zen".to_string()),
            },
        )
        .expect("append");
    store
        .append(
            &session,
            Record::ModelChange {
                v: FORMAT_VERSION,
                ts: 11,
                id: "c2".to_string(),
                from: Some("zen-free".to_string()),
                to: lca_testkit::SMOKE_MODEL.to_string(),
                provider: "openai-compatible".to_string(),
                profile: Some("opencode-go".to_string()),
            },
        )
        .expect("append2");

    let log = std::fs::read_to_string(session.log_path()).expect("read log");
    assert!(
        log.contains(r#""t":"model-change""#),
        "the type tag is `model-change`: {log}"
    );
    let ReadOutcome {
        records,
        truncated,
        skipped_unknown,
        ..
    } = store.read(&session).expect("read");
    assert!(!truncated, "a model change is not corruption");
    assert_eq!(skipped_unknown, 0, "the current reader knows the type");

    let changes: Vec<&Record> = records
        .iter()
        .filter(|record| matches!(record, Record::ModelChange { .. }))
        .collect();
    assert_eq!(changes.len(), 2, "both changes load");
    match changes[0] {
        Record::ModelChange {
            from,
            to,
            provider,
            profile,
            ts,
            ..
        } => {
            assert!(from.is_none(), "the first change has no previous model");
            assert_eq!(to, "zen-free");
            assert_eq!(provider, "openai-compatible");
            assert_eq!(profile.as_deref(), Some("zen"), "the billing profile");
            assert_eq!(*ts, 10, "ts is the record's own field");
        }
        other => panic!("a model change reads back: {other:?}"),
    }
    match changes[1] {
        Record::ModelChange { from, to, .. } => {
            assert_eq!(from.as_deref(), Some("zen-free"), "the model it left");
            assert_eq!(to, lca_testkit::SMOKE_MODEL);
        }
        other => panic!("a model change reads back: {other:?}"),
    }
}

// Verifies: gh #8 / EFG-013's compatibility argument, pinned - a log
// holding `model-change` records is readable by a build that never heard
// of them. Two facts make that true, and this row needs both: the older
// build's record enum has no such variant (serde refuses the line), and
// the reader turns a refused line whose type it does not know into a
// skip, not a stop. The first fact is the frozen type list below; the
// second is `unknown_record_types_are_skipped_not_fatal`, which drives
// the same branch an old binary would reach.
#[test]
fn an_old_reader_skips_model_change_and_keeps_reading() {
    // The `KNOWN_TYPES` list as shipped before gh #8. An older binary
    // consults exactly this list when a line fails to parse; the whole
    // additive-record argument in docs/session-log-format.md rests on
    // `model-change` not being in it.
    const OLD_READER_KNOWN_TYPES: &[&str] = &[
        "session-start",
        "user",
        "assistant",
        "tool-call",
        "tool-result",
        "permission",
        "extension-event",
        "compaction",
        "fork-point",
        "session-end",
    ];
    assert!(
        !OLD_READER_KNOWN_TYPES.contains(&"model-change"),
        "the old reader does not know the type, so it takes the skip branch"
    );

    // That same old reader's serde enum has no variant for it either: the
    // line is not a record it can build, which is what sends it down that
    // branch instead of silently mis-typing the record.
    #[derive(serde::Deserialize)]
    #[serde(tag = "t", rename_all = "kebab-case")]
    #[allow(dead_code)]
    enum OldReaderRecord {
        SessionStart {
            v: u32,
            ts: u64,
            agent_version: String,
            abi_version: String,
            working_dir: String,
        },
        User {
            v: u32,
            ts: u64,
            id: String,
            content: String,
        },
        SessionEnd {
            v: u32,
            ts: u64,
            id: String,
        },
    }
    let line = r#"{"v":1,"t":"model-change","ts":10,"id":"c1","to":"zen-free","provider":"openai-compatible"}"#;
    assert!(
        serde_json::from_str::<OldReaderRecord>(line).is_err(),
        "an old reader cannot parse the line as a record"
    );
    let _ = line;

    // And the log itself still holds everything around the unknown line:
    // a tag no reader knows is one skip, not a stop, and the records on
    // both sides - `model-change` included - load. That branch is the
    // same code an old binary reaches with `model-change` as its tag.
    let store = store("old-reader-model-change");
    let project = scratch("old-reader-model-change-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "before"))
        .expect("append");
    store
        .append(
            &session,
            Record::ModelChange {
                v: FORMAT_VERSION,
                ts: 9,
                id: "c9".to_string(),
                from: None,
                to: "zen-free".to_string(),
                provider: "openai-compatible".to_string(),
                profile: None,
            },
        )
        .expect("append the model change");
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("open");
        file.write_all(br#"{"v":1,"t":"from-the-future","ts":10,"id":"r19"}"#)
            .expect("write");
        file.write_all(b"\n").expect("newline");
    }
    store
        .append(&session, user_record("r2", "after"))
        .expect("append2");

    let ReadOutcome {
        records,
        truncated,
        skipped_unknown,
        ..
    } = store.read(&session).expect("read");
    assert!(!truncated, "an unknown record type is not truncation");
    assert_eq!(
        skipped_unknown, 1,
        "the unknown tag is one skip, not a stop"
    );
    let ids: Vec<&str> = records.iter().filter_map(|r| r.id()).collect();
    assert_eq!(
        ids,
        vec!["r1", "c9", "r2"],
        "the model change and both messages around the skipped line load, in order: {ids:?}"
    );
}
