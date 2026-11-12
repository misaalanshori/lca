//! Attachment reachability rows (gh #113/#311): a fork resolves its parent's
//! attachment, and the GC keeps every hash a resolved record list references
//! while still sweeping true orphans.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::common::{scratch, store};
use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::ExportOptions;

fn attachment_record(id: &str, hash: &str) -> Record {
    Record::ToolResult {
        v: FORMAT_VERSION,
        ts: 2,
        id: id.to_string(),
        parent: None,
        call_id: format!("{id}-call"),
        status: lca_protocol::ToolResultStatus::Ok,
        content: Some("[full output in an attachment]".to_string()),
        attachment: Some(hash.to_string()),
        truncated: true,
        duration_ms: None,
        exit_code: None,
        nested: Vec::new(),
        full_output_path: None,
        images: Vec::new(),
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
                parent: None,
                replaced_from: "r1".into(),
                replaced_to: "r1".into(),
                first_kept_id: String::new(),
                read_files: Vec::new(),
                modified_files: Vec::new(),
                system_prompt: None,
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
