//! tmux socket names, the directory they live in, and reaping stale ones
//! (gh #413's class).
//!
//! Split out of `common/mod.rs` when the harness crossed gate 11's ceiling.
//! The contract is documented here rather than at the call sites: one socket
//! name per test binary, derived from that binary's pid; the directory is asked
//! of *our* socket and never the default one; sockets left by dead test
//! processes are reaped by the next run.

use std::process::Command;

#[cfg(unix)]
pub(crate) fn tmux_socket() -> &'static str {
    static SOCKET: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SOCKET.get_or_init(|| format!("lca-tui-test-{}", std::process::id()))
}

/// The socket this binary's sessions live on (the contract rows read it).
#[cfg(unix)]
pub fn tmux_socket_for_tests() -> &'static str {
    tmux_socket()
}

/// The socket path this binary's tmux server uses, as tmux itself names it.
///
/// Exposed for the contract row: it is the *only* place the socket directory
/// is derived from, so a row can assert the answer belongs to our socket.
#[cfg(unix)]
pub fn socket_path_for_tests() -> Option<std::path::PathBuf> {
    let out = Command::new("tmux")
        .args([
            "-L",
            tmux_socket(),
            "display-message",
            "-p",
            "#{socket_path}",
        ])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let named = if stdout.is_empty() {
        stderr
            .trim()
            .strip_prefix("error connecting to ")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_default()
            .to_string()
    } else {
        stdout
    };
    (!named.is_empty()).then(|| std::path::PathBuf::from(named))
}

/// The directory tmux keeps its sockets in.
///
/// Asked of **our own** socket, never the default one: `display-message` on a
/// name that is not running still prints `error connecting to <path>`, and
/// that path is exactly the answer. One call covers both cases — a live server
/// prints `#{socket_path}` on stdout, a missing one names the same path on
/// stderr — and neither touches the default socket, which the harness must
/// never race two binaries on (the standing rule this harness exists to keep;
/// an earlier version of this function broke it, 2026-10-21 review).
///
/// Falls back to the documented `$TMUX_TMPDIR`-or-`/tmp` layout when the text
/// does not parse. Reaping is then skipped rather than guessed: a wrong
/// directory would delete someone else's sockets, which is worse than leaving
/// our own behind.
#[cfg(unix)]
pub fn socket_dir_for_tests() -> Option<std::path::PathBuf> {
    let out = Command::new("tmux")
        .args([
            "-L",
            tmux_socket(),
            "display-message",
            "-p",
            "#{socket_path}",
        ])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let named = if stdout.trim().is_empty() {
        // `error connecting to <path> (No such file or directory)` — the path
        // is the last whitespace-free token before the parenthesised reason.
        stderr
            .trim()
            .strip_prefix("error connecting to ")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_default()
            .to_string()
    } else {
        stdout.trim().to_string()
    };
    if named.is_empty() {
        return None;
    }
    std::path::Path::new(&named)
        .parent()
        .map(|dir| dir.to_path_buf())
}

/// Reap socket files left by test binaries that are no longer running.
///
/// A test process is not unwound when nextest finishes a binary, so a `Drop`
/// on the harness never fires and the server outlives its tests: this box
/// reached 134 (then 116) leftover `lca-tui-test-<pid>` sockets. Reaping from
/// the *next* run is the reliable point — every process that owns one of these
/// sockets is gone, so the name is dead by construction — and it costs one
/// directory read per test binary.
///
/// Scoped to this harness's own prefix, so a live drive (`lca-tui`) or another
/// tool's sockets are never touched.
#[cfg(unix)]
/// Whether a process id is still running, portably.
///
/// `kill(pid, 0)` is the POSIX idiom and works on every Unix we ship; doing it
/// through the shell keeps this file free of an `unsafe` libc call. A pid we
/// cannot query is treated as **alive** — the cautious direction, because the
/// cost of a false "alive" is one leftover socket and the cost of a false
/// "dead" is killing a suite that is running.
fn process_is_alive(pid: u32) -> bool {
    // `/proc` first where it exists: no process spawn per socket examined.
    if std::path::Path::new(&format!("/proc/{pid}")).exists() {
        return true;
    }
    // Elsewhere (macOS), ask ps. `ps -p <pid>` exits 0 only for a live pid.
    Command::new("ps")
        .args(["-p", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        // A machine without `ps` cannot answer: assume alive, do not reap.
        .unwrap_or(true)
}

/// Whether the pid that owns this socket file is still running.
pub fn reap_stale_sockets() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // tmux puts its sockets under `$TMUX_TMPDIR` (or the temp dir) in a
        // `tmux-<uid>` directory. Asking tmux for the parent avoids guessing
        // the uid, and a tmux that cannot answer simply means nothing to reap.
        let Some(dir) = socket_dir_for_tests() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(pid) = name
                .strip_prefix("lca-tui-test-")
                .and_then(|rest| rest.parse::<u32>().ok())
            else {
                continue;
            };
            // Alive? Then it is a concurrent suite's server: leave it.
            //
            // `/proc` is Linux-only. On macOS the same check reads as "every
            // socket is stale", so the reaper killed live servers - including
            // this binary's own, mid-test ("error connecting to
            // .../lca-tui-test-14375", CI run 39538010433). The portable probe
            // is the platform's own process query.
            if process_is_alive(pid) {
                continue;
            }
            let _ = Command::new("tmux")
                .args(["-L", &name, "kill-server"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = std::fs::remove_file(entry.path());
        }
    });
}
