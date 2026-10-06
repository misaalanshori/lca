//! Search fidelity: grep context + gitignore (gh #118), glob path +
//! limit + ignores (gh #119), list limit + pi ordering (gh #120).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lca_protocol::ToolResultStatus;
use lca_tools::{NativeOps, ToolExecutor};

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

fn call(name: &str, args: serde_json::Value) -> lca_protocol::ToolCall {
    lca_protocol::ToolCall {
        call_id: "c1".to_string(),
        name: name.to_string(),
        arguments: args.to_string(),
    }
}

async fn run(exec: &mut ToolExecutor, call: &lca_protocol::ToolCall) -> lca_protocol::ToolResult {
    exec.execute(call, &mut |_| {}, &lca_tools::CancelFlag::new())
        .await
}

// Verifies: gh #118 (gitignore): a file matched only inside an ignored
// directory is excluded from grep.
#[tokio::test]
async fn ignored_directories_are_excluded_from_grep() {
    let ws = scratch("grep-ignore");
    std::fs::create_dir_all(ws.join("ignored")).expect("mkdir");
    std::fs::write(ws.join(".gitignore"), "ignored/\n").expect("write");
    std::fs::write(ws.join("ignored/hit.txt"), "needle here\n").expect("write");
    std::fs::write(ws.join("kept.txt"), "needle here\n").expect("write");
    let mut exec = executor(&ws);
    let result = run(
        &mut exec,
        &call("grep", serde_json::json!({"pattern": "needle"})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(
        result.content.contains("kept.txt"),
        "unignored files match: {}",
        result.content
    );
    assert!(
        !result.content.contains("ignored/hit.txt"),
        "ignored files never match: {}",
        result.content
    );
}

// Verifies: gh #118 (`context: 2`): surrounding lines render in pi's
// block format - `path:line:` for the hit, `path-line-` for context.
#[tokio::test]
async fn grep_context_returns_surrounding_lines_in_pi_format() {
    let ws = scratch("grep-context");
    std::fs::write(
        ws.join("a.txt"),
        "one\ntwo\nthree\nfour HIT\nfive\nsix\nseven\n",
    )
    .expect("write");
    let mut exec = executor(&ws);
    let result = run(
        &mut exec,
        &call("grep", serde_json::json!({"pattern": "HIT", "context": 2})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    for line in [
        "a.txt-2- two",
        "a.txt-3- three",
        "a.txt:4: four HIT",
        "a.txt-5- five",
        "a.txt-6- six",
    ] {
        assert!(
            result.content.contains(line),
            "context block renders {line:?}: {}",
            result.content
        );
    }
    assert!(
        !result.content.contains("one") && !result.content.contains("seven"),
        "outside the window stays out: {}",
        result.content
    );
}

// Verifies: gh #119 (path scoping + limit): `path` roots the walk and
// `limit` caps the results with a note.
#[tokio::test]
async fn glob_scopes_to_path_and_caps_at_limit() {
    let ws = scratch("glob-path-limit");
    for name in ["a1.txt", "a2.txt", "a3.txt"] {
        std::fs::write(ws.join(name), "x").expect("write");
    }
    std::fs::create_dir_all(ws.join("sub")).expect("mkdir");
    std::fs::write(ws.join("sub/b1.txt"), "x").expect("write");
    let mut exec = executor(&ws);
    let result = run(
        &mut exec,
        &call(
            "glob",
            serde_json::json!({"pattern": "*.txt", "path": "sub"}),
        ),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(
        result.content.contains("b1.txt"),
        "the scoped root matches: {}",
        result.content
    );
    assert!(
        !result.content.contains("a1.txt"),
        "outside the root never matches: {}",
        result.content
    );

    let result = run(
        &mut exec,
        &call("glob", serde_json::json!({"pattern": "*.txt", "limit": 2})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(
        result.content.contains("2 results limit reached"),
        "the cap is named: {}",
        result.content
    );
}

// Verifies: gh #119 (ignores): gitignored files never match glob.
#[tokio::test]
async fn glob_honors_gitignore() {
    let ws = scratch("glob-ignore");
    std::fs::write(ws.join(".gitignore"), "ignored.txt\n").expect("write");
    std::fs::write(ws.join("ignored.txt"), "x").expect("write");
    std::fs::write(ws.join("kept.txt"), "x").expect("write");
    let mut exec = executor(&ws);
    let result = run(
        &mut exec,
        &call("glob", serde_json::json!({"pattern": "*.txt"})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(result.content.contains("kept.txt"), "{}", result.content);
    assert!(
        !result.content.contains("ignored.txt"),
        "ignored files never match: {}",
        result.content
    );
}

// Verifies: gh #120 (the golden fixture): alphabetical including
// dotfiles, `/`-suffixed directories, capped with pi's note.
#[tokio::test]
async fn list_matches_pi_ordering_on_the_fixture() {
    let ws = scratch("list-golden");
    for name in ["b.txt", "A.txt", ".hidden-file", "zebra"] {
        std::fs::write(ws.join(name), "x").expect("write");
    }
    std::fs::create_dir_all(ws.join("adir")).expect("mkdir");
    std::fs::create_dir_all(ws.join(".hidden-dir")).expect("mkdir");
    let mut exec = executor(&ws);
    let result = run(&mut exec, &call("list", serde_json::json!({"path": "."}))).await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert_eq!(
        result.content, ".hidden-dir/\n.hidden-file\nA.txt\nadir/\nb.txt\nzebra\n",
        "pi order: alphabetical, dotfiles in place, dirs suffixed"
    );
}

// Verifies: gh #120 (the cap): entries past `limit` are cut with pi's
// truncation note.
#[tokio::test]
async fn list_caps_entries_with_a_note() {
    let ws = scratch("list-limit");
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(ws.join(name), "x").expect("write");
    }
    let mut exec = executor(&ws);
    let result = run(
        &mut exec,
        &call("list", serde_json::json!({"path": ".", "limit": 2})),
    )
    .await;
    assert_eq!(result.status, ToolResultStatus::Ok, "{}", result.content);
    assert!(
        result.content.contains("a.txt\nb.txt\n"),
        "the first two survive: {}",
        result.content
    );
    assert!(
        !result.content.contains("c.txt"),
        "the third is cut: {}",
        result.content
    );
    assert!(
        result.content.contains("2 entries limit reached"),
        "the cap is named: {}",
        result.content
    );
}
