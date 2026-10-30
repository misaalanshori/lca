//! Operating-system terminal backend, ported in shape from pi's
//! `terminal.ts` (`pi-tui-re/src_re/tui-engine/terminal.md` §2/§5).
//!
//! This is `lca-tui`'s single documented `unsafe` exemption from the crate's
//! `#![deny(unsafe_code)]`, exactly like `lca-tools/src/pty.rs`: termios raw
//! mode, `TIOCGWINSZ`, `read(2)` and `isatty` on Unix; console mode,
//! `ReadFile` and screen-buffer info on Windows. Every `unsafe` block has a
//! `SAFETY` note.
//!
//! Difference from pi: resize is detected by polling the window size inside
//! the stdin reader loop (every ~50 ms) rather than by handling SIGWINCH.
//! `ponytail:` a poll instead of a signal costs one ioctl per 50 ms; upgrade
//! to a SIGWINCH self-pipe if idle CPU ever matters.
#![allow(unsafe_code)] // documented crate exemption: OS terminal backend

use std::io;

/// Whether the most recent Enter carried the Shift modifier (gh #367).
///
/// On a console whose virtual-terminal input drops the modifier, `Enter` and
/// `Shift+Enter` both arrive as `\r` - but the **console record** still knows which
/// physical key it was, so this is answerable on Windows rather than merely guessed.
/// `false` everywhere else: a Unix terminal that strips Shift gives the reader no
/// such second source, which is why those are handled by a policy instead.
pub(crate) fn shift_was_held_on_enter() -> bool {
    #[cfg(windows)]
    {
        windows_backend::shift_was_held_on_enter()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Saved terminal state, restored on stop.
pub(crate) struct RawModeState {
    #[cfg(unix)]
    original: libc::termios,
    #[cfg(windows)]
    original: windows_backend::Saved,
}

/// Put stdin in raw mode, returning the state to restore.
pub(crate) fn enable_raw_mode() -> io::Result<RawModeState> {
    #[cfg(unix)]
    {
        // SAFETY: `termios` is a plain-old-data struct; zeroing is a valid
        // initial value for tcgetattr to overwrite.
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: fd 0 is the process stdin; termios is a valid out-param.
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut termios) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = termios;
        // Node's setRawMode semantics (terminal.ts step 1).
        termios.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        termios.c_oflag &= !libc::OPOST;
        termios.c_cflag |= libc::CS8;
        termios.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        termios.c_cc[libc::VMIN] = 1;
        termios.c_cc[libc::VTIME] = 0;
        // SAFETY: termios is fully initialized; fd 0 is our stdin.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &termios) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(RawModeState { original })
    }
    #[cfg(windows)]
    {
        windows_backend::enable_raw_mode()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(RawModeState {})
    }
}

/// Restore the saved terminal state.
pub(crate) fn restore_raw_mode(state: RawModeState) {
    #[cfg(unix)]
    {
        // SAFETY: `state.original` came from tcgetattr on the same fd.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &state.original);
        }
    }
    #[cfg(windows)]
    {
        windows_backend::restore_raw_mode(&state);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = state;
    }
}

/// Whether stdin is a terminal (raw mode must be skipped otherwise).
pub(crate) fn stdin_is_tty() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: isatty only inspects the fd.
        unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
    }
    #[cfg(windows)]
    {
        windows_backend::stdin_is_tty()
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// Read available bytes from stdin (blocking).
pub(crate) fn read_stdin(buf: &mut [u8]) -> io::Result<usize> {
    #[cfg(unix)]
    {
        // SAFETY: `buf` is a valid writable slice of `buf.len()` bytes.
        let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let err = io::Error::last_os_error();
            // EINTR is not a failure.
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(0);
            }
            return Err(err);
        }
        Ok(n as usize)
    }
    #[cfg(windows)]
    {
        windows_backend::read_stdin(buf)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = buf;
        Ok(0)
    }
}

