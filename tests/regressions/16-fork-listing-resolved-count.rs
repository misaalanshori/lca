//! Released defect (0.2.0): `lca resume` listed a forked session as
//! `0 messages`. The index cache counted the fork's own two framing records
//! instead of its resolved (parent-inherited) history, so the listing
//! disagreed with what resuming the fork actually showed.
//!
//! Verifies: FR-SESS-3.

use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::SessionStore;

fn user(id: &str, content: &str) -> Record {
    Record::User {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.into(),
        content: content.into(),
        attachments: vec![],
    }
}

#[test]
fn a_forked_session_lists_its_resolved_message_count() {
    let root = lca_testkit::scratch_path("regression-fork-listing");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let parent = store.create_session(&project, "parent").expect("create");
    store.append(&parent, user("r1", "one")).expect("append");
    store.append(&parent, user("r2", "two")).expect("append");
    let child = store.fork(&parent, "r1").expect("fork");

    let listed = store.list_sessions(&project).expect("list");
    let count = |id: &str| {
        listed
            .iter()
            .find(|s| s.id == id)
            .expect("session listed")
            .message_count
    };
    assert_eq!(count(parent.id()), 2, "the parent counts its two messages");
    assert_eq!(
        count(child.id()),
        1,
        "the fork lists its inherited history (r1), not 0"
    );
}
