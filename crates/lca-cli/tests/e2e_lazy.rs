//! Lazy session creation (gh #122, EFG-034): launch-and-quit
//! writes no session directory; the first user record creates it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #122 - opening the TUI and quitting without a message
// leaves no session directory behind.
#[cfg(unix)]
#[test]
fn launch_and_quit_writes_no_session_directory() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let sandbox = sandbox("lazy-quit");
    let session = Tmux::new("lazy-quit");
    session.spawn(&sandbox, None, false, &[], &[]);
    // The orientation header shows from the working directory even
    // with nothing replayed (gh #122).
    session.wait_for("[session in", std::time::Duration::from_secs(30));
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));

    let sessions = sandbox.state_dir().join("sessions");
    let mut logs = Vec::new();
    if sessions.is_dir() {
        let mut stack = vec![sessions];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().is_some_and(|name| name == "log.jsonl") {
                    logs.push(path);
                }
            }
        }
    }
    assert!(logs.is_empty(), "no session log was written: {logs:?}");
}
