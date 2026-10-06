//! GitHub #116: `edit` refuses non-UTF-8 files instead of corrupting them.
//!
//! The edit path decoded with `from_utf8_lossy` and wrote the result
//! back: invalid sequences became U+FFFD outside the edited region.
//! Strict decode at the entry returns a clear tool error and leaves the
//! file's bytes untouched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;
use std::time::Duration;

use lca_protocol::ToolCall;

fn executor(workspace: &std::path::Path) -> lca_tools::ToolExecutor {
    lca_tools::ToolExecutor::new(
        Arc::new(lca_tools::NativeOps::default()),
        workspace.to_path_buf(),
        workspace.to_path_buf(),
        65536,
        Some(Duration::from_secs(30)),
    )
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = lca_testkit::scratch_path(&format!("gh116-edit-{name}"));
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

fn run(exec: &mut lca_tools::ToolExecutor, call: &ToolCall) -> lca_protocol::ToolResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let mut sink = |_: &[u8]| {};
            exec.execute(call, &mut sink, &lca_tools::CancelFlag::new())
                .await
        })
}

fn read_first(exec: &mut lca_tools::ToolExecutor, file: &str) {
    let result = run(exec, &call("read", serde_json::json!({"path": file})));
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Ok);
}

// Verifies: #116 (a Latin-1 file errors and its bytes are unchanged).
#[test]
fn gh116_edit_refuses_latin1_and_leaves_bytes_untouched() {
    let workspace = scratch("latin1");
    // "caf\xe9" in Latin-1: the \xe9 byte is invalid UTF-8.
    let bytes = b"caf\xe9 au lait\nsecond line\n".to_vec();
    std::fs::write(workspace.join("note.txt"), &bytes).expect("write");
    let mut exec = executor(&workspace);
    read_first(&mut exec, "note.txt");
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({"path": "note.txt", "edits": [{"oldText": "second", "newText": "2nd"}]}),
        ),
    );
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Error);
    assert!(
        result.content.contains("UTF-8"),
        "the error says why: {}",
        result.content
    );
    assert_eq!(
        std::fs::read(workspace.join("note.txt")).expect("read back"),
        bytes,
        "the file's bytes are untouched"
    );
}

// Verifies: #116 (a Shift-JIS-shaped row errors the same way — the check
// is on the bytes, not the encoding name).
#[test]
fn gh116_edit_refuses_shift_jis_shaped_bytes() {
    let workspace = scratch("shiftjis");
    // U+3042 in Shift-JIS is \x82\xa0: lead + trail, invalid UTF-8.
    let bytes = b"\x82\xa0 plain\n".to_vec();
    std::fs::write(workspace.join("note.txt"), &bytes).expect("write");
    let mut exec = executor(&workspace);
    read_first(&mut exec, "note.txt");
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({"path": "note.txt", "edits": [{"oldText": "plain", "newText": "x"}]}),
        ),
    );
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Error);
    assert_eq!(
        std::fs::read(workspace.join("note.txt")).expect("read back"),
        bytes,
        "the file's bytes are untouched"
    );
}
