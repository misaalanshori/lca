//! Released defect (Windows, the 2026-09-25 ConPTY quarantine): after the
//! pseudoconsole fix let the TUI render on ConPTY, `/exit` stopped the app
//! from ever shutting down. Two Windows input bugs sat under it:
//!
//! * `sys::wait_stdin` returned `true` unconditionally, so the reader loop's
//!   `ReadFile` blocked with no way out and `stop()`'s thread join hung.
//! * A console input handle can signal for non-character events, so even a
//!   real wait can report readable while `ReadFile` still blocks;
//!   `stop()` now calls `CancelSynchronousIo` before joining.
//!
//! This drives the real binary on a pseudo-console and requires a clean
//! `/exit` to write `session-end`. Against the old code the process hangs and
//! no marker is ever written. Skips (never fails) when the binary is absent
//! or off Windows.
//!
//! Verifies: docs/testing-plan.md section 14 (clean quit writes
//! `session-end`); docs/platform-notes.md (Windows pty/console path).

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The `lca` binary next to this test executable's profile directory.
fn lca_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // .../target/<profile>/deps/<test>.exe -> .../target/<profile>/lca.exe
    let profile = exe.parent()?.parent()?;
    let candidate = profile.join("lca.exe");
    candidate.is_file().then_some(candidate)
}

fn find_session_end(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if find_session_end(&path) {
                return true;
            }
        } else if path.file_name().is_some_and(|n| n == "log.jsonl")
            && let Ok(text) = std::fs::read_to_string(&path)
            && text.contains("\"t\":\"session-end\"")
        {
            return true;
        }
    }
    false
}

#[test]
fn a_clean_exit_on_a_pseudo_console_writes_session_end() {
    let Some(bin) = lca_binary() else {
        eprintln!("skip: no lca.exe beside the test binary (run the workspace build)");
        return;
    };
    let root = lca_testkit::scratch_path("regression-console-exit");
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    let data = root.join("data");
    for dir in [&project, &data] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let envs: Vec<(&str, String)> = vec![
        ("HOME", s(&root)),
        ("USERPROFILE", s(&root)),
        ("XDG_DATA_HOME", s(&data)),
        ("APPDATA", s(&data)),
        ("LOCALAPPDATA", s(&data)),
        ("XDG_CONFIG_HOME", s(&root.join("config"))),
        ("LCA_UPDATE_CHECK", "false".to_string()),
    ];
    let borrowed: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (*k, v.as_str())).collect();

    let mut pty = lca_tools::PtyChild::spawn(&s(&bin), &[], &project, 40, 140, &borrowed)
        .expect("spawn the TUI on a ConPTY");

    // Wait for the first frame, then ask to leave.
    let mut screen = String::new();
    let startup = Instant::now() + Duration::from_secs(30);
    while !screen.contains("session in") && Instant::now() < startup {
        if let Ok(Some(chunk)) = pty.read(65536)
            && !chunk.is_empty()
        {
            screen.push_str(&String::from_utf8_lossy(&chunk));
        }
    }
    assert!(
        screen.contains("session in"),
        "the TUI rendered: {screen:?}"
    );
    pty.write(b"/exit\r").expect("write /exit");

    // A clean quit writes session-end; the old teardown hung instead.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ended = false;
    while Instant::now() < deadline {
        if find_session_end(&data) {
            ended = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    pty.kill();
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        ended,
        "a clean exit must write session-end (the old code hung)"
    );
}