/// The error a refused `SetConsoleMode` produces (gh #323 defect 3).
///
/// Split out from the Windows backend for the same reason as
/// [`console_record_meaning`]: the *message* is the part a caller reads and the part
/// that regresses, and it can be asserted on any platform. The `SetConsoleMode` call
/// itself needs a real console.
///
/// **It names the handle and carries the OS code, and it is an `io::Error`, not a
/// panic.** A panic in a production path is the thing the workspace lints forbid;
/// turning a silent failure into a crash would be trading one failure for a worse
/// one. The code is what makes the report actionable - `6`
/// (`ERROR_INVALID_HANDLE`) means the handle was never a console, `5`
/// (`ERROR_ACCESS_DENIED`) means something else owns it, and only the first is the
/// caller's fault.
#[cfg_attr(not(windows), allow(dead_code))] // called by the Windows backend, and by its rows
pub(crate) fn set_mode_refusal(which: &str, code: i32) -> std::io::Error {
    std::io::Error::other(format!(
        "SetConsoleMode({which}) refused raw mode (os error {code})"
    ))
}

/// What one console record type means to the event loop (gh #323 defect 4).
///
/// Split out from the Windows backend so the decision is **testable on every
/// platform**: the `INPUT_RECORD` plumbing needs a real console, but the question
/// "does this record make the loop repaint?" is a comparison against a constant,
/// and a Linux row can hold that. Otherwise the whole fix would be untestable on
/// the only machine most of these cycles run on, which is how a regression here
/// would go unnoticed.
///
/// Only `WINDOW_BUFFER_SIZE_EVENT` counts as a resize. Mouse and focus records are
/// genuinely ignorable - the engine reads no pointer state from the console - so
/// treating them as resizes would repaint on every mouse move.
#[cfg_attr(not(windows), allow(dead_code))] // reached only by the Windows backend, and by its rows
pub(crate) fn console_record_meaning(event_type: u16) -> bool {
    /// `wincon.h`'s `WINDOW_BUFFER_SIZE_EVENT`.
    const WINDOW_BUFFER_SIZE_EVENT_TYPE: u16 = 0x0004;
    event_type == WINDOW_BUFFER_SIZE_EVENT_TYPE
}

/// What a [`wait_stdin`] call found (gh #323 defect 4).
///
/// A console reports more than keys: focus changes, mouse motion and — the one
/// that matters — **resizes**, as `WINDOW_BUFFER_SIZE_EVENT`. The previous
/// `bool` had no way to say so, so a resize was consumed and dropped, and the
/// event loop learned the new size only when its own timer next fired. Naming the
/// outcomes is what lets that record reach the handler that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitOutcome {
    /// Nothing arrived in the window.
    Timeout,
    /// A `KEY_EVENT` is pending: a `read_stdin` call will return promptly.
    Key,
    /// The console's buffer changed size; the terminal should re-query and
    /// repaint now rather than on its next tick.
    ///
    /// Constructed only by the Windows backend (`WINDOW_BUFFER_SIZE_EVENT` has no
    /// unix equivalent - a resize arrives as `SIGWINCH`), and asserted by the row
    /// that pins "a resize is not a key" on every platform.
    #[cfg_attr(not(windows), allow(dead_code))]
    Resize,
}

impl WaitOutcome {
    /// Whether a key is pending, for callers that only drain input.
    pub(crate) fn is_key(self) -> bool {
        matches!(self, WaitOutcome::Key)
    }
}

/// Wait up to `timeout_ms` for stdin to become readable.
pub(crate) fn wait_stdin(timeout_ms: i32) -> io::Result<WaitOutcome> {
    #[cfg(unix)]
    {
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd for the duration of the call.
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(WaitOutcome::Timeout);
            }
            return Err(err);
        }
        // A unix terminal has no console-record channel: a resize arrives as
        // `SIGWINCH`, which the process catches separately, so poll readiness only
        // ever means "a key is waiting".
        if rc > 0 && (pfd.revents & libc::POLLIN) != 0 {
            Ok(WaitOutcome::Key)
        } else {
            Ok(WaitOutcome::Timeout)
        }
    }
    #[cfg(windows)]
    {
        windows_backend::wait_stdin(timeout_ms)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = timeout_ms;
        Ok(WaitOutcome::Timeout)
    }
}

