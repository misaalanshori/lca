//! Restoring the default `SIGPIPE` disposition (GitHub issue #19).
//!
//! Rust ignores `SIGPIPE` before `main` runs and gives a program no way to
//! undo it, so a write to a pipe whose reader has already gone away - `|
//! head`, `less` quitting, a pager closed - arrives as `EPIPE` and
//! `println!` panics with `failed printing to stdout: Broken pipe`. Every
//! other Unix CLI is simply *terminated* by the signal, which is what a
//! shell expects: the lines written before the reader left are delivered,
//! the process stops, and stderr stays empty.
//!
//! This is the root fix: one disposition change covers every `println!` in
//! the binary rather than routing each caller through a fallible writer.
//! Windows is left alone deliberately - a write to a closed pipe there
//! returns a recoverable error instead of signalling, so there is no
//! disposition to restore and nothing to invent a mechanism for.

/// Put `SIGPIPE` back to `SIG_DFL`, so the next write to a pipe with no
/// reader ends the process by signal instead of panicking.
///
/// Called once, first thing in `main`, before anything is written. Repeated
/// calls are harmless: setting the same handler again changes nothing.
#[cfg(unix)]
#[allow(unsafe_code)] // documented crate exemption: one `signal(2)` call
pub fn restore_default() {
    // SAFETY: `signal(2)` takes an integer signal number and a handler by
    // value - it borrows no Rust memory, reads no pointer, and every value
    // here is a libc constant. Replacing Rust's installed handler (ignore)
    // with `SIG_DFL` is the whole point of the call, and POSIX defines
    // `SIG_DFL` for `SIGPIPE` as terminate-on-write.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// No `SIGPIPE` exists on Windows: a write to a closed pipe fails with a
/// recoverable error rather than signalling, so there is nothing to
/// restore. Kept as a call site so `main` needs no `cfg` of its own.
#[cfg(not(unix))]
pub fn restore_default() {}
