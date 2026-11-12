//! View and export rows (FR-SESS-7): display mode substitutes a compaction
//! summary for the range it replaces, audit mode walks everything, and the
//! export strips the audit-only record types unless asked.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::common::{scratch, store, user_record};
use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{ExportOptions, ViewMode};

// Verifies: FR-SESS-6 (the display view keeps the reader's truncation
// warning). It hardcoded `truncated: false` and empty warnings, so a corrupt
// session resumed with no warning at all.
#[test]
fn the_display_view_reports_a_truncated_log() {
    let store = store("view-truncated");
    let project = scratch("view-truncated-project");
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user_record("r1", "before"))
        .expect("append");
    {
        use std::io::Write;
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
    let outcome = store.read_with(&session, ViewMode::Display).expect("read");
    assert!(
        outcome.truncated,
        "the display view keeps the truncation flag"
    );
    assert!(!outcome.warnings.is_empty(), "and the warning text");
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
                parent: None,
                replaced_from: "r1".into(),
                replaced_to: "r3".into(),
                first_kept_id: String::new(),
                read_files: Vec::new(),
                modified_files: Vec::new(),
                system_prompt: None,
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
                parent: None,
                replaced_from: "r1".into(),
                replaced_to: "r2".into(),
                first_kept_id: String::new(),
                read_files: Vec::new(),
                modified_files: Vec::new(),
                system_prompt: None,
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
                parent: None,
                replaced_from: "r1".into(),
                replaced_to: "c1".into(),
                first_kept_id: String::new(),
                read_files: Vec::new(),
                modified_files: Vec::new(),
                system_prompt: None,
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
                parent: None,
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
                parent: None,
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
                    parent: None,
                    call_id: call_id.into(),
                    status: lca_protocol::ToolResultStatus::Ok,
                    content: Some("truncated".into()),
                    attachment: Some(hash.clone()),
                    truncated: true,
                    duration_ms: None,
                    exit_code: None,
                    nested: Vec::new(),
                    full_output_path: None,
                    images: Vec::new(),
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
