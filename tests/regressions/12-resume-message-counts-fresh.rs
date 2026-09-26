//! Released defect (0.1.x): `lca resume` showed a session's message count
//! frozen at creation. The listing trusted an `index.json` cache that is only
//! refreshed when another session is created, so a session that had grown
//! from nothing still listed as `0 messages`.
//!
//! Verifies: FR-SESS-2.

use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::SessionStore;

fn user(id: &str, content: &str) -> Record {
    Record::User {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        content: content.to_string(),
        attachments: vec![],
    }
}

#[test]
fn a_grown_session_lists_its_current_message_count() {
    let root = std::env::temp_dir().join(format!(
        "lca-regression-resume-count-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "grows").expect("create");
    store.append(&session, user("r1", "hi")).expect("append1");
    store
        .append(&session, user("r2", "there"))
        .expect("append2");
    let listed = store.list_sessions(&project).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].message_count, 2,
        "the listing is fresh, not a creation-time snapshot"
    );
    let _ = std::fs::remove_dir_all(&root);
}
