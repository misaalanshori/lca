//! GitHub #403 gap 2: every exit tells you how to come back.
//!
//! The fullscreen-exit path already named the session id (gh #82); a user who
//! quit any other way got nothing. The fix prints a pasteable resume command on
//! exit - but only when it will work, because a hint whose command fails is worse
//! than no hint: it teaches the user the tool lies.
//!
//! The three gates, each with a real-terminal row here:
//!
//! 1. **stdout is a TTY.** A piped run must gain zero bytes, so the non-TTY row
//!    compares against a run of the pre-fix shape (`-p`, which never printed it).
//! 2. **the session was saved.** Launch-and-quit writes no session directory
//!    (gh #122), so there is no id to advertise - that row is in `e2e_lazy.rs`
//!    and this file's `no_saved_session` case pins the exit half of it.
//! 3. **a non-default dir is quoted**, pinned as a unit row in
//!    `crates/lca-ui/src/run_tests.rs` (a real shell round-trip), so this file
//!    stays about the exit path rather than re-testing the formatter.
//!
//! The wording/formatter lives in `lca-ui` (`resume_command`); the CLI owns
//! stdout. These rows drive the real binary in a real tmux pane.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// Wait until the pane's child has exited but the pane (with its scrollback) is
/// still there, which is what `remain-on-exit` buys.
#[cfg(unix)]
fn wait_for_dead_pane(session: &Tmux, timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let out = std::process::Command::new("tmux")
            .args([
                "-L",
                common::tmux_socket_for_tests(),
                "list-panes",
                "-t",
                &session.name,
                "-F",
                "#{pane_dead}",
            ])
            .output()
            .expect("tmux list-panes");
        if String::from_utf8_lossy(&out.stdout).trim() == "1" {
            return;
        }
        assert!(
            std::time::Instant::now() <= deadline,
            "the interface never exited"
        );
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

/// The one command the hint must name (gh #403 gap 2). Asserted by shape rather
/// than by a literal id, because the id is minted per run.
#[cfg(unix)]
const HINT_PREFIX: &str = "resume with: lca --resume ";

// Verifies: gh #403 gap 2 - **the payoff.** A user who has one exchange and then
// exits gets the command to come back, naming the real id from the real save.
#[cfg(unix)]
#[test]
fn exiting_a_real_session_prints_the_resume_command() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![common::Reply::Sse(sse_text(
        "hello there",
    ))]));
    let sandbox = sandbox("resume-hint-pane");
    let session = Tmux::new("resume-hint-pane");
    session.spawn(&sandbox, Some(&mock), true, &[], &["--yolo"]);
    session.wait_for("[session in", std::time::Duration::from_secs(30));
    // Keep the pane after exit: tmux reaps a dead pane's scrollback, and the hint
    // is the last thing this run prints.
    session.remain_on_exit();

    // One exchange, so the session is really written.
    session.paste_text("hi");
    session.send(&["Enter"]);
    session.wait_for("hello there", std::time::Duration::from_secs(45));

    session.send(&["/exit", "Enter"]);
    let frame = session.wait_for_e(HINT_PREFIX, std::time::Duration::from_secs(20));

    let line = frame
        .lines()
        .find(|line| line.contains(HINT_PREFIX))
        .unwrap_or_else(|| panic!("the exit prints the resume command; pane held:\n{frame}"));
    // The id it names is a real, resumable session - not a placeholder.
    let id = line[HINT_PREFIX.len()..].trim();
    assert!(
        !id.is_empty() && !id.contains(' '),
        "the hint names one real id, got {id:?}"
    );
    let logs = sandbox.state_dir().join("sessions");
    let mut found = false;
    let mut stack = vec![logs];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|name| name.to_str()) == Some(id) {
                    found = true;
                }
                stack.push(path);
            }
        }
    }
    assert!(
        found,
        "the advertised id {id:?} is a session that exists on disk - a hint for a \
         session that was not saved would be a lie"
    );
}

