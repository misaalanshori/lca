//! Built-in tool tests: the seven tools (FR-TOOL-1), staleness (FR-TOOL-2),
//! permission surface (FR-TOOL-3), streaming (FR-TOOL-4), timeout tree-kill
//! (FR-TOOL-5), platform shell (FR-TOOL-6), truncation (FR-TOOL-7).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_permissions::Action;
use lca_protocol::{ToolCall, ToolResultStatus};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-tools-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn executor(workspace: &Path) -> ToolExecutor {
    ToolExecutor::new(
        Arc::new(NativeOps),
        workspace.to_path_buf(),
        workspace.to_path_buf(),
        65536,
        Duration::from_secs(120),
    )
}

fn call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        call_id: "c1".to_string(),
        name: name.to_string(),
        arguments: args.to_string(),
    }
}

async fn run(exec: &mut ToolExecutor, call: &ToolCall) -> lca_protocol::ToolResult {
    exec.execute(call, &mut |_| {}, &CancelFlag::new()).await
}

// Verifies: FR-TOOL-1 (the seven built-in tools exist with schemas)
#[test]
fn ships_exactly_the_documented_builtin_tools() {
    let specs = ToolExecutor::specs();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    for expected in ["read", "write", "edit", "list", "glob", "grep", "shell"] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
    for spec in ToolExecutor::specs() {
        assert!(
            spec.parameters.is_object(),
            "{} carries a JSON schema",
            spec.name
        );
        assert!(!spec.description.is_empty());
    }
}

// Verifies: FR-TOOL-1 (read returns content with line numbers and offsets)
#[tokio::test]
async fn read_shows_line_numbers_and_honours_offset() {
    let ws = scratch("read");
    std::fs::write(ws.join("a.txt"), "alpha\nbeta\ngamma\n").expect("write");
    let mut exec = executor(&ws);

    let result = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "a.txt"})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    let lines: Vec<&str> = result.content.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("alpha"));
    assert!(
        lines[0].trim_start().starts_with("1"),
        "line numbers: {}",
        lines[0]
    );
    assert!(lines[2].trim_start().starts_with("3"));

    let result = run(
        &mut exec,
        &call(
            "read",
            serde_json::json!({"path": "a.txt", "offset": 2, "limit": 1}),
        ),
    )
    .await;
    assert!(result.content.contains("beta"));
    assert!(!result.content.contains("gamma"), "limit honored");
    assert!(!result.content.contains("alpha"), "offset honored");
}

#[tokio::test]
async fn read_past_the_end_reports_an_error_to_the_model() {
    let ws = scratch("read-oob");
    std::fs::write(ws.join("a.txt"), "one\n").expect("write");
    let mut exec = executor(&ws);
    let result = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "a.txt", "offset": 99})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(result.content.contains("beyond"), "{}", result.content);
}

// Verifies: FR-TOOL-7 (results over the configured limit truncate and are
// marked)
#[tokio::test]
async fn read_truncates_over_the_limit_and_marks_it() {
    let ws = scratch("read-trunc");
    let big = "line\n".repeat(10_000);
    std::fs::write(ws.join("big.txt"), &big).expect("write");
    let mut exec = ToolExecutor::new(
        Arc::new(NativeOps),
        ws.clone(),
        ws.clone(),
        1024,
        Duration::from_secs(120),
    );
    let result = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "big.txt"})),
    )
    .await;
    assert!(result.truncated, "truncated flag set (FR-TOOL-7)");
    assert!(result.content.len() < big.len());
    assert!(
        result.content.contains("[Showing lines"),
        "visible marker: {}",
        &result.content[result.content.len().saturating_sub(200)..]
    );
    assert!(
        result.content.contains("offset="),
        "actionable continuation"
    );
}

