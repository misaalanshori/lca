//! Tree navigation and rename journeys (gh #37 phase 3, FR-UI-16):
//! `/tree` browses the entry tree and branches in place (one log),
//! `/rename` trails the session-info entry.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: FR-UI-16 - tree browse, in-place branch, and rename in a
// real terminal, with the markers persisted in the one log.
#[cfg(unix)]
#[test]
fn tree_browse_branch_and_rename_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text_with_usage("answer one", 20, 0)),
        Reply::Sse(sse_text_with_usage("answer two", 20, 0)),
        Reply::Sse(sse_text_with_usage("answer three", 20, 0)),
    ]));
    let sandbox = sandbox("tree");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("tree");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    // Two turns, so the tree has two message rows.
    session.send(&["alpha question", "Enter"]);
    session.wait_for("answer one", std::time::Duration::from_secs(25));
    session.send(&["beta question", "Enter"]);
    session.wait_for("answer two", std::time::Duration::from_secs(25));

    // Browse: both questions show as tree rows.
    session.send(&["/tree", "Enter"]);
    session.wait_for("alpha question", std::time::Duration::from_secs(15));

    // Branch at the first row (a real rewind): in place, no session.
    session.send(&["Enter"]);
    session.wait_for("branched at", std::time::Duration::from_secs(20));

    // The branch continues with a fresh turn.
    session.send(&["gamma question", "Enter"]);
    session.wait_for("answer three", std::time::Duration::from_secs(25));

    // Rename trails the session-info entry.
    session.send(&["/rename tree title", "Enter"]);
    session.wait_for(
        "renamed to 'tree title'",
        std::time::Duration::from_secs(15),
    );

    // One log carries the jump, the branch child, and the rename.
    session.send(&["/exit", "Enter"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut jumped = false;
    let mut renamed = false;
    let mut one_log = false;
    while std::time::Instant::now() < deadline && !(jumped && renamed && one_log) {
        jumped = false;
        renamed = false;
        let mut logs = vec![];
        for log in session_logs(&sandbox.state_dir()) {
            let Ok(text) = std::fs::read_to_string(&log) else {
                continue;
            };
            jumped |= text.contains("\"t\":\"branch-point\"");
            renamed |= text.contains("\"t\":\"session-info\"") && text.contains("tree title");
            if text.contains("\"t\":\"session-end\"") {
                logs.push(text);
            }
        }
        one_log = logs.len() == 1;
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(jumped, "branch-point persisted");
    assert!(renamed, "session-info persisted");
    assert!(one_log, "in-place branch keeps one log");
}

#[cfg(unix)]
fn session_logs(state: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.file_name().is_some_and(|name| name == "log.jsonl") {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(state, &mut found);
    found
}
