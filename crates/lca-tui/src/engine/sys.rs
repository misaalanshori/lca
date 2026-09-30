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

/// Wait up to `timeout_ms` for stdin to become readable.
pub(crate) fn wait_stdin(timeout_ms: i32) -> io::Result<bool> {
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
                return Ok(false);
            }
            return Err(err);
        }
        Ok(rc > 0 && (pfd.revents & libc::POLLIN) != 0)
    }
    #[cfg(windows)]
    {
        windows_backend::wait_stdin(timeout_ms)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = timeout_ms;
        Ok(false)
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
            unsafe { SetConsoleMode(stdin, new_mode) };
        }
        if have_stdout {
            // SAFETY: enable ANSI processing on the output console.
            unsafe { SetConsoleMode(stdout, stdout_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) };
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

    pub(super) fn wait_stdin(timeout_ms: i32) -> io::Result<bool> {
        // Report `true` only when a *key* event is pending, so the caller's
        // `ReadFile` returns promptly. A plain "handle is signaled" wait is
        // not enough: a console handle signals for non-key events (focus,
        // mouse, resize) too, and `ReadFile` then blocks forever waiting for
        // a key - which hung the reader thread on exit and `drain_input` on
        // the way there. Non-key records are consumed here so they cannot
        // pile up. `CancelSynchronousIo` was tried first and does not
        // reliably abort a console `ReadFile`.
        let handle = stdin_handle();
        if !valid(handle) {
            return Ok(false);
        }
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms.max(0) as u64);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok(false);
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
                return Ok(false);
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
                        return Ok(true);
                    }
                    let mut consumed = 0u32;
                    // SAFETY: same record, one event consumed.
                    unsafe {
                        ReadConsoleInputW(handle, &mut record, 1, &mut consumed);
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
