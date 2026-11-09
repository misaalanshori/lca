//! The Windows pseudo-console helpers for `e2e_terminal.rs`
//! (docs/testing-plan.md §14): the driver is ConPTY, the checklist is the same
//! as the tmux suite.
//!
//! Split out of `e2e_terminal.rs` for gate 11's 1,200-line ceiling (gh #543).
//! It lives under `tests/common/` rather than at `tests/`' top level on purpose:
//! cargo treats every top-level `.rs` in `tests/` as its own test **binary**, and
//! a helper module is not one - the first version of this split landed as a
//! standalone target that failed to compile (`unresolved import crate::common`).
//! Declared from `e2e_terminal.rs` as a `#[path]` module, so the rows keep every
//! name they had.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
#![cfg(windows)]

// The helpers below read the same doubles the rows do; from `tests/common/` the
// test crate's own `common` module is `crate::common`.
use crate::common::{Mock, Sandbox};

use lca_tools::PtyChild;

// ---------------------------------------------------------------------------
// Real-terminal tests on Windows (docs/testing-plan.md section 14): the TUI
// under a pseudo-console (ConPTY). The checklist is the same as the tmux
// suite; the driver is not.
// ---------------------------------------------------------------------------

/// Read the pseudo-console until `needle` appears in the accumulated screen,
/// or the deadline passes. ConPTY's read is a non-blocking peek, so this
/// polls.
#[cfg(windows)]
pub(crate) fn read_until(
    pty: &mut PtyChild,
    screen: &mut String,
    needle: &str,
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if screen.contains(needle) {
            return true;
        }
        if std::time::Instant::now() > deadline {
            return false;
        }
        match pty.read(65536) {
            Ok(Some(chunk)) if !chunk.is_empty() => {
                screen.push_str(&String::from_utf8_lossy(&chunk));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            _ => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// The sandbox environment the TUI process needs, as `PtyChild::envs`.
#[cfg(windows)]
pub(crate) fn console_env(
    sandbox: &Sandbox,
    mock: &Mock,
    with_key: bool,
) -> Vec<(&'static str, String)> {
    let home = sandbox.home.to_string_lossy().into_owned();
    let data = sandbox.data.to_string_lossy().into_owned();
    let config = sandbox.home.join(".config").to_string_lossy().into_owned();
    let mut envs = vec![
        ("HOME", home.clone()),
        ("USERPROFILE", home),
        ("XDG_DATA_HOME", data.clone()),
        ("APPDATA", data.clone()),
        ("LOCALAPPDATA", data.clone()),
        ("XDG_CONFIG_HOME", config),
        ("LCA_UPDATE_CHECK", "false".to_string()),
        ("OPENAI_BASE_URL", mock.url()),
    ];
    if with_key {
        envs.push(("OPENAI_API_KEY", "test-key".to_string()));
    }
    envs
}

#[cfg(windows)]
pub(crate) fn spawn_console(sandbox: &Sandbox, envs: &[(&'static str, String)]) -> PtyChild {
    let borrowed: Vec<(&str, &str)> = envs
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    PtyChild::spawn(
        env!("CARGO_BIN_EXE_lca"),
        &[],
        &sandbox.project(),
        40,
        140,
        &borrowed,
    )
    .expect("spawn the TUI on a ConPTY")
}

/// Serialize the Windows ConPTY tests: each spawns a whole TUI plus a mock
/// server, and running several at once starves them enough to miss the
/// shutdown deadline. One at a time is still fast (each is sub-second).
#[cfg(windows)]
pub(crate) fn conpty_serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}
