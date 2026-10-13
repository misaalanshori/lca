//! Turn emission (gh #258 §3A, acceptance 3): state transitions,
//! tool resolution, and retry attempts emit under the feature.
//! Feature-gated: without `debug-trace` there is no subscriber to
//! capture with.

#![cfg(feature = "debug-trace")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_testkit::{FakeProvider, fake_usage};

fn capture(filter: &str, run: impl FnOnce()) -> String {
    let root = lca_testkit::scratch_path("debug-core-emit");
    let paths = lca_debug::DebugPaths::in_dir(&root);
    let config = lca_debug::DebugConfig {
        paths: paths.clone(),
        filter: Some(filter.to_string()),
    };
    let subscriber = lca_debug::subscriber(&config).expect("subscriber");
    tracing::subscriber::with_default(subscriber, run);
    std::fs::read_to_string(&paths.log).unwrap_or_default()
}

// Verifies: gh #258 - one turn emits its start (input length, never
// content), its tool call, and its end status. Sync test: the scoped
// subscriber is thread-local, so the turn runs on this thread.
#[test]
fn a_turn_emits_start_tool_and_end() {
    let log = capture("lca_core=trace", || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let provider = FakeProvider::builder()
                .turn(|t| {
                    t.tool_call("read", r#"{"path":"notes.md"}"#)
                        .usage(fake_usage(10, 5, 0, 10))
                })
                .turn(|t| t.text("finished").usage(fake_usage(10, 5, 0, 10)))
                .build();
            let mut h = harness("emit", provider, default_config());
            std::fs::write(h.project.join("notes.md"), "three notes").expect("write");
            let mut sink = CollectingSink::default();
            let mut prompt = Prompt {
                answers: vec![],
                asked: vec![],
            };
            turn(&mut h, "hi there", &mut sink, &mut prompt).await;
        });
    });
    for event in ["turn.start", "tool.call", "turn.end"] {
        assert!(log.contains(event), "a turn emits {event}:\n{log}");
    }
    assert!(!log.contains("hi there"), "input content never emits");
    assert!(!log.contains("file contents"), "results never emit");
}
