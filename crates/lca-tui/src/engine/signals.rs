//! The two signals the interface owes, and the exit they earn (gh #403 gap 4).
//!
//! # A dead terminal, two directions
//!
//! A terminal disappears in one of two ways, and a program must notice both:
//!
//! - **The pty hangs up.** The master side closes (an SSH client is killed, a
//!   terminal emulator dies, a tmux server exits), so `poll` reports `POLLHUP`
//!   and `read` returns end-of-file. Nothing is *sent*; the fd stops being a
//!   terminal. This is the direction that used to hang: `wait_stdin` asked only
//!   about `POLLIN`, `read` answered `Ok(0)`, and the reader loop read "no
//!   bytes" as "nothing yet" - so it spun at ~92% of a core forever, painting
//!   nothing, on a terminal nobody was watching.
//! - **`SIGHUP` arrives.** The kernel's own notification for the same event.
//!   Left at its default disposition it terminates the process, which exits but
//!   skips the orderly shutdown: no `session-end`, no resume hint, and the exit
//!   status is a signal death rather than a number a script can read.
//!
//! Both end in one decision: mark the terminal dead, write nothing more to it,
//! flush the session, exit `EXIT_CODE`. So the reader loop's hangup and this
//! module's flag are the *same* fact, and `requested()` plus
//! [`crate::engine::terminal`]'s per-terminal flag are the two readers.
//!
//! # Why the handler is installed by the binary, not here
//!
//! A signal disposition is process-wide and permanent, so installing it from a
//! library constructor would change every embedder's behaviour - including
//! headless runs, which have no UI loop to poll a flag and would then *ignore*
//! `SIGHUP` instead of dying. The interactive host installs it
//! (`lca_cli::tui::run`, in the binary), because that is the process with somewhere to
//! notice it.
//!
//! # Why a flag and not a self-pipe
//!
//! A handler may call only async-signal-safe functions; storing to an atomic is
//! one, and it is all this needs. The loop already wakes at least every 50 ms
//! (the tick is the floor for sources with no producer to signal), so the worst
//! case is one tick of latency rather than a hang.
//! `ponytail:` a tick of latency, no self-pipe; upgrade only if a deadline ever
//! needs sub-tick precision here.

use std::sync::atomic::{AtomicBool, Ordering};

/// The exit code a dead terminal earns: `128 + SIGHUP` (gh #403 gap 4).
///
/// The number is not invented - `128 + n` is the shell's convention for "the
/// child died of signal `n`", and `SIGHUP` is 1. Both directions use it because
/// both directions are the same event, and the code is the same one an
/// interactive shell would have reported had the signal killed the process. It
/// is documented as row 129 of `docs/headless.md`'s table.
pub const EXIT_CODE: i32 = 129;

/// Whether a `SIGHUP` has been seen (gh #403 gap 4).
///
/// Process-wide because a signal is: the handler has no context to write
/// through, so this cell is the only channel it can use.
static HANGUP: AtomicBool = AtomicBool::new(false);

/// Whether the terminal is known to be gone, by either direction.
#[must_use]
pub fn requested() -> bool {
    HANGUP.load(Ordering::SeqCst)
}

/// Record that the terminal is gone.
///
/// Called by the `SIGHUP` handler; exposed so the reader's own detection can
/// set the same cell and so a test can drive either route.
pub fn request() {
    HANGUP.store(true, Ordering::SeqCst);
}

/// Clear the flag. Tests only: a real process that saw `SIGHUP` is exiting.
///
/// Public but `#[doc(hidden)]`-free on purpose: an *embedder's* test harness owns
/// a process too, and a suite that drives the hangup path needs the same reset this
/// crate's rows use. It is absent from a release build's behaviour (nothing calls
/// it), so it is a reset switch, not a feature.
pub fn forget() {
    HANGUP.store(false, Ordering::SeqCst);
}

/// Install the `SIGHUP` handler, which records the hangup instead of killing
/// the process.
///
/// Idempotent, so an entry point can call it without tracking whether it
/// already did. Called by the interactive host before it claims the terminal,
/// so a hangup during the startup window is recorded too.
///
/// **Deliberately not installed for Windows' console-close events.** A Windows
/// console reports its loss through a failed `ReadFile` - the `EIO`/`ENOTTY`
/// equivalent, classified by the reader - so there is no signal to catch, and a
/// console control handler here would be a second mechanism for an event that
/// already arrives.
#[cfg(unix)]
#[allow(unsafe_code)] // documented exemption: one `signal(2)` call, as in `sigpipe`
pub fn install() {
    /// `signal(2)`'s handler shape: store a flag and nothing else, which is the
    /// only work a handler is allowed to do.
    extern "C" fn on_hangup(_signal: libc::c_int) {
        request();
    }
    // The signature has to be the real one: `signal` is typed against
    // `sighandler_t`, so the coercion goes through a function pointer rather than a
    // direct item-to-integer cast (which `clippy::fn_to_numeric_cast` refuses, for
    // the good reason that it usually means someone is boxing a function).
    let handler: extern "C" fn(libc::c_int) = on_hangup;
    // SAFETY: `signal(2)` takes an integer signal number and a handler by value -
    // it borrows no Rust memory and reads no pointer, and `SIGHUP` is a libc
    // constant. The handler `on_hangup` touches nothing but a `static AtomicBool`,
    // which is async-signal-safe, so the signal may fire mid-anything.
    unsafe {
        libc::signal(libc::SIGHUP, handler as *const () as libc::sighandler_t);
    }
}

