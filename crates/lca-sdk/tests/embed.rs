//! The Embedding SDK's contract (SRDD: "creates a session, subscribes
//! to events, and sends input").

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;
use std::time::Duration;

use lca_protocol::{Record, Usage};
use lca_sdk::{Session, TurnEvent};
use lca_testkit::FakeProvider;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = lca_testkit::scratch_path(name);
    std::fs::create_dir_all(project_of(&dir)).expect("mkdir");
    dir
}

fn project_of(root: &std::path::Path) -> std::path::PathBuf {
    root.join("project")
}

fn fake() -> Arc<FakeProvider> {
    Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.text("hel").text("lo").usage(Usage {
                    input: 11,
                    output: 2,
                    ..Usage::default()
                })
            })
            .turn(|t| {
                t.text("again").usage(Usage {
                    input: 9,
                    output: 1,
                    ..Usage::default()
                })
            })
            .build(),
    )
}

// Verifies: FR-CORE-4 (partial content reaches the subscriber as it
// arrives - the embedding event stream: deltas before the turn's end,
// usage included, exactly the interface section's promise that a host
// subscribes to events).
#[tokio::test]
async fn a_host_creates_a_session_subscribes_and_sends_input() {
    let root = scratch("sdk-stream");
    let session = Session::create(project_of(&root), root.join("data"), fake(), "faux-1")
        .expect("session starts");
    let mut events = session.subscribe();

    session.send("first").await;

    let mut deltas = 0usize;
    let mut saw_usage = false;
    let mut ended = false;
    let mut order = Vec::new();
    for _ in 0..64 {
        match events.try_recv() {
            Ok(TurnEvent::TextDelta(_)) => {
                deltas += 1;
                order.push("delta");
            }
            Ok(TurnEvent::Usage(_)) => {
                saw_usage = true;
                order.push("usage");
            }
            Ok(TurnEvent::TurnEnded { .. }) => {
                ended = true;
                order.push("end");
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(deltas >= 2, "both scripted deltas streamed: {deltas}");
    assert!(saw_usage, "the turn's usage arrived");
    assert!(ended, "the turn's end arrived");
    let end = order.iter().position(|step| *step == "end").expect("end");
    assert!(
        order[..end].contains(&"delta"),
        "deltas arrived before the end, not batched after it: {order:?}"
    );
}

// Verifies: FR-SESS-1 (each input appends to the same on-disk log:
// the second turn's pair lands after the first, nothing rewritten -
// append-only at the embedding surface).
#[tokio::test]
async fn inputs_accumulate_in_one_append_only_log() {
    let root = scratch("sdk-log");
    let session = Session::create(project_of(&root), root.join("data"), fake(), "faux-1")
        .expect("session starts");

    session.send("first").await;
    session.send("second").await;

    let store = lca_session::SessionStore::new(root.join("data"));
    let stored = store
        .session(&project_of(&root), session.id())
        .expect("reattach");
    let records = store.read(&stored).expect("read").records;
    let shape: Vec<&str> = records
        .iter()
        .map(|record| match record {
            Record::SessionStart { .. } => "start",
            Record::User { .. } => "user",
            Record::Assistant { .. } => "assistant",
            other => other.type_tag(),
        })
        .collect();
    assert_eq!(
        shape,
        ["start", "user", "assistant", "user", "assistant"],
        "both turns appended in order"
    );
}

// ---------------------------------------------------------------------------
// Gh #309 / #310: session-scoped state must survive across turns.
// ---------------------------------------------------------------------------

/// A provider that runs the `shell` tool once, then answers. `env` prints the
/// child's environment without a shell expansion, so the analyzer does not
/// classify the command as sensitive (a `$VAR` trips that heuristic).
fn shell_env_provider() -> Arc<FakeProvider> {
    Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.tool_call("shell", r#"{"command":"env"}"#).usage(Usage {
                    input: 5,
                    output: 1,
                    ..Usage::default()
                })
            })
            .turn(|t| {
                t.text("done").usage(Usage {
                    input: 5,
                    output: 1,
                    ..Usage::default()
                })
            })
            .build(),
    )
}

/// Every tool result's content in the session's log.
fn tool_results(root: &std::path::Path, session: &Session) -> Vec<String> {
    let store = lca_session::SessionStore::new(root.join("data"));
    let stored = store
        .session(&project_of(root), session.id())
        .expect("reattach");
    store
        .read(&stored)
        .expect("read")
        .records
        .iter()
        .filter_map(|record| match record {
            Record::ToolResult { content, .. } => content.clone(),
            _ => None,
        })
        .collect()
}

