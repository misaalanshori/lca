//! Label bookmark journey (gh #37 phase 1, ADR-0046): `/label` persists
//! a bookmark, `/labels` lists it, `/jump` branches at it, and the log
//! carries the `label` record.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: FR-SESS-10 - the label verbs work end to end in a real
// terminal: add, list, jump, and the persisted record.
#[cfg(unix)]
#[test]
fn label_add_list_and_jump_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text_with_usage("first answer", 20, 0)),
        Reply::Sse(sse_text_with_usage("second answer", 20, 0)),
    ]));
    let sandbox = sandbox("labels");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("labels");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    // One turn, so the session holds a user message to bookmark.
    session.send(&["first question", "Enter"]);
    session.wait_for("first answer", std::time::Duration::from_secs(25));

    // Add: the notice names the bookmark.
    session.send(&["/label checkpoint", "Enter"]);
    session.wait_for(
        "bookmarked 'checkpoint'",
        std::time::Duration::from_secs(15),
    );

    // List: the bookmark shows with its record.
    session.send(&["/labels", "Enter"]);
    let listed = session.wait_for("checkpoint ->", std::time::Duration::from_secs(15));
    assert!(
        listed.contains("checkpoint ->"),
        "the list names the mark:\n{listed}"
    );

    // Jump: a branch forks at the mark and the TUI switches to it.
    session.send(&["/jump checkpoint", "Enter"]);
    session.wait_for(
        "branched at 'checkpoint'",
        std::time::Duration::from_secs(20),
    );

    // The logs carry the label record (FR-SESS-10's export row rides
    // the same line shape the store test pins). Two sessions exist now
    // (the jump branched), so every log is scanned: the original holds
    // the bookmark, the fork the clean exit.
    session.send(&["/exit", "Enter"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut labeled = false;
    let mut ended = false;
    while std::time::Instant::now() < deadline && !(labeled && ended) {
        labeled = false;
        ended = false;
        for log in session_logs(&sandbox.state_dir()) {
            let Ok(text) = std::fs::read_to_string(&log) else {
                continue;
            };
            labeled |= text.contains("\"t\":\"label\"") && text.contains("checkpoint");
            ended |= text.contains("\"t\":\"session-end\"");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(labeled, "label record with the bookmark persisted");
    assert!(ended, "the fork exited cleanly");
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
