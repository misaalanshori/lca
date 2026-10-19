//! Gh #310: `Session::send` rebuilt `ToolExecutor` and `AgentConfig` on every
//! turn, so all their cross-turn state reset.
//!
//! Two consequences, both silent:
//!
//! - **`ReadTracker`.** `edit_requires_read: true` is a staleness guard: an
//!   edit demands a prior fresh read. With a per-turn executor, a file read on
//!   turn 1 looked unread on turn 2, so the guard refused an edit the user had
//!   every right to make - and the same reset wiped `loaded_instructions`, the
//!   subfolder `AGENTS.md` once-per-session dedup.
//! - **`sent_stable`.** FR-CACHE-6 detects a `context-transform` that rewrote a
//!   message inside the stable prefix, which is what makes a prompt-cache
//!   divergence visible. `AgentConfig::default()` created a fresh `Arc` each
//!   turn, so the check could only ever compare a turn against itself. The
//!   same reset dropped `instruction_baseline` and `skills_cache`.
//!
//! Verifies: gh #310 acceptance.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Arc;

use lca_protocol::Usage;
use lca_sdk::Session;
use lca_testkit::FakeProvider;

fn scratch(name: &str) -> std::path::PathBuf {
    let root = lca_testkit::scratch_path(name);
    std::fs::create_dir_all(root.join("project")).expect("mkdir");
    root
}

// Verifies: gh #310 acceptance - a read on turn 1 permits an edit on turn 2
// with `edit_requires_read` on. Before the fix the second turn's fresh
// executor had no record of the read and refused the edit.
#[tokio::test]
async fn a_read_on_one_turn_permits_an_edit_on_the_next() {
    let root = scratch("310-readtracker");
    let file = root.join("project/note.txt");
    std::fs::write(&file, "one\n").expect("seed");
    let provider = Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.tool_call("read", r#"{"path":"note.txt"}"#).usage(Usage {
                    input: 3,
                    output: 1,
                    ..Usage::default()
                })
            })
            .turn(|t| {
                t.tool_call(
                    "edit",
                    r#"{"path":"note.txt","edits":[{"oldText":"one","newText":"two"}]}"#,
                )
                .usage(Usage {
                    input: 3,
                    output: 1,
                    ..Usage::default()
                })
            })
            .turn(|t| {
                t.text("done").usage(Usage {
                    input: 3,
                    output: 1,
                    ..Usage::default()
                })
            })
            .build(),
    );
    let session = Session::create(root.join("project"), root.join("data"), provider, "faux-1")
        .expect("session starts")
        .with_agent_config(|config| config.edit_requires_read = true)
        .with_permission_prompt(|_| lca_permissions::Decision::Always);
    session.send("read it").await;
    session.send("edit it").await;

    assert_eq!(
        std::fs::read_to_string(&file).expect("read back"),
        "two\n",
        "the edit applied, so the read tracker survived the turn boundary"
    );
}

// Verifies: gh #310 acceptance - `sent_stable` is one shared cell for the life
// of the session, and each turn writes it. A per-turn `AgentConfig::default()`
// would write into a throwaway.
#[tokio::test]
async fn the_cache_divergence_cell_is_shared_across_turns() {
    let root = scratch("310-sentstable");
    let provider = Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.text("one").usage(Usage {
                    input: 3,
                    output: 1,
                    ..Usage::default()
                })
            })
            .turn(|t| {
                t.text("two").usage(Usage {
                    input: 3,
                    output: 1,
                    ..Usage::default()
                })
            })
            .build(),
    );
    let session = Session::create(root.join("project"), root.join("data"), provider, "faux-1")
        .expect("session starts");
    let cell = session.sent_stable_handle();
    session.send("first").await;
    let after_first = cell
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert!(
        after_first.is_some(),
        "the turn recorded the stable prefix it sent"
    );
    session.send("second").await;
    let after_second = cell
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_ne!(
        after_first, after_second,
        "the second turn read and rewrote the SAME cell, not a fresh one"
    );
}

// Verifies: gh #310 - the tool executor is built once, so its instruction dedup
// (`loaded_instructions`) is per session. A nested `AGENTS.md` reaches the
// model at most once, no matter how many turns run.
#[tokio::test]
async fn a_nested_instruction_injects_at_most_once_per_session() {
    let root = scratch("310-instructions");
    let nested = root.join("project/sub");
    std::fs::create_dir_all(&nested).expect("mkdir");
    std::fs::write(nested.join("AGENTS.md"), "nested rules\n").expect("seed");
    std::fs::write(nested.join("data.txt"), "payload\n").expect("seed");
    let provider = Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.tool_call("read", r#"{"path":"sub/data.txt"}"#)
                    .usage(Usage {
                        input: 3,
                        output: 1,
                        ..Usage::default()
                    })
            })
            .turn(|t| {
                t.tool_call("read", r#"{"path":"sub/data.txt"}"#)
                    .usage(Usage {
                        input: 3,
                        output: 1,
                        ..Usage::default()
                    })
            })
            .turn(|t| {
                t.text("done").usage(Usage {
                    input: 3,
                    output: 1,
                    ..Usage::default()
                })
            })
            .build(),
    );
    let session = Session::create(root.join("project"), root.join("data"), provider, "faux-1")
        .expect("session starts")
        .with_permission_prompt(|_| lca_permissions::Decision::Always);
    session.send("read one").await;
    session.send("read two").await;

    let store = lca_session::SessionStore::new(root.join("data"));
    let stored = store
        .session(&root.join("project"), session.id())
        .expect("reattach");
    let injected = store
        .read(&stored)
        .expect("read")
        .records
        .iter()
        .filter(|record| match record {
            lca_protocol::Record::User { content, .. } => content.contains("nested rules"),
            _ => false,
        })
        .count();
    assert!(
        injected <= 1,
        "the nested instruction injects at most once per session, got {injected}"
    );
}
