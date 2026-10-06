//! Tool-result parity: the interpreter ladder resolves on every platform,
//! a failed command reports its exit code as data, and oversized results
//! are marked truncated. (Pi's `bash` result shape, same contract.)

use std::sync::Arc;
use std::time::Duration;

use lca_protocol::{ToolCall, ToolResultStatus};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

fn executor(workspace: &std::path::Path, limit: usize) -> ToolExecutor {
    ToolExecutor::new(
        Arc::new(NativeOps::default()),
        workspace.to_path_buf(),
        workspace.to_path_buf(),
        limit,
        Some(Duration::from_secs(30)),
    )
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = lca_testkit::scratch_path(&format!("pi-parity-tools-{name}"));
    std::fs::create_dir_all(dir.join("project")).expect("mkdir");
    dir.join("project")
}

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        call_id: "call-1".to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

// Verifies: pi:packages/coding-agent/docs/settings.md (`shellPath`
// platform default — there is always an explicit interpreter).
#[test]
fn pi_parity_shell_ladder_resolves_an_interpreter() {
    let shell = lca_tools::shell::resolve("auto", None).expect("the ladder resolves");
    assert!(
        !shell.describe().is_empty(),
        "the model is told which interpreter it got"
    );
}

// Verifies: pi:packages/coding-agent/docs/codemode.md (`bash` resolves to
// a value carrying the exit code, also for non-zero exits — a failed
// command is data, not a crash).
#[tokio::test]
async fn pi_parity_shell_result_reports_exit_status() {
    let workspace = scratch("exit");
    let mut exec = executor(&workspace, 65536);
    let mut chunks = Vec::new();
    let mut on_output = |bytes: &[u8]| chunks.extend_from_slice(bytes);
    // `exit 3` is valid in sh, bash, cmd, and PowerShell alike.
    let result = exec
        .execute(
            &call("shell", serde_json::json!({"command": "exit 3"})),
            &mut on_output,
            &CancelFlag::new(),
        )
        .await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(
        result.content.contains("code 3"),
        "unexpected content: {}",
        result.content
    );
}

// Verifies: pi:packages/coding-agent/docs/codemode.md (a capped tool
// result says it is truncated and keeps the tail the model saw).
#[tokio::test]
async fn pi_parity_tool_result_marks_truncation() {
    let workspace = scratch("truncate");
    let mut exec = executor(&workspace, 64);
    let big = "x".repeat(200);
    let cancel = CancelFlag::new();
    let mut sink = |_: &[u8]| {};
    let written = exec
        .execute(
            &call(
                "write",
                serde_json::json!({"path": "big.txt", "content": big}),
            ),
            &mut sink,
            &cancel,
        )
        .await;
    assert_eq!(written.status, ToolResultStatus::Ok);
    let result = exec
        .execute(
            &call("read", serde_json::json!({"path": "big.txt"})),
            &mut sink,
            &cancel,
        )
        .await;
    assert_eq!(result.status, ToolResultStatus::Ok);
    assert!(result.truncated, "an over-limit result is marked");
}
