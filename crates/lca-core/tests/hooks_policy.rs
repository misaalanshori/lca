//! The hook-policy rows: a pre-tool denial that never reaches the prompt, a
//! replaced call, the pre-turn/attention pair, session close, and all six hook
//! points in one session (FR-CORE-10, FR-EXT-3).
//!
//! Gh #548 phase 4 split `tests/hooks.rs` by subject, so each of these rows now
//! reports as `hooks_policy::<row>`, stated in the cycle report.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::hooks::PolicyExt;
use common::hooks::harness;
use common::hooks::turn;
use common::*;
use lca_core::{ExtensionRegistry, TurnEvent, TurnStatus};
use lca_protocol::{Record, ToolResultStatus};
use lca_session::ViewMode;
use lca_testkit::{FakeProvider, fake_usage};
use std::sync::Arc;

// Verifies: FR-CORE-10 (the pre-tool hook runs before the permission
// check, and a hook denial ends the call without a user prompt).
#[tokio::test]
async fn hook_denial_ends_the_call_without_a_prompt() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"deny-me-now"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("understood").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(PolicyExt::deny("gate", "shell")));
    let mut h = harness("deny", provider, registry);
    let mut sink = CollectingSink::default();

    let outcome = turn(&mut h, "run it", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Ok);

    let results: Vec<_> = sink
        .events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::ToolFinished(result) => Some(result.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, ToolResultStatus::Denied);
    assert!(
        results[0].content.contains("denied by pattern"),
        "{}",
        results[0].content
    );

    let asked = h.prompt.asked();
    assert!(
        asked.is_empty(),
        "no user prompt for a hook denial, asked: {asked:?}"
    );

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    assert!(
        read.records.iter().any(|r| matches!(
            r,
            Record::ToolResult {
                status: ToolResultStatus::Denied,
                ..
            }
        )),
        "the denial is on record"
    );
}

// Verifies: FR-CORE-10 (a replaced call passes through the permission
// layer like any other and is not fed back through the hooks).
#[tokio::test]
async fn a_replaced_call_reaches_the_prompt_and_is_not_rehooked() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"danger-original"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let policy = Arc::new(PolicyExt::replacing("rewriter", "echo safe-replacement"));
    let mut registry = ExtensionRegistry::new();
    registry.register(policy.clone());
    let mut h = harness("replace", provider, registry);
    let mut sink = CollectingSink::default();

    let outcome = turn(&mut h, "run it", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Ok, "{:?}", outcome.error);
    assert_eq!(policy.calls(), 1, "the replacement is not re-hooked");

    let asked = h.prompt.asked();
    assert_eq!(
        asked.len(),
        1,
        "the replacement still hits the prompt: {asked:?}"
    );
    assert!(asked[0].contains("echo safe-replacement"), "{asked:?}");
    assert!(
        !asked[0].contains("danger-original"),
        "prompt shows the REPLACED call: {asked:?}"
    );

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    let permission = read
        .records
        .iter()
        .find_map(|r| match r {
            Record::Permission { action, .. } => Some(action.clone()),
            _ => None,
        })
        .expect("permission record");
    assert!(permission.contains("echo safe-replacement"), "{permission}");
}

// Verifies: the SRDD hook points - `pre-turn` fires once per turn before any
// provider work, and `attention-required` fires when a turn ends in error.
#[tokio::test]
async fn pre_turn_and_attention_hooks_fire() {
    let policy = Arc::new(PolicyExt::deny("observed", "deny-"));

    let provider = FakeProvider::builder()
        .turn(|t| t.text("hi").usage(fake_usage(10, 5, 0, 10)))
        .build();
    let mut registry = ExtensionRegistry::new();
    registry.register(policy.clone());
    let mut h = harness("hooks-pre-turn", provider, registry);
    let mut sink = CollectingSink::default();
    let outcome = turn(&mut h, "hello", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Ok);
    assert_eq!(policy.pre_turn_calls(), 1, "pre-turn fires once per turn");
    assert_eq!(
        policy.attention_calls(),
        0,
        "a successful turn needs no attention"
    );

    let failing = FakeProvider::builder()
        .turn(|t| t.error("boom", false).usage(fake_usage(1, 1, 0, 0)))
        .build();
    let mut registry = ExtensionRegistry::new();
    registry.register(policy.clone());
    let mut h = harness("hooks-attention", failing, registry);
    let mut sink = CollectingSink::default();
    let outcome = turn(&mut h, "fail please", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Error);
    assert_eq!(
        policy.attention_calls(),
        1,
        "a failed turn asks for attention"
    );
    assert_eq!(
        policy.pre_turn_calls(),
        2,
        "the failing turn ran pre-turn too"
    );
}

// The registry's `session-close` fans out to every hooks handle; the CLI
// calls it on exit (headless and after the interface).
#[tokio::test]
async fn session_close_hook_fires_when_called() {
    let policy = Arc::new(PolicyExt::deny("closer", "deny-"));
    let mut registry = ExtensionRegistry::new();
    registry.register(policy.clone());
    registry.on_session_close().await;
    assert_eq!(policy.close_calls(), 1);
}

// Verifies: all six hook points fire in one session - the "three of six
// never fired" class the audit found. Before the post-audit wiring,
// pre-turn, attention-required, and session-close were registered but never
// called.
#[tokio::test]
async fn all_six_hook_points_fire_in_one_session() {
    let policy = Arc::new(PolicyExt::deny("all-six", "deny-"));

    // Four provider calls across three agent turns: a plain reply, a tool
    // call plus its follow-up reply, then an error that asks for attention.
    let provider = FakeProvider::builder()
        .turn(|t| t.text("one").usage(fake_usage(10, 5, 0, 10)))
        .turn(|t| {
            t.tool_call("read", r#"{"path":"notes.md"}"#)
                .usage(fake_usage(20, 5, 10, 10))
        })
        .turn(|t| t.text("two").usage(fake_usage(30, 5, 20, 10)))
        .turn(|t| t.error("boom", false).usage(fake_usage(1, 1, 0, 0)))
        .build();
    let mut registry = ExtensionRegistry::new();
    registry.register(policy.clone());
    let mut h = harness("hooks-all-six", provider, registry);
    std::fs::write(h.tools.workspace().join("notes.md"), "notes").expect("write");
    let mut sink = CollectingSink::default();

    for input in ["one", "two", "three"] {
        let _ = turn(&mut h, input, &mut sink).await;
    }
    // The CLI calls this when the session ends (headless and after the TUI).
    h.config.extensions.on_session_close().await;

    assert_eq!(policy.pre_turn_calls(), 3, "pre-turn once per turn");
    assert_eq!(policy.calls(), 1, "pre-tool-use for the model's read call");
    assert_eq!(policy.post_tool_calls(), 1, "post-tool-use after the read");
    assert_eq!(policy.post_turn_calls(), 3, "post-turn-end once per turn");
    assert_eq!(
        policy.attention_calls(),
        1,
        "attention-required on the error turn"
    );
    assert_eq!(
        policy.close_calls(),
        1,
        "session-close when the session ends"
    );
}
