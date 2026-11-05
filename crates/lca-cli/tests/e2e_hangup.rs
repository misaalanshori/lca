//! Gh #403 gap 4: a terminal that goes away must end the process, not hang it.
//!
//! # What this drives
//!
//! A **real** `lca` on a **real** pty, both ways a terminal disappears:
//!
//! - the master side is closed under it (an SSH client killed, a terminal
//!   emulator dying, a tmux server exiting) - `poll` reports `POLLHUP` and `read`
//!   returns end-of-file;
//! - `SIGHUP` is delivered to the process - the kernel's own notification for the
//!   same event.
//!
//! Both must reach the documented exit code (`128 + SIGHUP` = 129) rather than
//! hanging, being killed by the signal, or exiting 0 as if nothing happened.
//!
//! # Why a real pty and not a fake
//!
//! `docs/testing-plan.md`'s rule: a unit test that passes while the terminal
//! misbehaves proves nothing. The defect *was* the interaction with a closed pty -
//! `read` returning 0 was read as "nothing yet", so the reader spun at ~92% of a
//! core forever. A fake cannot produce that: it has to be a real fd that really
//! hangs up.
//!
//! # What is row-level only
//!
//! The Windows half of gap 4 (`Ctrl+Z` says suspend is unsupported) **cannot run
//! here** - this box is Linux, and `cfg(windows)` code does not compile into the
//! test binary that would run. It is pinned as a pure predicate instead
//! (`lca_ui::run_terminal`'s `suspend_refusal`, whose `supported` flag is a
//! parameter for exactly this reason) and the cross-target clippy gate proves it
//! compiles for Windows. The report says so rather than implying the row ran.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

use std::io::Read;
use std::os::unix::io::FromRawFd;
use std::time::{Duration, Instant};

mod common;

use common::{find_session_log, sandbox};

/// How long the app gets to paint its first frame before the test acts, and how
/// long it must stay quiet before the frame is considered complete.
const STARTUP: Duration = Duration::from_secs(20);
const QUIET: Duration = Duration::from_millis(400);

/// How long the app gets to exit after the terminal goes away. Generous: the
/// failure mode this row exists for never exits at all, so a slow exit is still a
/// pass and the ceiling only bounds the failure case.
const EXIT_WITHIN: Duration = Duration::from_secs(10);

/// How a child ended (gh #403 gap 4).
///
/// The distinction is load-bearing and was the first version's bug: a process killed
/// by `SIGHUP` also reports `128 + 1 = 129` through the shell's convention, so a row
/// that only compared numbers could not tell "the handler ran and exited 129" from
/// "the signal killed it and the shell would have said 129". Collapsing the two is
/// exactly how a fix that never installed a handler passes its own test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The process exited with this code - an orderly shutdown.
    Exited(i32),
    /// The process died of this signal - not a shutdown at all.
    Signalled(i32),
}

/// A `lca` running on its own pseudo-terminal.
struct Pane {
    pid: libc::pid_t,
    master: std::fs::File,
}

impl Pane {
    /// Spawn the binary on a fresh pty, with the sandbox's `HOME` and a
    /// deliberately unreachable provider endpoint.
    ///
    /// The endpoint matters: a turn that cannot connect still records the user's
    /// message before the call, which is what makes "the session's last record
    /// survives a dead terminal" assertable without a mock server in a forked
    /// child.
    fn spawn(sandbox: &common::Sandbox) -> Pane {
        Self::spawn_with(sandbox, false)
    }

