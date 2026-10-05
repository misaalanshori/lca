//! GitHub issue #14 (open): the SDK embedders' approval surface. The
//! permission machinery exists host-side; `Session::with_permission_prompt`
//! receives the host's approval callback (the action verbatim as the TUI
//! modal shows it in, once/always/deny out) and the host's `authorize`
//! path calls it wherever the interactive UI would open the modal.
//! Without a callback the session declines and records (deny-by-default,
//! NFR-13).

use std::sync::{Arc, Mutex};

use lca_testkit::{FakeProvider, fake_usage};

fn provider() -> FakeProvider {
    FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"echo hi"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 10, 0)))
        .build()
}

fn sandbox(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = lca_testkit::scratch_path(&format!("lca-gh14-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let cwd = root.join("project");
    let data = root.join("data");
    std::fs::create_dir_all(&cwd).expect("mkdir");
    std::fs::create_dir_all(&data).expect("mkdir");
    (cwd, data)
}

fn permission_records(
    data: &std::path::Path,
    cwd: &std::path::Path,
    id: &str,
) -> Vec<lca_protocol::Record> {
    let store = lca_session::SessionStore::new(data.to_path_buf());
    let session = store.session(cwd, id).expect("reopen");
    store
        .read(&session)
        .expect("read")
        .records
        .into_iter()
        .filter(|record| matches!(record, lca_protocol::Record::Permission { .. }))
        .collect()
}

// An allow answer governs execution: the fake host's callback sees the
// shell request verbatim, the command runs, and the log carries the same
// `Once` permission record the modal path writes.
#[tokio::test]
async fn an_allow_answer_runs_the_command_and_records_once() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let asked = seen.clone();
    let (cwd, data) = sandbox("allow");
    let session = lca_sdk::Session::create(cwd.clone(), data.clone(), Arc::new(provider()), "")
        .expect("session")
        .with_permission_prompt(move |action: String| {
            asked
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(action);
            lca_permissions::Decision::Once
        });
    let mut events = session.subscribe();
    let outcome = session.send("run it").await;
    assert_eq!(outcome.status, lca_protocol::TurnStatus::Ok);
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|action| action.contains("echo hi")),
        "the callback saw the request verbatim: {:?}",
        seen.lock().unwrap()
    );
    let mut ran = false;
    while let Ok(event) = events.try_recv() {
        if let lca_protocol::TurnEvent::ToolFinished(result) = event
            && result.status == lca_protocol::ToolResultStatus::Ok
            && result.content.contains("hi")
        {
            ran = true;
        }
    }
    assert!(ran, "the approved command ran");
    let records = permission_records(&data, &cwd, session.id());
    assert!(
        records.iter().any(|record| matches!(
            record,
            lca_protocol::Record::Permission {
                decision: lca_protocol::PermissionDecision::Once,
                ..
            }
        )),
        "the modal path's record, decision Once: {records:?}"
    );
}

// A deny answer refuses with the same denied record the modal path
// writes, and the command never runs.
#[tokio::test]
async fn a_deny_answer_refuses_and_records_denied() {
    let (cwd, data) = sandbox("deny");
    let session = lca_sdk::Session::create(cwd.clone(), data.clone(), Arc::new(provider()), "")
        .expect("session")
        .with_permission_prompt(|_action: String| lca_permissions::Decision::Denied);
    let mut events = session.subscribe();
    let outcome = session.send("run it").await;
    assert_eq!(outcome.status, lca_protocol::TurnStatus::Ok);
    let mut refused = false;
    let mut ran = false;
    while let Ok(event) = events.try_recv() {
        if let lca_protocol::TurnEvent::ToolFinished(result) = event {
            if result.status == lca_protocol::ToolResultStatus::Denied {
                refused = true;
            }
            // Only a run produces an Ok result; the denial message
            // itself names the command, so content alone cannot tell.
            if result.status == lca_protocol::ToolResultStatus::Ok {
                ran = true;
            }
        }
    }
    assert!(refused, "the denial reaches the model as denied");
    assert!(!ran, "the refused command never ran");
    let records = permission_records(&data, &cwd, session.id());
    assert!(
        records.iter().any(|record| matches!(
            record,
            lca_protocol::Record::Permission {
                decision: lca_protocol::PermissionDecision::Denied,
                ..
            }
        )),
        "the modal path's record, decision Denied: {records:?}"
    );
}

// No callback registered: the session declines and records, exactly
// like headless mode (deny-by-default, NFR-13).
#[tokio::test]
async fn the_unregistered_default_declines_and_records() {
    let (cwd, data) = sandbox("default");
    let session = lca_sdk::Session::create(cwd.clone(), data.clone(), Arc::new(provider()), "")
        .expect("session");
    let mut events = session.subscribe();
    let outcome = session.send("run it").await;
    assert_eq!(outcome.status, lca_protocol::TurnStatus::Ok);
    let mut refused = false;
    while let Ok(event) = events.try_recv() {
        if let lca_protocol::TurnEvent::ToolFinished(result) = event
            && result.status == lca_protocol::ToolResultStatus::Denied
        {
            refused = true;
        }
    }
    assert!(refused, "the default declines without a callback");
    let records = permission_records(&data, &cwd, session.id());
    assert!(
        records.iter().any(|record| matches!(
            record,
            lca_protocol::Record::Permission {
                decision: lca_protocol::PermissionDecision::Denied,
                ..
            }
        )),
        "the decline is recorded: {records:?}"
    );
}