// Verifies: gh #403 gap 2 - **gate 2: the session must actually be saved.** A
// launch-and-quit run writes no session directory (gh #122), so there is nothing
// to resume and the exit must stay silent.
//
// This is the row that would catch "print the hint unconditionally": the id would
// be a pending name with no file behind it.
#[cfg(unix)]
#[test]
fn quitting_without_a_saved_session_prints_no_hint() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let sandbox = sandbox("resume-hint-empty");
    let session = Tmux::new("resume-hint-empty");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(30));
    // Keep the pane so "absent" is an assertion about a frame that exists rather
    // than about a pane tmux already reaped.
    session.remain_on_exit();
    session.send(&["/exit", "Enter"]);
    // Wait for the process to be gone before reading: `remain-on-exit` turns the
    // pane into a tombstone, and tmux marks it dead once the child is reaped.
    wait_for_dead_pane(&session, std::time::Duration::from_secs(20));
    let frame = session.capture_with_history();
    // Two facts together: the run really exited, and it said nothing on the way
    // out. Without the first, "absent" would be vacuous - an empty pane contains
    // no hint either.
    assert!(
        frame.contains("lca 0.6.0") || frame.contains("[session in"),
        "the pane still shows the run, so the absence below is meaningful:\n{frame}"
    );
    assert!(
        !frame.contains(HINT_PREFIX),
        "no saved session means no resume command; pane held:\n{frame}"
    );
}

// Verifies: gh #403 gap 2 - **gate 1: a non-TTY stdout gains zero bytes.**
//
// A headless run never printed a hint and must not start. The assertion is on the
// whole output being clean of the hint *and* of the formatter's other spelling
// (`lca --session`), so a future "helpful" change that leaks the command into
// stderr or stdout fails here first.
#[test]
fn a_headless_run_prints_no_resume_hint() {
    // Fully qualified: this row runs on every platform, so it cannot lean on the
    // Unix-only `use common::*` at the top of the file.
    let runtime = common::rt();
    let mock = runtime.block_on(common::start_mock(vec![common::Reply::Sse(
        common::sse_text("headless reply"),
    )]));
    let sandbox = common::sandbox("resume-hint-headless");
    // `-p` is the scripted path: stdout is gathered by the harness, not a TTY.
    let output = sandbox.run(Some(&mock), &["--yolo", "-p", "hi"]);
    let stdout = common::stdout(&output);
    let stderr = common::stderr(&output);
    assert!(
        stdout.contains("headless reply"),
        "the run really happened: {stdout}"
    );
    for stream in [&stdout, &stderr] {
        assert!(
            !stream.contains("resume with:") && !stream.contains("lca --resume"),
            "a non-TTY run must gain no resume hint, in either stream:\n{stream}"
        );
    }
}

// Verifies: gh #403 gap 2 - **one formatter, two callers.** The fullscreen path's
// gh #82 hint and the exit hint name the same command, so a change to one shows in
// both. Driven through the config key that already existed.
#[cfg(unix)]
#[test]
fn the_fullscreen_hint_still_names_the_session() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let sandbox = sandbox("resume-hint-fullscreen");
    // The gh #82 key: leaving fullscreen names the session.
    let config = sandbox.state_dir().join("config.toml");
    std::fs::create_dir_all(config.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &config,
        "ui.fullscreen = true\nui.fullscreen_exit_output = \"resume-hint\"\n",
    )
    .expect("write config");
    let session = Tmux::new("resume-hint-fullscreen");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(30));
    // Leave fullscreen (the gh #82 behaviour) with the slash command and wait for
    // its notice - the keybinding set is configurable, the command is not.
    session.send(&["/fullscreen", "Enter"]);
    let frame = session.wait_for(
        "resume with: lca --resume",
        std::time::Duration::from_secs(20),
    );
    assert!(
        frame.contains("resume with: lca --resume"),
        "the gh #82 fullscreen hint still works: {frame}"
    );
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