/// Cancel a thread's pending synchronous I/O, unblocking a `ReadFile`
/// parked on the console input handle so the reader can observe shutdown.
#[cfg(windows)]
pub(super) fn cancel_blocking_read(thread: std::os::windows::io::RawHandle) {
    // SAFETY: the caller passes a live thread handle (the JoinHandle keeps
    // the thread alive); CancelSynchronousIo aborts that thread's pending
    // synchronous I/O, which is exactly the intended unblock.
    unsafe {
        windows_sys::Win32::System::IO::CancelSynchronousIo(thread as *mut _);
    }
}

/// The terminal's current column/row count.
pub(crate) fn terminal_size() -> io::Result<(u16, u16)> {
    #[cfg(unix)]
    {
        // SAFETY: winsize is plain-old-data; ioctl TIOCGWINSZ fills it.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: fd 1 is stdout; a valid out-param.
        if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0
            && ws.ws_col > 0
        {
            return Ok((ws.ws_col, ws.ws_row));
        }
        // Fall back to stdin.
        // SAFETY: same call against fd 0.
        if unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0
            && ws.ws_col > 0
        {
            return Ok((ws.ws_col, ws.ws_row));
        }
        Ok((80, 24))
    }
    #[cfg(windows)]
    {
        windows_backend::terminal_size()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok((80, 24))
    }
}

