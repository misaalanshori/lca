//! gh #37 phase 2: the resume path end to end without a live model -
//! a branched session's Display view assembles with the summary in
//! context and the abandoned path out of it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_core::assemble;
use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{SessionStore, ViewMode};

fn text(message: &lca_protocol::ChatMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn user(id: &str, content: &str) -> Record {
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

// Verifies: FR-SESS-11 (resuming a branch assembles the live chain:
// the summary in, the abandoned branch out).
#[test]
fn resuming_a_branch_assembles_the_live_chain() {
    let root = lca_testkit::scratch_path("core-branch-resume");
    let store = SessionStore::new(root);
    let project = lca_testkit::scratch_path("core-branch-resume-project");
    let session = store.create_session(&project, "test").expect("create");
    store.append(&session, user("a", "first")).expect("a");
    store
        .append(&session, user("b", "abandoned message"))
        .expect("b");
    store
        .summarize_branch(&session, "a", "tried the abandoned thing")
        .expect("summarize");
    store.append(&session, user("d", "continued")).expect("d");

    let shown = store
        .resolved(&session, ViewMode::Display)
        .expect("display");
    let assembled = assemble(&shown.records, "sys");
    let bodies: Vec<String> = assembled.messages.iter().map(text).collect();
    assert!(
        bodies
            .iter()
            .any(|body| body.contains("tried the abandoned thing")),
        "the summary reaches the model: {bodies:?}"
    );
    assert!(
        bodies.iter().any(|body| body == "continued"),
        "the continuation reaches the model: {bodies:?}"
    );
    assert!(
        !bodies.iter().any(|body| body.contains("abandoned message")),
        "the cut path stays out of context: {bodies:?}"
    );
}