// Verifies: gh #309 acceptance - `LCA_DATA_DIR` in a shell child is the
// `data_dir` passed to `Session::create`. `AgentConfig::default()` leaves
// `data_dir` empty, so before the fix a shell child saw `LCA_DATA_DIR=""`:
// every command that reads the agent's data home (session lookups, credential
// paths, the documented `LCA_*` row in `docs/configuration.md`) got nothing.
#[tokio::test]
async fn a_shell_child_sees_the_configured_data_dir() {
    let root = scratch("sdk-datadir");
    let data = root.join("data");
    // Deny-by-default is load-bearing (NFR-13), so the shell call needs an
    // explicit approval here - the row is about the environment the child
    // sees, not about the permission path.
    let session = Session::create(
        project_of(&root),
        data.clone(),
        shell_env_provider(),
        "faux-1",
    )
    .expect("session starts")
    .with_permission_prompt(|_| lca_permissions::Decision::Always);
    session.send("run it").await;

    let results = tool_results(&root, &session);
    let expected = format!("LCA_DATA_DIR={}", data.display());
    assert!(
        results.iter().any(|content| content.contains(&expected)),
        "the shell child's LCA_DATA_DIR is the session's data dir ({expected}); got: {results:?}"
    );
    assert!(
        !results
            .iter()
            .any(|content| content.contains("LCA_DATA_DIR=\n")),
        "and it is not empty: {results:?}"
    );
}

// Verifies: gh #310 acceptance - the `ReadTracker` survives a turn boundary, so
// `edit_requires_read` means what it says across a conversation. Before the fix
// `ToolExecutor` was rebuilt per `send`, so a file read on turn 1 looked unread
// on turn 2 and the edit was refused with "read it first".
#[tokio::test]
async fn a_read_on_one_turn_permits_an_edit_on_the_next() {
    let root = scratch("sdk-readtracker");
    let file = project_of(&root).join("note.txt");
    std::fs::write(&file, "one\n").expect("seed");

    let provider = Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.tool_call("read", r#"{"path":"note.txt"}"#).usage(Usage {
                    input: 5,
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
                    input: 5,
                    output: 1,
                    ..Usage::default()
                })
            })
            .turn(|t| {
                t.text("done").usage(Usage {
                    input: 5,
                    output: 1,
                    ..Usage::default()
                })
            })
            .build(),
    );
    let session = Session::create(project_of(&root), root.join("data"), provider, "faux-1")
        .expect("session starts")
        .with_agent_config(|config| config.edit_requires_read = true)
        .with_permission_prompt(|_| lca_permissions::Decision::Always);

    session.send("read it").await;
    session.send("edit it").await;

    let text = std::fs::read_to_string(&file).expect("read the file back");
    assert_eq!(
        text, "two\n",
        "the edit applied, so the read tracker persisted across turns"
    );
}