#[cfg(windows)]
mod windows_backend {
    use std::io;

    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode,
        GetConsoleScreenBufferInfo, GetNumberOfConsoleInputEvents, GetStdHandle, INPUT_RECORD,
        KEY_EVENT, PeekConsoleInputW, ReadConsoleInputW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };

    /// Whether the most recent Enter arrived with Shift held (gh #367), latched by
    /// `wait_stdin` from the `KEY_EVENT` record itself.
    static SHIFT_PRESSED_ON_ENTER: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// `winuser.h`'s `SHIFT_PRESSED`, the `dwControlKeyState` bit for a held Shift.
    const SHIFT_PRESSED: u32 = 0x0010;
    /// `winuser.h`'s `VK_RETURN`.
    const VK_RETURN: u16 = 0x0D;

    /// Whether the last Enter carried Shift (gh #367).
    pub(super) fn shift_was_held_on_enter() -> bool {
        SHIFT_PRESSED_ON_ENTER.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) struct Saved {
        stdin_mode: u32,
        stdout_mode: u32,
        have_stdin: bool,
        have_stdout: bool,
    }

    fn stdin_handle() -> HANDLE {
        // SAFETY: GetStdHandle is infallible for a valid selector.
        unsafe { GetStdHandle(STD_INPUT_HANDLE) }
    }

    fn stdout_handle() -> HANDLE {
        // SAFETY: GetStdHandle is infallible for a valid selector.
        unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }
    }

    fn valid(h: HANDLE) -> bool {
        !h.is_null() && h != INVALID_HANDLE_VALUE
    }

    /// The refusal of a `SetConsoleMode`, naming which handle and carrying the
    /// OS code (gh #323 defect 3).
    ///
    /// The code is part of the message because it is the only actionable part:
    /// `ERROR_INVALID_HANDLE` (6) means the handle was not a console at all,
    /// `ERROR_ACCESS_DENIED` (5) means something else owns it, and the difference
    /// decides whether retrying is worth anything.
    fn set_mode_error(which: &str) -> io::Error {
        let code = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        super::set_mode_refusal(which, code)
    }

    /// Turn off echo / line buffering / Ctrl-C processing, and on VT input.
    ///
    /// **Every `SetConsoleMode` return value is checked** (gh #323 defect 3). The
    /// calls used to be bare statements, so a refusal was invisible: the process
    /// carried on believing it was in raw mode while the console stayed cooked,
    /// and the symptom surfaced much later as garbled input or a stuck prompt.
    ///
    /// **A failure is reported, not panicked** — the return is an `io::Error`
    /// built from `GetLastError`, so the caller decides. Panicking here would
    /// trade one production-path failure for another, and the caller is the one
    /// that knows whether it can run without a raw console (a piped stdin, a
    /// non-tty diagnosis) or must stop.
    ///
    /// What is *not* reverted on a partial failure: a stdin mode already changed
    /// when the stdout call fails. The caller drops the returned state on the
    /// error path, so `restore_raw_mode` never runs — a leak this small is
    /// deliberate, because the alternative is a second `SetConsoleMode` that can
    /// also fail, inside the error path, with no way to report it. The console is
    /// per-process and the process is about to stop or degrade; a mode left
    /// changed under a dying process is not the hazard.
    pub(super) fn enable_raw_mode() -> io::Result<super::RawModeState> {
        let stdin = stdin_handle();
        let stdout = stdout_handle();
        let mut stdin_mode = 0u32;
        let mut stdout_mode = 0u32;
        let have_stdin = valid(stdin) && unsafe { GetConsoleMode(stdin, &mut stdin_mode) } != 0;
        let have_stdout = valid(stdout) && unsafe { GetConsoleMode(stdout, &mut stdout_mode) } != 0;
        if have_stdin {
            // Raw-ish input plus VT input so modified keys arrive as sequences.
            let new_mode = (stdin_mode
                & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
                | ENABLE_VIRTUAL_TERMINAL_INPUT;
            // SAFETY: handle and mode are valid console values.
            if unsafe { SetConsoleMode(stdin, new_mode) } == 0 {
                return Err(set_mode_error("stdin"));
            }
        }
        if have_stdout {
            // SAFETY: enable ANSI processing on the output console.
            if unsafe { SetConsoleMode(stdout, stdout_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) }
                == 0
            {
                return Err(set_mode_error("stdout"));
            }
        }
        Ok(super::RawModeState {
            original: Saved {
                stdin_mode,
                stdout_mode,
                have_stdin,
                have_stdout,
            },
        })
    }

    pub(super) fn restore_raw_mode(state: &super::RawModeState) {
        let saved = &state.original;
        if saved.have_stdin {
            // SAFETY: restoring the mode we read earlier.
            unsafe { SetConsoleMode(stdin_handle(), saved.stdin_mode) };
        }
        if saved.have_stdout {
            // SAFETY: restoring the mode we read earlier.
            unsafe { SetConsoleMode(stdout_handle(), saved.stdout_mode) };
        }
    }

    pub(super) fn stdin_is_tty() -> bool {
        let h = stdin_handle();
        let mut mode = 0u32;
        valid(h) && unsafe { GetConsoleMode(h, &mut mode) } != 0
    }

    pub(super) fn read_stdin(buf: &mut [u8]) -> io::Result<usize> {
        let handle = stdin_handle();
        if !valid(handle) {
            return Ok(0);
        }
        // When VT input is enabled, ReadFile on the console returns the VT
        // byte stream, which is exactly what the engine parses. If no VT is
        // available, fall back to ReadConsoleInputW is out of scope; ReadFile
        // still yields cooked input, so the owner's console verification
        // (docs/conpty-testing-plan.md) is the place a gap would surface.
        let mut read = 0u32;
        // SAFETY: `buf` is writable for buf.len() bytes.
        let ok = unsafe {
            ReadFile(
                handle,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(read as usize)
    }

    pub(super) fn wait_stdin(timeout_ms: i32) -> io::Result<super::WaitOutcome> {
        // `Key` means a `ReadFile` will return promptly. A plain "handle is
        // signaled" wait is not enough: a console handle signals for non-key
        // events (focus, mouse, resize) too, and `ReadFile` then blocks forever
        // waiting for a key - which hung the reader thread on exit and
        // `drain_input` on the way there. `CancelSynchronousIo` was tried first
        // and does not reliably abort a console `ReadFile`.
        //
        // **A non-key record is no longer swallowed** (gh #323 defect 4). The
        // previous shape consumed it with `ReadConsoleInputW` and looped, so a
        // `WINDOW_BUFFER_SIZE_EVENT` - the console telling us the user resized -
        // was read and thrown away, and the loop only learned the new size on its
        // next timer tick. That is the #522 class: a resize that arrives while the
        // loop is waiting gets serviced up to a tick late, and a resize that
        // arrives while it is busy is delayed until the next wait. The record kind
        // is reported now so the reader can act on it immediately.
        let handle = stdin_handle();
        if !valid(handle) {
            return Ok(super::WaitOutcome::Timeout);
        }
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms.max(0) as u64);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok(super::WaitOutcome::Timeout);
            }
            // Wait for *any* input without polling: a console handle signals
            // for key and non-key events alike, so the key test happens after
            // the wait. A zero-millisecond wait would spin, hence the clamp.
            let wait_ms = remaining.as_millis().clamp(1, u32::MAX as u128) as u32;
            // SAFETY: `handle` is our console stdin handle.
            if unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(handle, wait_ms)
            } != 0
            {
                // WAIT_TIMEOUT or failure: nothing arrived in the window.
                return Ok(super::WaitOutcome::Timeout);
            }
            let mut count = 0u32;
            // SAFETY: valid handle, valid out-pointer.
            if unsafe { GetNumberOfConsoleInputEvents(handle, &mut count) } != 0 && count > 0 {
                let mut record = INPUT_RECORD::default();
                let mut peeked = 0u32;
                // SAFETY: one INPUT_RECORD out-param, valid for the call.
                if unsafe { PeekConsoleInputW(handle, &mut record, 1, &mut peeked) } != 0
                    && peeked > 0
                {
                    if record.EventType == KEY_EVENT as u16 {
                        // Gh #367: latch whether Shift was held on this Enter. The
                        // `KEY_EVENT` record carries `dwControlKeyState`, which is
                        // timestamped to the exact key event - strictly more accurate
                        // than polling the keyboard, and this is the peek path that
                        // already runs. `VK_RETURN` with Shift is the only case the
                        // editor needs, because a Shift-blind console sends both
                        // `Enter` and `Shift+Enter` as the same `\r`.
                        // SAFETY: `record` was filled by `PeekConsoleInputW` above.
                        let key_event = unsafe { record.Event.KeyEvent };
                        if key_event.wVirtualKeyCode == VK_RETURN {
                            SHIFT_PRESSED_ON_ENTER.store(
                                key_event.dwControlKeyState & SHIFT_PRESSED != 0,
                                std::sync::atomic::Ordering::SeqCst,
                            );
                        }
                        return Ok(super::WaitOutcome::Key);
                    }
                    // Gh #323 defect 4: a non-key record is consumed (it must be,
                    // or it stays at the head of the queue and the next peek sees
                    // the same one forever) but its *kind* decides what the loop
                    // does next. A resize read here is reported so the reader can
                    // re-query the size and repaint now, instead of waiting for
                    // its own tick - the difference between a resize that lands
                    // in the same frame and one that lands a tick later.
                    let was_resize = super::console_record_meaning(record.EventType);
                    let mut consumed = 0u32;
                    // SAFETY: same record, one event consumed.
                    unsafe {
                        ReadConsoleInputW(handle, &mut record, 1, &mut consumed);
                    }
                    if was_resize {
                        return Ok(super::WaitOutcome::Resize);
                    }
                }
            }
        }
    }

    /// Drain pending console input records so stop() does not leak keystrokes.
    #[allow(dead_code)]
    pub(super) fn drain_console_input() {
        let handle = stdin_handle();
        if !valid(handle) {
            return;
        }
        let mut rec = [0u8; 64];
        let mut read = 0u32;
        loop {
            // SAFETY: `rec` is a scratch buffer of one INPUT_RECORD (16 bytes
            // on x64; 64 is safely larger).
            let ok = unsafe { ReadConsoleInputW(handle, rec.as_mut_ptr().cast(), 1, &mut read) };
            if ok == 0 || read == 0 {
                break;
            }
        }
    }

    pub(super) fn terminal_size() -> io::Result<(u16, u16)> {
        let handle = stdout_handle();
        if !valid(handle) {
            return Ok((80, 24));
        }
        // SAFETY: info is a valid out-param for GetConsoleScreenBufferInfo.
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: handle valid, info valid.
        if unsafe { GetConsoleScreenBufferInfo(handle, &mut info) } == 0 {
            return Ok((80, 24));
        }
        let cols = info.srWindow.Right - info.srWindow.Left + 1;
        let rows = info.srWindow.Bottom - info.srWindow.Top + 1;
        Ok((cols.max(1) as u16, rows.max(1) as u16))
    }
}

#[cfg(test)]
#[path = "sys_tests.rs"]
mod tests;
