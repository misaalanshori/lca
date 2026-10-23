//! The host's clipboard write (R6) - routed through the in-process backend
//! (gh #406).
//!
//! Split from `hooks.rs` at gate 11's 1,200-line ceiling; behaviour unchanged.
//! The module exists because the clipboard deserves its own file for a second
//! reason: it is the one piece of `hooks.rs` that talks to the desktop rather
//! than to the agent, and the next person changing it should not have to read
//! five hundred lines of session-tree plumbing to find it.

/// How long a clipboard *write* may take before the ladder gives up on it and
/// lets OSC 52 take over (gh #406).
///
/// `arboard` can block on X11: the selection owner is arbitrary code in another
/// process, and a copy is not worth a frozen interface. A write that has not
/// finished in this window is treated as a failure - which the ladder already
/// handles correctly - rather than waited on indefinitely.
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// Run `work` on a helper thread and give up after [`CLIPBOARD_TIMEOUT`].
///
/// A timed-out call leaves its thread behind: the clipboard API has no
/// cancellation, and the alternative (blocking the interface until an arbitrary
/// other process answers) is the failure this exists to prevent. The thread is
/// detached and exits when the call finally returns; one per stuck copy is a
/// leak this repo accepts, and it is the reason the bound is 500 ms rather than
/// generous.
fn bounded<T, F>(work: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // The receiver may already be gone (the caller gave up); sending then
        // fails, which is not an error worth reporting.
        let _ = tx.send(work());
    });
    rx.recv_timeout(WRITE_TIMEOUT).ok()
}

/// Write text to the system clipboard through the in-process backend, returning
/// `true` only when the write succeeded (R6). A `false` lets the interface fall
/// back to OSC 52 and report it unverified.
///
/// Gh #406: this used to spawn `pbcopy`/`clip`/`wl-copy`/`xclip`/`xsel` per copy,
/// while `arboard` - already a direct dependency, already doing the image read
/// in `attach.rs` - sat linked and unused. The helper ladder is now gone:
///
/// - **It forked a process per copy.** Copying a selection is not worth a
///   `fork`+`exec`+`waitpid`, and on a loaded machine that is measurable.
/// - **It needed one of five programs to exist.** `xclip` and `xsel` are not
///   installed by default on most systems, `wl-copy` needs a Wayland session
///   even when `DISPLAY` also works, and containers usually have none. Every one
///   of those cases silently degraded to OSC 52 - which a terminal may block -
///   while a working backend was in the binary the whole time.
/// - **`arboard` handles the platform's real problems**, which a pipe to
///   `xclip` does not: Win32 clipboard contention (it retries), the macOS
///   pasteboard's multiple types, and X11's `INCR` protocol for large transfers.
///
/// A `false` return here is the headless case (SSH, a container, no display):
/// the ladder treats it exactly as it treated a missing helper, and OSC 52 takes
/// over.
pub(super) fn native_clipboard(text: &str) -> bool {
    let text = text.to_string();
    bounded(move || {
        // A fresh handle per write: `arboard`'s is not `Sync`, and a long-lived
        // one would pin the X11 selection for the whole session, which turns a
        // later copy in another program into a fight.
        let Ok(mut clipboard) = arboard::Clipboard::new() else {
            return false;
        };
        clipboard.set_text(text).is_ok()
    })
    .unwrap_or(false)
}
