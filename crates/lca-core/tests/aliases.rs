//! Pi-name alias dispatch (gh #119): the model may call `find`, `ls`,
//! and `bash`; the turn normalizes them to `glob`, `list`, and `shell`
//! before records, permission, and dispatch see the name.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_core::{TurnEvent, TurnStatus};
use lca_protocol::ToolResultStatus;
use lca_session::ViewMode;
use lca_testkit::{FakeProvider, fake_usage};

// Verifies: gh #119 (a `find` call runs `glob`, and the log stores the
// canonical name): the alias answers, and every record says `glob`.
#[tokio::test]
async fn a_find_call_runs_glob_and_records_the_canonical_name() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("find", r#"{"pattern": "*.md"}"#)
                .usage(fake_usage(100, 20, 0, 100))
        })
        .turn(|t| t.text("done.").usage(fake_usage(200, 30, 100, 100)))
        .build();
    let mut h = harness("alias-find", provider, default_config());
    std::fs::write(h.project.join("notes.md"), "words").expect("write");
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "find my notes", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, TurnStatus::Ok);
    assert!(
        sink.events.iter().any(
            |e| matches!(e, TurnEvent::ToolFinished(r) if r.status == ToolResultStatus::Ok && r.content.contains("notes.md"))
        ),
        "the alias dispatched and answered"
    );

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    let names: Vec<&str> = read
        .records
        .iter()
        .filter_map(|r| match r {
            lca_protocol::Record::ToolCall { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["glob"], "records store the canonical name");
    assert!(
        !read.records.iter().any(|r| match r {
            lca_protocol::Record::ToolCall { name, .. } => name == "find",
            _ => false,
        }),
        "the alias never reaches the log"
    );
}

// Verifies: gh #119 (the map itself): every alias resolves, and anything
// else passes through untouched.
#[test]
fn canonical_tool_name_maps_aliases_and_passes_the_rest_through() {
    assert_eq!(lca_tools::canonical_tool_name("find"), "glob");
    assert_eq!(lca_tools::canonical_tool_name("ls"), "list");
    assert_eq!(lca_tools::canonical_tool_name("bash"), "shell");
    assert_eq!(lca_tools::canonical_tool_name("glob"), "glob");
    assert_eq!(
        lca_tools::canonical_tool_name("custom-ext-tool"),
        "custom-ext-tool"
    );
}
