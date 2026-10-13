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

// Verifies: gh #219 - reading a file under a subfolder AGENTS.md
// injects its instructions once per session, nearest first.
#[tokio::test]
async fn subfolder_agents_load_on_read() {
    let root = scratch("agents-disc");
    std::fs::create_dir_all(root.join("sub/deep")).expect("mkdir");
    std::fs::write(root.join("AGENTS.md"), "root rules").expect("write");
    std::fs::write(root.join("sub/AGENTS.md"), "sub rules").expect("write");
    std::fs::write(root.join("sub/deep/note.md"), "hello").expect("write");
    let mut exec = executor(&root);
    let first = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "sub/deep/note.md"})),
    )
    .await;
    assert_eq!(first.status, ToolResultStatus::Ok);
    assert!(first.content.contains("sub rules"), "{}", first.content);
    assert!(
        !first.content.contains("root rules"),
        "the root file already loaded at startup: {}",
        first.content
    );
    let second = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "sub/deep/note.md"})),
    )
    .await;
    assert!(
        !second.content.contains("sub rules"),
        "injected at most once: {}",
        second.content
    );
}

// Verifies: gh #219 - paths outside the workspace never inject.
#[tokio::test]
async fn outside_paths_never_inject() {
    let root = scratch("agents-outside");
    std::fs::create_dir_all(&root).expect("mkdir");
    let outside = lca_testkit::scratch_path("agents-outside-ext");
    std::fs::create_dir_all(&outside).expect("mkdir");
    std::fs::write(outside.join("AGENTS.md"), "evil rules").expect("write");
    std::fs::write(outside.join("note.md"), "hello").expect("write");
    let mut exec = executor(&root);
    let result = run(
        &mut exec,
        &call(
            "read",
            serde_json::json!({"path": outside.join("note.md").to_string_lossy()}),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok);
    assert!(!result.content.contains("evil rules"), "{}", result.content);
}

// Verifies: gh #219 - nested instruction files inject nearest first,
// and a directory listing discovers its own directory.
#[tokio::test]
async fn nested_agents_inject_nearest_first() {
    let root = scratch("agents-nest");
    std::fs::create_dir_all(root.join("sub/deep")).expect("mkdir");
    std::fs::write(root.join("sub/AGENTS.md"), "sub rules").expect("write");
    std::fs::write(root.join("sub/deep/AGENTS.md"), "deep rules").expect("write");
    std::fs::write(root.join("sub/deep/note.md"), "hello").expect("write");
    let mut exec = executor(&root);
    let result = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "sub/deep/note.md"})),
    )
    .await;
    let deep = result.content.find("deep rules").expect("deep injects");
    let sub = result.content.find("sub rules").expect("sub injects");
    assert!(deep < sub, "nearest first:\n{}", result.content);
    let mut fresh = executor(&root);
    let listed = run(
        &mut fresh,
        &call("list", serde_json::json!({"path": "sub/deep"})),
    )
    .await;
    assert_eq!(listed.status, ToolResultStatus::Ok);
    assert!(
        listed.content.contains("deep rules"),
        "a listing discovers its own directory:\n{}",
        listed.content
    );
}