// Verifies: gh #309 acceptance - `Session::resume` reopens an existing session
// by id and appends to its log. Before the fix `create` was the only
// constructor and it always started a new session, though `id()` documents
// itself as "resume tools take it".
#[tokio::test]
async fn resume_reopens_a_session_and_appends() {
    let root = scratch("sdk-resume");
    let data = root.join("data");
    let first =
        Session::create(project_of(&root), data.clone(), fake(), "faux-1").expect("session starts");
    let id = first.id().to_string();
    first.send("first").await;
    drop(first);

    let resumed =
        Session::resume(project_of(&root), data.clone(), &id, fake(), "faux-1").expect("resume");
    assert_eq!(resumed.id(), id, "the same session");
    resumed.send("second").await;

    let store = lca_session::SessionStore::new(data);
    let stored = store.session(&project_of(&root), &id).expect("reattach");
    let records = store.read(&stored).expect("read").records;
    let users: Vec<&str> = records
        .iter()
        .filter_map(|record| match record {
            Record::User { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        users,
        ["first", "second"],
        "both turns are in the one log, in order"
    );
}

// Verifies: gh #309 acceptance - `resume` refuses an unknown id rather than
// silently starting a fresh session under that name.
#[tokio::test]
async fn resume_refuses_an_unknown_id() {
    let root = scratch("sdk-resume-missing");
    let err = Session::resume(
        project_of(&root),
        root.join("data"),
        "no-such-session",
        fake(),
        "faux-1",
    )
    .expect_err("an unknown id is refused");
    assert!(
        err.to_string().contains("session"),
        "the failure names what went wrong: {err}"
    );
}

// Verifies: gh #310 acceptance - `with_extensions` reaches the agent's config.
// A per-turn `AgentConfig::default()` dropped it, so an embedded extension
// could attach and never run.
#[tokio::test]
async fn with_extensions_reaches_the_agent_config() {
    let root = scratch("sdk-extensions");
    let registry = Arc::new(lca_core::ExtensionRegistry::default());
    let session = Session::create(project_of(&root), root.join("data"), fake(), "faux-1")
        .expect("session starts")
        .with_extensions(registry.clone());
    let attached = session.shares_extensions(&registry);
    assert!(attached, "the attached registry is the config's registry");
    // And a turn still runs cleanly with it attached.
    let outcome = session.send("hello").await;
    assert_eq!(outcome.stop_reason, lca_protocol::StopReason::Stop);
}

// Verifies: gh #309 acceptance - `Session::cancel` stops the turn that is
// running. Before the fix the `CancelFlag` was a local of `send`, so an
// embedder could watch a turn start and had no way to stop it; the only
// alternative was dropping the session.
//
// The turn future is not `Send` (the tool executor holds non-`Send` state),
// so this drives both sides on one task with `join!`: a timer cancels while
// `send` is parked. That is the same race the issue describes, expressed in
// the shape the SDK can actually offer today.
#[tokio::test]
async fn cancel_stops_an_in_flight_turn() {
    let root = scratch("sdk-cancel");
    let provider = Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.pause(1_500).text("never arrives").usage(Usage {
                    input: 5,
                    output: 1,
                    ..Usage::default()
                })
            })
            .build(),
    );
    let session = Session::create(project_of(&root), root.join("data"), provider, "faux-1")
        .expect("session starts");
    let canceller = async {
        tokio::time::sleep(Duration::from_millis(120)).await;
        session.cancel();
    };
    let (outcome, ()) = tokio::join!(session.send("long turn"), canceller);
    assert_eq!(
        outcome.stop_reason,
        lca_protocol::StopReason::Cancelled,
        "the turn reports cancellation: {outcome:?}"
    );
}

// Verifies: gh #309 acceptance - `send_with_cancel` lets the caller own the
// flag. Same behavior, the other API: a host that keeps its own `CancelFlag`
// (to share one stop button across sessions) cancels through it.
#[tokio::test]
async fn send_with_cancel_uses_the_callers_flag() {
    let root = scratch("sdk-send-with-cancel");
    let provider = Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.pause(1_500).text("never arrives").usage(Usage {
                    input: 5,
                    output: 1,
                    ..Usage::default()
                })
            })
            .build(),
    );
    let session = Session::create(project_of(&root), root.join("data"), provider, "faux-1")
        .expect("session starts");
    let flag = lca_tools::CancelFlag::new();
    let canceller = {
        let flag = flag.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            flag.cancel();
        }
    };
    let (outcome, ()) = tokio::join!(session.send_with_cancel("long turn", &flag), canceller);
    assert_eq!(
        outcome.stop_reason,
        lca_protocol::StopReason::Cancelled,
        "the caller's flag stopped the turn: {outcome:?}"
    );
    // And the session's own flag is untouched: `send` re-arms it per turn, so
    // an external cancel never leaks into the next `send`.
    let next = session.send("a fresh turn").await;
    assert_ne!(
        next.stop_reason,
        lca_protocol::StopReason::Cancelled,
        "the next turn is not pre-cancelled: {next:?}"
    );
}

// Verifies: gh #309 - a cancel with no turn in flight is harmless: the flag is
// re-armed at each `send`, so the next turn starts uncancelled.
#[tokio::test]
async fn a_cancel_before_any_turn_does_not_poison_the_next_one() {
    let root = scratch("sdk-cancel-idle");
    let session = Session::create(project_of(&root), root.join("data"), fake(), "faux-1")
        .expect("session starts");
    session.cancel();
    let outcome = session.send("hello").await;
    assert_eq!(
        outcome.stop_reason,
        lca_protocol::StopReason::Stop,
        "an idle cancel does not stick: {outcome:?}"
    );
}
