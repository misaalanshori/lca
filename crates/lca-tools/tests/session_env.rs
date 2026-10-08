//! Session environment tests (gh #129): the executor's session
//! context reaches shell children as `LCA_*` variables. Split from
//! `tools.rs` at the file ceiling; behavior unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lca_protocol::{ToolCall, ToolResultStatus};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

fn scratch(name: &str) -> PathBuf {
    lca_testkit::scratch_path(name)
}

/// The executor the rows drive: **bash**, the reference dialect; a host
/// with no bash skips by name (never a silent pass).
fn bash_executor(workspace: &std::path::Path) -> Option<ToolExecutor> {
    let shell = lca_tools::shell::resolve("bash", None).ok()?;
    Some(ToolExecutor::new(
        Arc::new(NativeOps::new(shell)),
        workspace.to_path_buf(),
        workspace.to_path_buf(),
        65536,
        Some(Duration::from_secs(120)),
    ))
}

fn call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        call_id: "c1".to_string(),
        name: name.to_string(),
        arguments: args.to_string(),
        parent_call_id: None,
    }
}

async fn run(exec: &mut ToolExecutor, call: &ToolCall) -> lca_protocol::ToolResult {
    exec.execute(call, &mut |_| {}, &CancelFlag::new()).await
}

// Verifies: gh #129 (pi's session-env row) — `echo $LCA_SESSION_ID`
// prints the id: the executor's session context reaches shell
// children as `LCA_*` variables, identifiers and names only.
#[tokio::test]
async fn shell_children_see_the_session_environment() {
    let ws = scratch("session-env");
    let Some(mut exec) = bash_executor(&ws) else {
        eprintln!("skip: no bash on this host");
        return;
    };
    exec.set_session_env(Some(lca_tools::SessionEnv {
        session_id: "sess-123".to_string(),
        session_dir: ws.join("sessions").join("sess-123"),
        provider: "zen".to_string(),
        model: "zen-flash".to_string(),
        thinking: Some("high".to_string()),
        data_dir: ws.join("data"),
    }));
    let result = run(
        &mut exec,
        &call(
            "shell",
            serde_json::json!({"command": "echo $LCA_SESSION_ID/$LCA_PROVIDER/$LCA_MODEL/$LCA_THINKING; echo $LCA_SESSION_DIR; echo $LCA_DATA_DIR"}),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(
        result.content.contains("sess-123/zen/zen-flash/high"),
        "literal echo acceptance: {}",
        result.content
    );
    assert!(
        result.content.contains("sessions"),
        "the session dir rides along: {}",
        result.content
    );
    assert!(
        result.content.contains("data"),
        "the data dir rides along: {}",
        result.content
    );
}
