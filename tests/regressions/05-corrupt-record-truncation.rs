//! Finding 5 (high, `../lca-issues.md`): the session reader's record-type
//! extractor read the JSON key `t` instead of its value, so a *known*
//! record type with bad fields was silently skipped instead of being
//! reported as truncation.
//!
//! Verifies: FR-SESS-6 (defect 5).

use std::io::Write;

use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{ReadOutcome, SessionStore};

fn user_record(id: &str, content: &str) -> Record {
    Record::User {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        content: content.to_string(),
        attachments: vec![],
    }
}

#[test]
fn a_known_record_type_with_bad_fields_reports_truncation() {
    let root = std::env::temp_dir().join(format!("lca-regression-corrupt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "before"))
        .expect("append");

    {
        // `user` requires `content`; omitting it makes deserialization fail
        // for a *known* type.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("open");
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
    assert_eq!(skipped_unknown, 0, "it is not counted as unknown");
    assert_eq!(records.len(), 2, "records before the bad line are kept");
    assert_eq!(records[1].id(), Some("r1"));
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("known record type")),
        "the warning names the known type: {warnings:?}"
    );
}
