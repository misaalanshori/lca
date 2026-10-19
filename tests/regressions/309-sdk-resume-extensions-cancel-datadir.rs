//! Gh #309: the embedding `Session` dropped `data_dir`, had no `resume`, no
//! way to attach extensions, and no way to cancel a running turn.
//!
//! Four gaps, one shape: the SDK was a "start a session and send input"
//! wrapper, and everything an embedder needs *after* that was missing.
//!
//! - `create` accepted `data_dir`, used it to open the store, then dropped it.
//!   `AgentConfig::default()` leaves `data_dir` empty, so every shell child of
//!   an embedded turn saw `LCA_DATA_DIR=""` - and `docs/configuration.md`
//!   documents that variable as how a command finds the agent's data home.
//! - `id()` documents itself as "resume tools take it", but there was no
//!   `Session::resume`; a host that kept the handle could not come back.
//! - Nothing attached an `ExtensionRegistry`, so an embedded host could not
//!   run any extension at all.
//! - The `CancelFlag` was a local of `send`, so a host could watch a turn
//!   start and had no way to stop it short of dropping the session.
//!
//! Verifies: gh #309 acceptance.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Arc;
use std::time::Duration;

use lca_protocol::{Record, StopReason, Usage};
use lca_sdk::Session;
use lca_testkit::FakeProvider;

fn scratch(name: &str) -> std::path::PathBuf {
    let root = lca_testkit::scratch_path(name);
    std::fs::create_dir_all(root.join("project")).expect("mkdir");
    root
}

/// A provider answering one text turn.
fn one_turn() -> Arc<FakeProvider> {
    Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.text("ok").usage(Usage {
                    input: 3,
                    output: 1,
                    ..Usage::default()
                })
            })
            .build(),
    )
}

// Verifies: gh #309 acceptance - a shell child of an embedded turn sees the
// session's `data_dir` as `LCA_DATA_DIR`. `env` prints the child's
// environment without a shell expansion (a `$VAR` trips the analyzer's
// sensitive-command heuristic).
#[tokio::test]
async fn a_shell_child_sees_the_sessions_data_dir() {
    let root = scratch("309-datadir");
    let data = root.join("data");
    let provider = Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.tool_call("shell", r#"{"command":"env"}"#).usage(Usage {
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
    let session = Session::create(root.join("project"), data.clone(), provider, "faux-1")
        .expect("session starts")
        .with_permission_prompt(|_| lca_permissions::Decision::Always);
    session.send("run it").await;

    let store = lca_session::SessionStore::new(data.clone());
    let stored = store
        .session(&root.join("project"), session.id())
        .expect("reattach");
    let results: Vec<String> = store
        .read(&stored)
        .expect("read")
        .records
        .iter()
        .filter_map(|record| match record {
            Record::ToolResult { content, .. } => content.clone(),
            _ => None,
        })
        .collect();
    let expected = format!("LCA_DATA_DIR={}", data.display());
    assert!(
        results.iter().any(|content| content.contains(&expected)),
        "LCA_DATA_DIR is the configured data dir ({expected}): {results:?}"
    );
}

// Verifies: gh #309 acceptance - `resume` reopens a session by id and appends
// to its log, in order.
#[tokio::test]
async fn resume_appends_to_the_existing_log() {
    let root = scratch("309-resume");
    let data = root.join("data");
    let first = Session::create(root.join("project"), data.clone(), one_turn(), "faux-1")
        .expect("session starts");
    let id = first.id().to_string();
    first.send("alpha").await;
    drop(first);

    let resumed = Session::resume(
        root.join("project"),
        data.clone(),
        &id,
        one_turn(),
        "faux-1",
    )
    .expect("resume");
    assert_eq!(resumed.id(), id);
    resumed.send("beta").await;

    let store = lca_session::SessionStore::new(data);
    let stored = store.session(&root.join("project"), &id).expect("reattach");
    let users: Vec<String> = store
        .read(&stored)
        .expect("read")
        .records
        .iter()
        .filter_map(|record| match record {
            Record::User { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["alpha", "beta"], "both turns, in order");
}

// Verifies: gh #309 acceptance - `resume` on an id that does not exist is an
// error, never a silently created session.
#[tokio::test]
async fn resume_of_an_unknown_id_is_an_error() {
    let root = scratch("309-resume-missing");
    let result = Session::resume(
        root.join("project"),
        root.join("data"),
        "nope",
        one_turn(),
        "faux-1",
    );
    assert!(result.is_err(), "an unknown id is refused");
}

// Verifies: gh #309 acceptance - an attached extension registry is the one the
// agent's config carries, so an embedded host can actually run extensions.
#[tokio::test]
async fn an_attached_registry_is_the_one_the_agent_uses() {
    let root = scratch("309-extensions");
    let registry = Arc::new(lca_core::ExtensionRegistry::default());
    let session = Session::create(
        root.join("project"),
        root.join("data"),
        one_turn(),
        "faux-1",
    )
    .expect("session starts")
    .with_extensions(registry.clone());
    assert!(
        session.shares_extensions(&registry),
        "the attached registry is the config's registry"
    );
}

// Verifies: gh #309 acceptance - `cancel` stops the running turn, and a cancel
// with no turn in flight does not poison the next one. The turn future is not
// `Send`, so both sides run on one task (`join!`) rather than through `spawn`.
#[tokio::test]
async fn cancel_stops_the_running_turn_and_does_not_stick() {
    let root = scratch("309-cancel");
    let provider = Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.pause(1_500).text("never").usage(Usage {
                    input: 3,
                    output: 1,
                    ..Usage::default()
                })
            })
            .build(),
    );
    let session = Session::create(root.join("project"), root.join("data"), provider, "faux-1")
        .expect("session starts");
    let canceller = async {
        tokio::time::sleep(Duration::from_millis(120)).await;
        session.cancel();
    };
    let (outcome, ()) = tokio::join!(session.send("long"), canceller);
    assert_eq!(
        outcome.stop_reason,
        StopReason::Cancelled,
        "the running turn was cancelled: {outcome:?}"
    );

    // A fresh session for the second half: the first one's provider has no
    // more scripted turns.
    let root = scratch("309-cancel-idle");
    let session = Session::create(
        root.join("project"),
        root.join("data"),
        one_turn(),
        "faux-1",
    )
    .expect("session starts");
    session.cancel();
    let outcome = session.send("hello").await;
    assert_eq!(
        outcome.stop_reason,
        StopReason::Stop,
        "an idle cancel does not stick: {outcome:?}"
    );
}
