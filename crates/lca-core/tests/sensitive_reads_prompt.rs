//! Sensitive reads at the turn level (gh #215 reopened): the turn
//! authorizes in-workspace reads of credential filenames —
//! `required_permission` must surface them, or `authorize()` never
//! runs and the escalation never fires. The authorize-level tests
//! missed this path; these drive whole turns.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_testkit::{FakeProvider, fake_usage};

// Verifies: gh #215 — a turn reading `.env` in-workspace prompts
// (the approval gate runs); the denial surfaces as a refused tool.
#[tokio::test]
async fn turn_reading_dotenv_prompts() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("read", r#"{"path":".env"}"#)
                .usage(fake_usage(10, 5, 0, 10))
        })
        .turn(|t| t.text("understood").usage(fake_usage(10, 5, 0, 10)))
        .build();
    let mut h = harness("sensitive-turn", provider, default_config());
    std::fs::write(h.project.join(".env"), "SECRET=hunter2").expect("write");
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    turn(&mut h, "read the env file", &mut sink, &mut prompt).await;
    assert_eq!(prompt.asked.len(), 1, ".env prompts at the turn level");
    assert!(
        prompt.asked[0].contains(".env"),
        "the prompt names the file: {:?}",
        prompt.asked
    );
}

// Verifies: gh #215 — a turn reading an ordinary file stays silent.
#[tokio::test]
async fn turn_reading_ordinary_files_stays_silent() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("read", r#"{"path":"notes.md"}"#)
                .usage(fake_usage(10, 5, 0, 10))
        })
        .turn(|t| t.text("noted").usage(fake_usage(10, 5, 0, 10)))
        .build();
    let mut h = harness("ordinary-turn", provider, default_config());
    std::fs::write(h.project.join("notes.md"), "three notes").expect("write");
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    turn(&mut h, "read the notes", &mut sink, &mut prompt).await;
    assert!(
        prompt.asked.is_empty(),
        "ordinary reads stay silent: {:?}",
        prompt.asked
    );
}