// Verifies: FR-TOOL-1 (write creates or replaces a file)
#[tokio::test]
async fn write_creates_and_replaces() {
    let ws = scratch("write");
    let mut exec = executor(&ws);
    let result = run(
        &mut exec,
        &call(
            "write",
            serde_json::json!({"path": "sub/dir/new.txt", "content": "hello"}),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(
        std::fs::read_to_string(ws.join("sub/dir/new.txt")).expect("read"),
        "hello"
    );

    let result = run(
        &mut exec,
        &call(
            "write",
            serde_json::json!({"path": "sub/dir/new.txt", "content": "replaced"}),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok);
    assert_eq!(
        std::fs::read_to_string(ws.join("sub/dir/new.txt")).expect("read"),
        "replaced"
    );
}

// Verifies: FR-TOOL-2 (an edit of a file that changed since the last read
// is rejected with an error back to the model)
#[tokio::test]
async fn edit_rejects_a_file_that_changed_since_the_last_read() {
    let ws = scratch("edit-stale");
    std::fs::write(ws.join("a.txt"), "original").expect("write");
    let mut exec = executor(&ws);

    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "a.txt",
                "edits": [{"oldText": "original", "newText": "edited"}]
            }),
        ),
    )
    .await;
    assert_eq!(
        result.status,
        ToolResultStatus::Error,
        "no read yet: {}",
        result.content
    );
    assert!(result.content.contains("read"), "{}", result.content);

    let read = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "a.txt"})),
    )
    .await;
    assert_eq!(read.status, ToolResultStatus::Ok);

    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "a.txt",
                "edits": [{"oldText": "original", "newText": "edited"}]
            }),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(
        std::fs::read_to_string(ws.join("a.txt")).expect("read"),
        "edited"
    );

    // Something else changes the file after our read.
    let read = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "a.txt"})),
    )
    .await;
    assert_eq!(read.status, ToolResultStatus::Ok);
    std::fs::write(ws.join("a.txt"), "externally changed").expect("external write");
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "a.txt",
                "edits": [{"oldText": "externally", "newText": "locally"}]
            }),
        ),
    )
    .await;
    assert_eq!(
        result.status,
        ToolResultStatus::Error,
        "stale: {}",
        result.content
    );
    assert!(
        result.content.contains("changed since"),
        "{}",
        result.content
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("a.txt")).expect("read"),
        "externally changed",
        "no write happened"
    );
}

