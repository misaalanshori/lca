//! Result shaping (gh #214 duration, gh #216 head-and-tail):
//! what an executed tool records and how oversized output reads.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lca_protocol::{ToolCall, ToolResultStatus};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

fn scratch(name: &str) -> PathBuf {
    lca_testkit::scratch_path(name)
}

fn executor(workspace: &Path) -> ToolExecutor {
    ToolExecutor::new(
        Arc::new(NativeOps::default()),
        workspace.to_path_buf(),
        workspace.to_path_buf(),
        65536,
        Some(Duration::from_secs(120)),
    )
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

// Verifies: gh #214 - an executed tool records its active duration
// (and the WIT-compatible mirror); refusals carry none.
#[tokio::test]
async fn executed_tools_record_duration() {
    let root = scratch("duration");
    std::fs::write(root.join("notes.md"), "three notes").expect("write");
    let mut exec = executor(&root);
    let result = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "notes.md"})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok);
    let ms = result.duration_ms.expect("duration recorded");
    assert!(
        result
            .extras
            .get("duration-ms")
            .is_some_and(|v| v == &ms.to_string()),
        "the WIT mirror matches"
    );
    let refused = lca_protocol::ToolResult::denied("c2", "no");
    assert_eq!(refused.duration_ms, None, "refusals carry none");
}

// Verifies: gh #216 - oversized output keeps head and tail with an
// accurate middle-omission marker, within the byte budget.
#[test]
fn head_tail_truncation_keeps_both_ends() {
    use lca_tools::truncate::truncate_head_tail;
    let lines: Vec<String> = (1..=2000).map(|n| format!("line {n:04}")).collect();
    let content = lines.join("\n");
    let (out, truncated) = truncate_head_tail(&content, 4096, 200, Some("attachments/abc123"));
    assert!(truncated, "oversized truncates");
    assert!(out.len() <= 4096, "within the byte budget: {}", out.len());
    assert!(out.contains("line 0001"), "head kept");
    assert!(out.contains("line 2000"), "tail kept");
    assert!(!out.contains("line 1000"), "middle omitted");
    assert!(
        out.contains("lines omitted"),
        "the marker names the omission"
    );
    assert!(out.contains("of 2000"), "the marker counts total lines");
    assert!(
        out.contains("attachments/abc123"),
        "the marker cites the attachment"
    );
    let (same, truncated) = truncate_head_tail("short", 4096, 200, None);
    assert_eq!((same.as_str(), truncated), ("short", false));
}