    /// The same, optionally with **`SIGHUP` blocked** in the child.
    ///
    /// A blocked signal is inherited across `exec`, and a handler cannot run while
    /// it is blocked - so the app's own `install()` becomes unreachable and the pty
    /// closing is the *only* notice it can get. That is what isolates the reader's
    /// detection from the signal's: without it, closing the master also raises
    /// `SIGHUP` (the child is the pty's session leader), and the two routes become
    /// indistinguishable - the first version of this file could not tell them apart.
    fn spawn_with(sandbox: &common::Sandbox, block_hangup: bool) -> Pane {
        // SAFETY: the pty family below is used exactly as POSIX documents it; each
        // call's result is checked and the error paths close what they opened.
        unsafe {
            let master_fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(master_fd >= 0, "posix_openpt failed");
            assert_eq!(libc::grantpt(master_fd), 0, "grantpt failed");
            assert_eq!(libc::unlockpt(master_fd), 0, "unlockpt failed");
            let name = libc::ptsname(master_fd);
            assert!(!name.is_null(), "ptsname failed");

            let pid = libc::fork();
            assert!(pid >= 0, "fork failed");
            if pid == 0 {
                // Child: become a session leader, take the slave as the
                // controlling terminal, then exec.
                libc::close(master_fd);
                libc::setsid();
                if block_hangup {
                    let mut set: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&mut set);
                    libc::sigaddset(&mut set, libc::SIGHUP);
                    libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                }
                let slave = libc::open(name, libc::O_RDWR);
                if slave < 0 {
                    libc::_exit(127);
                }
                libc::dup2(slave, 0);
                libc::dup2(slave, 1);
                libc::dup2(slave, 2);
                if slave > 2 {
                    libc::close(slave);
                }
                let bin = std::ffi::CString::new(env!("CARGO_BIN_EXE_lca")).unwrap();
                let argv = [bin.as_ptr(), std::ptr::null()];
                // The environment is inherited from the test process, which the
                // sandbox's `Command`-based path normally sets. Set the three that
                // decide where state lives directly.
                let home =
                    std::ffi::CString::new(format!("HOME={}", sandbox.home.display())).unwrap();
                let data = std::ffi::CString::new(format!(
                    "LCA_DATA_DIR={}",
                    sandbox.state_dir().display()
                ))
                .unwrap();
                let update = std::ffi::CString::new("LCA_UPDATE_CHECK=false").unwrap();
                let endpoint =
                    std::ffi::CString::new("OPENAI_BASE_URL=http://127.0.0.1:1/v1").unwrap();
                let key = std::ffi::CString::new("OPENAI_API_KEY=test-key").unwrap();
                let model = std::ffi::CString::new("OPENAI_MODEL=test-model").unwrap();
                let term = std::ffi::CString::new("TERM=xterm-256color").unwrap();
                libc::putenv(home.into_raw());
                libc::putenv(data.into_raw());
                libc::putenv(update.into_raw());
                libc::putenv(endpoint.into_raw());
                libc::putenv(key.into_raw());
                libc::putenv(model.into_raw());
                libc::putenv(term.into_raw());
                libc::execv(argv[0], argv.as_ptr());
                libc::_exit(127);
            }

            // Nonblocking, so `read_for` can poll for "anything yet" instead of
            // parking on an idle terminal - which is what a blocking read does for
            // as long as the app is quiet, and is how the first version of this
            // helper timed the whole suite out.
            let flags = libc::fcntl(master_fd, libc::F_GETFL);
            assert!(flags >= 0, "fcntl F_GETFL failed");
            assert_eq!(
                libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK),
                0,
                "fcntl F_SETFL failed"
            );
            let master = std::fs::File::from_raw_fd(master_fd);
            Pane { pid, master }
        }
    }

    /// Read until the app has been quiet for `quiet`, or `window` runs out.
    ///
    /// Quiet-based rather than "read once": the app paints a frame over several
    /// writes, so returning at the first one would sample a half-drawn screen. And
    /// bounded by `window` rather than looping until it, so a healthy pane costs one
    /// quiet period instead of the whole timeout.
    fn read_for(&mut self, quiet: Duration) -> String {
        let start = Instant::now();
        let mut seen = Vec::new();
        let mut buf = [0u8; 8192];
        let mut last_data: Option<Instant> = None;
        loop {
            match self.master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    seen.extend_from_slice(&buf[..n]);
                    last_data = Some(Instant::now());
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
            match last_data {
                // Phase 2: something painted; stop once it has been quiet.
                Some(at) if at.elapsed() >= quiet => break,
                // Phase 1: nothing yet; wait out the startup window.
                _ if start.elapsed() >= STARTUP => break,
                _ => std::thread::sleep(Duration::from_millis(25)),
            }
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    /// Type at the app.
    fn type_text(&mut self, text: &str) {
        use std::io::Write as _;
        self.master.write_all(text.as_bytes()).unwrap();
        self.master.flush().unwrap();
    }

    /// Wait for the process to exit, returning `Some(code)` when it did.
    ///
    /// `None` is the failure this whole file exists for: the process is still
    /// there when the window closes, which is a hang, and the caller kills it so a
    /// failing row does not leak a spinning process into the rest of the suite.
    fn wait(&self, window: Duration) -> Option<Outcome> {
        let deadline = Instant::now() + window;
        loop {
            let mut status = 0;
            // SAFETY: WNOHANG makes this a poll; `status` is a valid out-param.
            let reaped = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if reaped == self.pid {
                if libc::WIFSIGNALED(status) {
                    return Some(Outcome::Signalled(libc::WTERMSIG(status)));
                }
                return Some(Outcome::Exited(libc::WEXITSTATUS(status)));
            }
            if Instant::now() >= deadline {
                // SAFETY: killing our own child.
                unsafe {
                    libc::kill(self.pid, libc::SIGKILL);
                    libc::waitpid(self.pid, &mut status, 0);
                }
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let mut status = 0;
        // SAFETY: our own child; ECHILD after a successful wait is harmless.
        unsafe {
            libc::kill(self.pid, libc::SIGKILL);
            libc::waitpid(self.pid, &mut status, 0);
        }
    }
}

// Verifies: gh #403 gap 4 - **the hang.** Closing the pty's master side under a
// running interface ends the process with the documented code.
//
// On the pre-fix tree this row times out: `wait_stdin` only asked about `POLLIN`,
// `read` returned 0, and the reader loop read that as "nothing yet" and spun - at
// ~92% of a core, indefinitely, on a terminal nobody was watching.
#[test]
fn a_closed_pty_ends_the_process_with_the_documented_code() {
    let sandbox = sandbox("hangup-closed-pty");
    let mut pane = Pane::spawn(&sandbox);
    let painted = pane.read_for(QUIET);
    assert!(
        painted.contains("lca") || !painted.is_empty(),
        "the interface painted a first frame before the terminal went away: {painted:?}"
    );

    // The terminal goes away. `poll` on the app's stdin now reports POLLHUP.
    drop(std::mem::replace(
        &mut pane.master,
        std::fs::File::open("/dev/null").unwrap(),
    ));

    assert_eq!(
        pane.wait(EXIT_WITHIN),
        Some(Outcome::Exited(129)),
        "a dead terminal **exits** 128 + SIGHUP - `Exited`, not `Signalled(1)`: the \
         point is that it ran a shutdown, not that the shell would have reported the \
         same number for a signal death"
    );
}

// Verifies: gh #403 gap 4 - **the reader's own detection, isolated.** With `SIGHUP`
// blocked in the child, the pty closing is the only notice the process can get, so
// this row can only pass if `poll`'s hangup classification (or the end-of-file read)
// ends the loop.
//
// This is the receipt for the part the plain closed-pty row *cannot* prove: closing a
// pty's master also raises `SIGHUP` at its session leader, so on the pre-fix tree that
// row still exits - by signal - and a row comparing only numbers would call it a pass.
// Here the pre-fix tree spins at ~92% of a core until the wait expires.
#[test]
fn the_readers_own_detection_ends_the_loop_without_a_signal() {
    let sandbox = sandbox("hangup-reader-only");
    let mut pane = Pane::spawn_with(&sandbox, true);
    let painted = pane.read_for(QUIET);
    assert!(
        !painted.is_empty(),
        "the interface painted before the terminal went away"
    );

    drop(std::mem::replace(
        &mut pane.master,
        std::fs::File::open("/dev/null").unwrap(),
    ));
    assert_eq!(
        pane.wait(EXIT_WITHIN),
        Some(Outcome::Exited(129)),
        "a hangup the reader must notice on its own still exits 129; the pre-fix \
         tree spins here until this window closes"
    );
}

// Verifies: gh #403 gap 4 - **the same event from the other direction.** A
// `SIGHUP` reaches the same orderly path instead of killing the process before its
// last records land.
//
// `128 + 1` is what this row asserts, and it is the same number the pty route earns:
// the two directions are one event, so an exit code that forked by route would be a
// script's problem to discover.
#[test]
fn a_sighup_takes_the_same_path() {
    let sandbox = sandbox("hangup-sighup");
    let mut pane = Pane::spawn(&sandbox);
    let _ = pane.read_for(QUIET);

    // SAFETY: signalling our own child.
    unsafe {
        libc::kill(pane.pid, libc::SIGHUP);
    }
    assert_eq!(
        pane.wait(EXIT_WITHIN),
        Some(Outcome::Exited(129)),
        "SIGHUP is recorded and handled, not left at its terminate disposition - the \
         pre-fix tree dies of the signal, which this row tells apart by variant"
    );
}

// Verifies: gh #403 gap 4 - **a dead terminal must not lose the session.** The
// record written before the terminal went away is on disk after the exit.
//
// The append-only log makes this mostly free, but "mostly" is exactly the word the
// brief refuses, so the claim gets a receipt: `SessionStore::append` writes the whole
// line and flushes before it returns, so a record that reached the store is durable
// without the process being given a chance to run any shutdown at all. The row drives
// `/name` rather than a turn because a *turn* against an unreachable endpoint keeps
// the process busy in the cancel/join path - a real behaviour, but a different race
// (the brief's fourth deferred item), and mixing it in would make this row measure
// two things and fail for the wrong one.
#[test]
fn the_last_record_survives_a_dead_terminal() {
    let sandbox = sandbox("hangup-session-durable");
    let mut pane = Pane::spawn(&sandbox);
    let _ = pane.read_for(QUIET);

    // A command that appends one record and nothing else.
    pane.type_text("/name doomed-session\r");
    std::thread::sleep(Duration::from_millis(800));

    drop(std::mem::replace(
        &mut pane.master,
        std::fs::File::open("/dev/null").unwrap(),
    ));
    assert_eq!(
        pane.wait(EXIT_WITHIN),
        Some(Outcome::Exited(129)),
        "it still exits 129"
    );

    let log = find_session_log(&sandbox.state_dir()).expect("the session was written");
    let text = std::fs::read_to_string(&log).expect("read the log");
    assert!(
        text.contains("doomed-session"),
        "the record outlived the terminal: {text}"
    );
    // And the log is whole to its last line: a truncated final line would mean the
    // death interrupted a write rather than followed one.
    assert!(
        text.ends_with('\n'),
        "every record is a whole line, so nothing was cut mid-write: {text:?}"
    );
    // The strongest receipt available: the clean-exit marker is present, so the
    // orderly shutdown ran to completion. A killed process would leave the log
    // without one (`docs/session-log-format.md`: its absence means the session ended
    // without one, normal after a crash).
    assert!(
        text.contains("\"session-end\""),
        "the shutdown completed rather than being cut off: {text}"
    );
}

// Verifies: gh #403 gap 4, the control - **a terminal that stays put is not
// affected.** A run whose pty is never closed keeps running until it is asked to
// leave, so the fix did not turn "the terminal is quiet" into "the terminal is
// gone".
//
// Without this row a fix that treated *any* timeout as a hangup would pass every
// other row in this file and exit every idle session.
#[test]
fn a_live_terminal_is_not_mistaken_for_a_dead_one() {
    let sandbox = sandbox("hangup-live-control");
    let mut pane = Pane::spawn(&sandbox);
    let _ = pane.read_for(QUIET);

    // A perfectly healthy, perfectly silent terminal, for far longer than the
    // reader's 50 ms wait - the interval a hangup would land on. `sleep`, not
    // `read_for`: "nothing arrives" is the whole point, so waiting for quiet would
    // just wait out the startup window.
    std::thread::sleep(Duration::from_millis(1200));
    let mut status = 0;
    // SAFETY: our own child; WNOHANG polls.
    let alive = unsafe { libc::waitpid(pane.pid, &mut status, libc::WNOHANG) } == 0;
    assert!(alive, "an idle interface stays alive");

    // `/exit` leaves cleanly, which also proves the loop was still doing its job.
    pane.type_text("/exit\r");
    assert_eq!(
        pane.wait(EXIT_WITHIN),
        Some(Outcome::Exited(0)),
        "and it still exits 0 when asked to"
    );
}