// Edits match against the original file, never incrementally, and every
// oldText must be unique (the editing contract the model relies on).
#[tokio::test]
async fn edits_match_against_the_original_uniquely() {
    let ws = scratch("edit-unique");
    std::fs::write(ws.join("a.rs"), "fn a() {}\nfn b() {}\n").expect("write");
    let mut exec = executor(&ws);
    let _ = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "a.rs"})),
    )
    .await;

    // Non-unique old text is rejected.
    std::fs::write(ws.join("b.txt"), "x x").expect("write");
    let _ = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "b.txt"})),
    )
    .await;
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "b.txt",
                "edits": [{"oldText": "x", "newText": "y"}]
            }),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Error, "{}", result.content);

    // Two disjoint edits both see the original content.
    let result = run(
        &mut exec,
        &call(
            "edit",
            serde_json::json!({
                "path": "a.rs",
                "edits": [
                    {"oldText": "fn a() {}", "newText": "fn aa() {}"},
                    {"oldText": "fn b() {}", "newText": "fn bb() {}"}
                ]
            }),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(
        std::fs::read_to_string(ws.join("a.rs")).expect("read"),
        "fn aa() {}\nfn bb() {}\n"
    );
}

// Verifies: FR-TOOL-1 (list, glob, grep search the workspace)
#[tokio::test]
async fn list_glob_and_grep_search_the_workspace() {
    let ws = scratch("search");
    std::fs::create_dir_all(ws.join("src")).expect("mkdir");
    std::fs::write(ws.join("src/main.rs"), "fn main() { todo!() }\n").expect("write");
    std::fs::write(ws.join("README.md"), "# title\nnothing here\n").expect("write");
    std::fs::write(ws.join("src/util.rs"), "fn helper() {}\n").expect("write");
    let mut exec = executor(&ws);

    let result = run(&mut exec, &call("list", serde_json::json!({"path": "."}))).await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(
        result.content.contains("src/"),
        "directories marked: {}",
        result.content
    );
    assert!(result.content.contains("README.md"));

    let result = run(
        &mut exec,
        &call("glob", serde_json::json!({"pattern": "**/*.rs"})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(result.content.contains("src/main.rs"));
    assert!(result.content.contains("src/util.rs"));
    assert!(!result.content.contains("README.md"));

    let result = run(
        &mut exec,
        &call("glob", serde_json::json!({"pattern": "*.md"})),
    )
    .await;
    assert!(
        result.content.contains("README.md"),
        "single-segment wildcard: {}",
        result.content
    );
    assert!(
        !result.content.contains("src/"),
        "*.md does not cross directories"
    );

    let result = run(
        &mut exec,
        &call("grep", serde_json::json!({"pattern": "fn (main|helper)"})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(result.content.contains("src/main.rs"));
    assert!(result.content.contains("src/util.rs"));
    assert!(
        result.content.contains(":1:"),
        "path:line:text shape: {}",
        result.content
    );
    assert!(!result.content.contains("README"), "regex semantics");

    let result = run(
        &mut exec,
        &call(
            "grep",
            serde_json::json!({"pattern": "nothing here", "literal": true}),
        ),
    )
    .await;
    assert!(
        result.content.contains("README.md:2:"),
        "literal search: {}",
        result.content
    );

    let result = run(
        &mut exec,
        &call("grep", serde_json::json!({"pattern": "zzz_absent"})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok);
    assert!(
        result.content.contains("No matches"),
        "empty result is explicit: {}",
        result.content
    );
}

// Verifies: FR-TOOL-3 (shell always asks; writes outside the workspace ask;
// in-workspace writes do not)
#[test]
fn permission_surface_matches_the_requirement() {
    let ws = scratch("perm");
    let exec = executor(&ws);

    let shell = call("shell", serde_json::json!({"command": "ls"}));
    assert!(
        matches!(exec.required_permission(&shell), Some(Action::Shell { .. })),
        "every command passes the permission layer"
    );

    let write_in = call(
        "write",
        serde_json::json!({"path": "ok.txt", "content": "x"}),
    );
    assert!(
        exec.required_permission(&write_in).is_none(),
        "workspace writes are ungated"
    );

    let write_out = call(
        "write",
        serde_json::json!({"path": "../escape.txt", "content": "x"}),
    );
    assert!(
        matches!(
            exec.required_permission(&write_out),
            Some(Action::WritePath { .. })
        ),
        "outside the workspace asks first (FR-TOOL-3)"
    );

    let read_anywhere = call("read", serde_json::json!({"path": "../neighbor/notes.md"}));
    assert!(exec.required_permission(&read_anywhere).is_none());
}

// Verifies: FR-TOOL-4 (output streams to the interface while running)
#[tokio::test]
async fn shell_streams_output_while_running() {
    let ws = scratch("stream");
    let mut exec = executor(&ws);
    let chunks: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_chunks = chunks.clone();
    let mut sink = move |bytes: &[u8]| {
        sink_chunks
            .lock()
            .expect("lock")
            .push(String::from_utf8_lossy(bytes).into_owned());
    };
    let call = call(
        "shell",
        serde_json::json!({"command": "printf 'one\\ntwo\\nthree\\n'"}),
    );
    let result = exec.execute(&call, &mut sink, &CancelFlag::new()).await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    let streamed: String = chunks.lock().expect("lock").join("");
    assert!(
        streamed.contains("one"),
        "chunks arrive during the run: {streamed:?}"
    );
    assert!(streamed.contains("three"));
    assert_eq!(
        result.content.matches("two").count(),
        1,
        "final result holds the full output once"
    );
}

// Verifies: FR-TOOL-4 (a failing command reports its output and exit code)
#[tokio::test]
async fn shell_reports_exit_codes_with_output() {
    let ws = scratch("exit");
    let mut exec = executor(&ws);
    let result = run(
        &mut exec,
        &call(
            "shell",
            serde_json::json!({"command": "echo partial; exit 3"}),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(result.content.contains("partial"), "{}", result.content);
    assert!(result.content.contains("code 3"), "{}", result.content);
}

// Verifies: FR-TOOL-5 (a timeout stops the command's process tree and
// returns a timeout error)
#[tokio::test]
async fn shell_timeout_kills_the_process_tree() {
    let ws = scratch("timeout");
    let mut exec = ToolExecutor::new(
        Arc::new(NativeOps),
        ws.clone(),
        ws.clone(),
        65536,
        Duration::from_millis(500),
    );
    let started = std::time::Instant::now();
    let result = run(
        &mut exec,
        &call(
            "shell",
            serde_json::json!({"command": "sleep 30 & echo started-child $!; wait"}),
        ),
    )
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the timeout fires"
    );
    assert_eq!(
        result.status,
        ToolResultStatus::Timeout,
        "{}",
        result.content
    );
    assert!(result.content.contains("timed out"), "{}", result.content);

    // The background grandchild is gone too, not just the shell.
    if cfg!(unix) {
        let pid = result
            .content
            .lines()
            .find_map(|l| l.split_whitespace().last())
            .and_then(|token| token.parse::<i32>().ok())
            .expect("child pid echoed");
        std::thread::sleep(Duration::from_millis(300));
        let status = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .expect("kill -0");
        assert!(
            !status.success(),
            "grandchild {pid} must be dead (FR-TOOL-5)"
        );
    }
}

// Verifies: FR-TOOL-6 (on Windows the command runs through the platform
// shell; on unix through /bin/sh-family shells)
#[tokio::test]
async fn shell_uses_the_platform_shell() {
    let ws = scratch("platform");
    let mut exec = executor(&ws);
    if cfg!(target_os = "windows") {
        let result = run(
            &mut exec,
            &call("shell", serde_json::json!({"command": "echo %OS%"})),
        )
        .await;
        assert_eq!(
            result.status,
            ToolResultStatus::Ok,
            "cmd syntax works: {}",
            result.content
        );
        assert!(result.content.to_lowercase().contains("windows"));
    } else {
        let result = run(
            &mut exec,
            &call(
                "shell",
                serde_json::json!({"command": "echo $0 | head -c 200"}),
            ),
        )
        .await;
        assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    }
}

// Verifies: FR-TOOL-7 (shell output over the limit truncates with a marker)
#[tokio::test]
async fn shell_output_truncates_and_marks() {
    let ws = scratch("shell-trunc");
    let mut exec = ToolExecutor::new(
        Arc::new(NativeOps),
        ws.clone(),
        ws.clone(),
        256,
        Duration::from_secs(30),
    );
    let result = run(
        &mut exec,
        &call(
            "shell",
            serde_json::json!({"command": "for i in $(seq 1 500); do echo \"line $i\"; done"}),
        ),
    )
    .await;
    assert!(result.truncated, "truncated flag set");
    assert!(result.content.contains("truncated"), "visible marker");
    assert!(
        result.content.contains("line 500"),
        "shell truncation keeps the tail, where errors live"
    );
    assert!(!result.content.contains("line 1\n"), "head dropped");
}

// Cancellation stops a running command promptly (FR-CONC-3's shell branch).
#[tokio::test]
async fn cancellation_stops_a_running_command() {
    let ws = scratch("cancel");
    let cancel = CancelFlag::new();
    let flag = cancel.clone();
    let handle = tokio::spawn(async move {
        let mut exec = executor(&ws);
        exec.execute(
            &call("shell", serde_json::json!({"command": "sleep 30"})),
            &mut |_| {},
            &flag,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("returns promptly")
        .expect("join");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_ne!(
        result.status,
        ToolResultStatus::Ok,
        "cancelled command is not success"
    );
}