/// No `SIGHUP` exists on Windows: a closed console surfaces as a failed read,
/// which the reader classifies without a signal. Kept as a call site so the host
/// needs no `cfg` of its own.
#[cfg(not(unix))]
pub fn install() {}

/// Whether this platform can suspend to the background (gh #403 gap 4).
///
/// `SIGTSTP` is a unix job-control signal with no Windows equivalent, and the
/// question is asked *before* anything is torn down: leaving the alternate
/// screen and stopping the reader to attempt a suspend that cannot happen would
/// leave the frame torn for no reason.
#[must_use]
pub const fn suspend_supported() -> bool {
    cfg!(unix)
}

/// What a suspend request did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suspended {
    /// The process stopped and has now been continued.
    Resumed,
    /// This platform has no job control; nothing was attempted.
    Unsupported,
}

/// Stop this process (and its process group) until something continues it.
///
/// Sends `SIGTSTP` to the **process group** (pi's `process.kill(0, "SIGTSTP")`),
/// so a shell child in the same group stops too rather than being orphaned in
/// the foreground. The call returns after `SIGCONT`, which is what lets the
/// caller restore the terminal on the same line.
///
/// The caller must have restored the terminal first: the whole point of a
/// suspend is to hand the tty back to the shell.
///
/// Deferred by the brief, deliberately: pi keeps a Node timer alive across the
/// stop and shields `SIGINT` while suspended. Neither is a Rust problem - the
/// threads exist independently of any event loop - so this is the plain
/// suspend, and the report names the two guards as still owed if a live
/// `Ctrl+C` in the background turns out to be able to kill a suspended LCA.
#[cfg(unix)]
#[allow(unsafe_code)] // documented exemption: one `kill(2)` call, as in `sigpipe`
pub fn suspend() -> Suspended {
    // SAFETY: `kill(2)` takes a pid and a signal number; `0` means "every
    // process in my group", which is the shell's own convention for a suspend
    // and needs no pid bookkeeping. No memory is borrowed and nothing is read.
    unsafe {
        libc::kill(0, libc::SIGTSTP);
    }
    Suspended::Resumed
}

/// No job control on Windows (see [`suspend_supported`]).
#[cfg(not(unix))]
pub fn suspend() -> Suspended {
    Suspended::Unsupported
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #403 gap 4 - the flag round-trips, and it is the **one** cell
    // both directions write (the handler has no other way to speak).
    //
    // Self-contained: it clears before and after, because the cell is
    // process-wide and the suite runs tests as threads in one process.
    #[test]
    fn the_hangup_flag_round_trips() {
        forget();
        assert!(!requested(), "a fresh process has not seen a hangup");
        request();
        assert!(requested(), "the signal's own path records it");
        forget();
        assert!(!requested(), "clearing is what keeps this row repeatable");
    }

    // Verifies: gh #403 gap 4 - installing is idempotent and does **not** by
    // itself claim a hangup, so the entry point can call it unconditionally.
    #[test]
    fn installing_does_not_claim_a_hangup() {
        forget();
        install();
        install();
        assert!(!requested(), "installation is not an event");
    }

    // Verifies: gh #403 gap 4 - the exit code is the shell's `128 + SIGHUP`
    // convention, which is what makes it readable by a script that never heard
    // of LCA. A code invented here ("7", "42") would be indistinguishable from
    // a usage error.
    #[test]
    fn the_exit_code_is_128_plus_sighup() {
        assert_eq!(EXIT_CODE, 129);
    }

    // Verifies: gh #403 gap 4 - the platform predicates agree with each other.
    //
    // A `suspend_supported` that said `true` where `suspend` answers
    // `Unsupported` would tear down the alternate screen for nothing, which is
    // worse than the silence this cycle removes.
    #[test]
    fn the_suspend_predicate_matches_what_suspend_can_do() {
        // Asserted as a **relationship**, not as a constant: `suspend()` is what
        // actually hands the tty back, so the predicate has to agree with it on
        // every platform this compiles for.
        let expected = if cfg!(unix) {
            Suspended::Resumed
        } else {
            Suspended::Unsupported
        };
        if !suspend_supported() {
            assert_eq!(
                suspend(),
                Suspended::Unsupported,
                "an unsupported platform must refuse rather than pretend"
            );
        }
        assert_eq!(
            expected == Suspended::Unsupported,
            !suspend_supported(),
            "the predicate and the suspension must agree"
        );
    }
}
