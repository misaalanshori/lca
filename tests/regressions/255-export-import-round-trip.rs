//! GitHub #255: export writes a JSON document, import wanted raw
//! JSONL, so a just-exported session could not be re-imported.
//! Direction (brief): import accepts both shapes, export unchanged.
//!
//! Verifies: export → import → resume round trip lands a listed,
//! readable session with its messages.

use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{ExportOptions, SessionStore, ViewMode};

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

// Verifies: gh #255 - the file `export` writes imports back into a
// session that lists with its messages and reads them back (today
// the import fails on the document's first line).
#[test]
fn export_document_imports_back_to_a_listed_session() {
    let root = lca_testkit::scratch_path("gh255-export-import");
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "original").expect("create");
    store
        .append(&session, user_record("u1", "hello"))
        .expect("user");
    store
        .append(&session, user_record("u2", "world"))
        .expect("user");

    let exported = store
        .export(&session, ExportOptions { audit: false })
        .expect("export");
    let text = std::fs::read_to_string(&exported).expect("read export");
    assert!(text.contains("\"records\""), "the document shape:\n{text}");
    let lines: Vec<&str> = text.lines().collect();
    let imported = store
        .import_jsonl(&project, "round-trip", &lines)
        .expect("import the export");
    let listed = store.list_sessions(&project).expect("list");
    let entry = listed
        .iter()
        .find(|entry| entry.id == imported.id())
        .expect("the imported session lists");
    assert!(
        entry.message_count >= 2,
        "its messages count: {}",
        entry.message_count
    );
    let read = store
        .read_with(&imported, ViewMode::Audit)
        .expect("read back");
    assert!(
        read.records
            .iter()
            .any(|record| matches!(record, Record::User { content, .. } if content == "hello")),
        "the transcript survived"
    );
    let _ = std::fs::remove_dir_all(&root);
}
