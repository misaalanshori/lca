//! Released defect (0.1.x): the display view discarded the session reader's
//! truncation warning. `SessionStore::read_with` (the path the interactive
//! interface uses) hardcoded `truncated: false` and empty warnings, so a
//! corrupt session resumed with a short or empty transcript and no
//! explanation. Headless mode reads through `read` and did warn.
//!
//! Verifies: FR-SESS-6.

use std::io::Write;

use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{SessionStore, ViewMode};

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
fn the_display_view_keeps_a_truncation_warning() {
    let root = lca_testkit::scratch_path("regression-view-truncated");
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("create");
    store
        .append(&session, user("r1", "before"))
        .expect("append");
    {
        // A known type missing a required field: deserialization fails.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("open");
        file.write_all(b"{\"v\":1,\"t\":\"user\",\"ts\":3,\"id\":\"r2\"}\n")
            .expect("write");
    }
    store.append(&session, user("r3", "after")).expect("append");

    let outcome = store.read_with(&session, ViewMode::Display).expect("read");
    assert!(outcome.truncated, "the display view keeps the flag");
    assert!(!outcome.warnings.is_empty(), "and the warning text");
    let _ = std::fs::remove_dir_all(&root);
}
