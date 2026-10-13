//! GitHub #253: a compacted session must survive the Display read.
//! Live drive 2026-10-13: turns + `/compact` + `/rename` listed "No
//! sessions yet" — `read_with(Display)` failed while audit read fine,
//! the message count hit 0, and the session vanished from resume.
//!
//! Verifies: the Display read of a compacted log (turns, compaction
//! marker, rename, session end) succeeds with its messages, and the
//! resume index counts a compacted session.

use lca_protocol::{FORMAT_VERSION, Record, session_info_record};
use lca_session::{SessionStore, ViewMode};

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

fn assistant_record(id: &str, text: &str) -> Record {
    Record::Assistant {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        content: vec![lca_protocol::ContentBlock::Text {
            text: text.to_string(),
        }],
        reasoning: None,
        reasoning_signature: None,
        provider_thinking_level: None,
        model: None,
        provider: None,
        usage: None,
    }
}

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

fn compaction_record(id: &str, from: &str, to: &str) -> Record {
    Record::Compaction {
        v: FORMAT_VERSION,
        ts: 2,
        id: id.to_string(),
        parent: None,
        replaced_from: from.to_string(),
        replaced_to: to.to_string(),
        first_kept_id: String::new(),
        read_files: Vec::new(),
        modified_files: Vec::new(),
        system_prompt: None,
        summary: "old work summarized".to_string(),
        strategy: "compaction-default".to_string(),
        usage: None,
    }
}

fn session_end_record(id: &str) -> Record {
    Record::SessionEnd {
        v: FORMAT_VERSION,
        ts: 3,
        id: id.to_string(),
        parent: None,
    }
}

/// The live shape: turns, a compaction over them, a rename, and
/// the session end — with NOTHING after the compaction (the user
/// compacted, renamed, and left). Display correctly suppresses the
/// replaced turns; the session must still list via its summary.
fn live_compacted_log(store: &SessionStore, project: &std::path::Path) -> lca_session::Session {
    let session = store.create_session(project, "test").expect("create");
    for (id, text) in [("u1", "first"), ("u2", "second")] {
        store.append(&session, user_record(id, text)).expect("user");
        store
            .append(&session, assistant_record(&format!("{id}a"), "answer"))
            .expect("assistant");
    }
    store
        .append(&session, compaction_record("c1", "u1", "u2a"))
        .expect("compact");
    store
        .append(&session, session_info_record(4, "s1", "renamed"))
        .expect("rename");
    store
        .append(&session, session_end_record("e1"))
        .expect("end");
    session
}

// Verifies: gh #253 - the Display read of a compacted log succeeds
// and the session lists with its summary counted (today the count
// hits 0 and resume skips it).
#[test]
fn compacted_session_lists_with_its_summary_counted() {
    let root = scratch("gh253-display-compacted");
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = live_compacted_log(&store, &project);
    let audit = store
        .read_with(&session, ViewMode::Audit)
        .expect("audit reads");
    assert!(!audit.records.is_empty(), "audit sees the log");
    let display = store
        .read_with(&session, ViewMode::Display)
        .expect("display reads");
    assert!(
        display
            .records
            .iter()
            .any(|record| matches!(record, Record::Compaction { .. })),
        "the summary survives suppression"
    );
    let listed = store.list_sessions(&project).expect("list");
    let summary = listed
        .iter()
        .find(|entry| entry.id == session.id())
        .expect("the compacted session lists");
    assert!(summary.message_count > 0, "its summary counts");
    let _ = std::fs::remove_dir_all(&root);
}
