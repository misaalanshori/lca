//! The Embedding SDK's contract (SRDD: "creates a session, subscribes
//! to events, and sends input").

use std::sync::Arc;

use lca_protocol::{Record, Usage};
use lca_sdk::{Session, TurnEvent};
use lca_testkit::FakeProvider;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-sdk-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
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
