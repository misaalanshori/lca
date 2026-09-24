//! Pseudo-terminal allocation for the `pty` capability (ADR-0016).
//!
//! The PTY itself is allocated host-side behind the import; the extension
//! only ever sees bytes through `read`/`write` and a `resize` control. A
//! program spawned under a pty sees a real terminal with rows, columns,
//! and raw mode available (capability catalog).
//!
//! # Unsafe-code exemption
//!
//! POSIX ptmx ioctls and Windows ConPTY are not reachable through `std`;
//! the `unsafe_ioctl` and `windows_conpty` modules below are this crate's
//! documented exemption, and every `unsafe` block carries a `SAFETY` note.

#![deny(unsafe_code)]

use std::path::Path;

/// A live pseudo-terminal session with its spawned program.
pub struct PtyChild {
    inner: Inner,
}

impl Drop for PtyChild {
    fn drop(&mut self) {
        // Nothing survives the handle: the session dies with it.
        self.kill();
    }
}

#[cfg(unix)]
struct Inner {
    /// Master side: read output, write keystrokes, resize.
    master: std::fs::File,
    child: std::process::Child,
    /// The child's process group, for a whole-tree kill.
    pgid: u32,
}

#[cfg(windows)]
struct Inner {
    /// Host end of the console input (keystrokes in).
    input: std::fs::File,
    /// Host end of the console output (terminal bytes out).
    output: std::fs::File,
    console: windows_conpty::Console,
    child: windows_conpty::ProcessHandle,
    /// Whole-tree kill, same identity the Job Object path uses.
    job: Option<crate::ops::windows_job::Job>,
}

impl PtyChild {
    /// Spawn `program` with `args` in `cwd` attached to a new
    /// pseudo-terminal sized `rows` x `cols`.
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> std::io::Result<PtyChild> {
        if !cwd.is_dir() {
            return Err(std::io::Error::other(format!(
                "working directory does not exist: {}",
                cwd.display()
            )));
        }
        spawn_impl(program, args, cwd, rows, cols).map(|inner| PtyChild { inner })
    }

    /// Read up to `max` terminal bytes; `None` means the session ended.
    pub fn read(&mut self, max: usize) -> std::io::Result<Option<Vec<u8>>> {
        read_impl(&mut self.inner, max)
    }

    /// Forward keystrokes (or other input) to the program.
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<u64> {
        write_impl(&mut self.inner, bytes)
    }

    /// Resize the terminal; the program receives SIGWINCH (or the Windows
    /// buffer-resize equivalent).
    pub fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
        resize_impl(&mut self.inner, rows, cols)
    }

    /// Wait for the program to exit; returns its exit code.
    pub fn wait(&mut self) -> std::io::Result<i32> {
        wait_impl(&mut self.inner)
    }

    /// Kill everything attached to the session and release it.
    pub fn kill(&mut self) {
        kill_impl(&mut self.inner);
    }
}

// ---------------------------------------------------------------------------
// POSIX: /dev/ptmx + ioctls
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[allow(unsafe_code)] // documented crate exemption: POSIX pty allocation
mod unsafe_ptmx {
    //! The POSIX pty surface std does not expose. The `posix_openpt`/
    //! `grantpt`/`unlockpt`/`ptsname` family is portable across every unix
    //! target we ship (libc defines it in `unix/mod.rs`), unlike the
    //! ptmx-specific ioctl constants, which differ per platform and do not
    //! exist in libc for macOS.

    use std::os::unix::io::{FromRawFd, RawFd};

    /// Open a master/slave pty pair's master side, unlocked and ready.
    pub(super) fn open_master() -> std::io::Result<std::fs::File> {
        // SAFETY: posix_openpt returns a fresh descriptor or -1; O_NOCTTY
        // keeps the slave from becoming a controlling terminal.
        let fd = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is open and owned by us.
        if unsafe { libc::grantpt(fd) } != 0 {
            let err = std::io::Error::last_os_error();
            // SAFETY: closing our own descriptor on the error path.
            unsafe { libc::close(fd) };
            return Err(err);
        }
        // SAFETY: same descriptor; unlock the slave side.
        if unsafe { libc::unlockpt(fd) } != 0 {
            let err = std::io::Error::last_os_error();
            // SAFETY: closing our own descriptor on the error path.
            unsafe { libc::close(fd) };
            return Err(err);
        }
        // SAFETY: `fd` transfers to the File, which closes it on drop.
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }

    /// The slave path for an open master (copied out immediately: the
    /// libc buffer is overwritten by the next call).
    pub(super) fn slave_path(fd: RawFd) -> std::io::Result<std::path::PathBuf> {
        use std::ffi::CStr;
        // SAFETY: ptsname writes into a libc-owned buffer for this fd.
        let ptr = unsafe { libc::ptsname(fd) };
        if ptr.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: NUL-terminated string from the C library.
        let name = unsafe { CStr::from_ptr(ptr) };
        Ok(std::path::PathBuf::from(
            name.to_string_lossy().into_owned(),
        ))
    }

    /// Set the window size on the master; the kernel signals the group.
    pub(super) fn set_winsize(fd: RawFd, rows: u16, cols: u16) -> std::io::Result<()> {
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: `fd` is open and `size` outlives the call.
        let rc = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) };
        if rc < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// The raw descriptor of an open file.
    /// Put the master into nonblocking mode: a live panel polls it
    /// between keystrokes, and "no data yet" has to be an empty read
    /// rather than a frozen frame (`read_impl` maps `WouldBlock`).
    pub(super) fn set_nonblocking(file: &std::fs::File) -> std::io::Result<()> {
        // SAFETY: our own open descriptor; F_GETFL/F_SETFL with
        // libc::O_NONBLOCK is the portable POSIX spelling (the constant
        // differs between Linux and macOS, which is why libc decides it
        // and not a literal here).
        unsafe {
            let flags = libc::fcntl(raw_fd(file), libc::F_GETFL);
            if flags < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(raw_fd(file), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub(super) fn raw_fd(file: &std::fs::File) -> RawFd {
        use std::os::unix::io::AsRawFd;
        file.as_raw_fd()
    }
}

#[cfg(unix)]
fn spawn_impl(
    program: &str,
    args: &[String],
    cwd: &Path,
    rows: u16,
    cols: u16,
) -> std::io::Result<Inner> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;

    let master = unsafe_ptmx::open_master()?;
    let fd = unsafe_ptmx::raw_fd(&master);
    unsafe_ptmx::set_winsize(fd, rows, cols)?;
    let slave_path = unsafe_ptmx::slave_path(fd)?;
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(&slave_path)?;

    let cwd = crate::process::without_verbatim(cwd);
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::from(slave.try_clone()?))
        .stdout(std::process::Stdio::from(slave.try_clone()?))
        .stderr(std::process::Stdio::from(slave));
    // Own process group so the whole session dies together.
    cmd.process_group(0);
    let child = cmd.spawn()?;
    let pgid = child.id();
    // The parent's slave copies moved into the child's stdio; holding one
    // open would stop the master from ever reporting EOF (platform notes
    // class of bug: inherited handles keep pipes alive).
    // A live panel polls the master between keystrokes: nonblocking
    // makes "no data yet" an empty read instead of a frozen frame
    // (read_impl maps WouldBlock to an empty chunk; EOF stays EIO).
    unsafe_ptmx::set_nonblocking(&master)?;
    Ok(Inner {
        master,
        child,
        pgid,
    })
}

#[cfg(unix)]
fn read_impl(inner: &mut Inner, max: usize) -> std::io::Result<Option<Vec<u8>>> {
    match crate::process::read_up_to(&mut inner.master, max) {
        Ok(bytes) => Ok(bytes),
        // Linux reports EIO on the master once every slave side is gone;
        // that is this stream's EOF, not a failure.
        Err(err) if err.raw_os_error() == Some(5) => Ok(None),
        // The master is nonblocking: no data yet is "nothing this
        // call", not EOF and not an error - a panel polling a live
        // session between keystrokes lands here every time.
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(Some(Vec::new())),
        Err(err) => Err(err),
    }
}

#[cfg(unix)]
fn write_impl(inner: &mut Inner, bytes: &[u8]) -> std::io::Result<u64> {
    crate::process::write_all(&mut inner.master, bytes)
}

#[cfg(unix)]
fn resize_impl(inner: &mut Inner, rows: u16, cols: u16) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    unsafe_ptmx::set_winsize(inner.master.as_raw_fd(), rows, cols)
}

#[cfg(unix)]
fn wait_impl(inner: &mut Inner) -> std::io::Result<i32> {
    let status = inner.child.wait()?;
    use std::os::unix::process::ExitStatusExt;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
}

#[cfg(unix)]
fn kill_impl(inner: &mut Inner) {
    // Group kill first: the child may have forked (platform notes).
    let _ = std::process::Command::new("/bin/kill")
        .args(["-9", &format!("-{}", inner.pgid)])
        .status();
    let _ = inner.child.kill();
    let _ = inner.child.wait();
}

// ---------------------------------------------------------------------------
// Windows: ConPTY
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[allow(unsafe_code)] // documented crate exemption: ConPTY handles
mod windows_conpty {
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Console::{
        COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE,
        InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
        PROCESS_INFORMATION, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject,
    };

    /// A pseudo-console. The host owns the input-write and output-read
    /// ends as `File`s; this owns only the console itself.
    pub struct Console {
        hpc: Option<HPCON>,
    }

    impl Console {
        /// Build the console plus the two host-side pipe ends
        /// (input-to-console write, console-output read).
        pub fn new(rows: u16, cols: u16) -> std::io::Result<(Console, HANDLE, HANDLE)> {
            let mut input_read: HANDLE = std::ptr::null_mut();
            let mut input_write: HANDLE = std::ptr::null_mut();
            let mut output_read: HANDLE = std::ptr::null_mut();
            let mut output_write: HANDLE = std::ptr::null_mut();
            // SAFETY: all four are valid out-pointer slots.
            unsafe {
                if CreatePipe(&mut input_read, &mut input_write, std::ptr::null(), 0) == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if CreatePipe(&mut output_read, &mut output_write, std::ptr::null(), 0) == 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            let mut hpc: HPCON = 0;
            let size = COORD {
                X: cols as i16,
                Y: rows as i16,
            };
            // SAFETY: valid pipe handles and a writable HPCON slot; ConPTY
            // duplicates the handles it keeps.
            if unsafe { CreatePseudoConsole(size, input_read, output_write, 0, &mut hpc) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: ConPTY duplicated its ends; closing ours makes EOF
            // propagate once the console closes.
            unsafe {
                CloseHandle(input_read);
                CloseHandle(output_write);
            }
            Ok((Console { hpc: Some(hpc) }, input_write, output_read))
        }

        /// Resize; the program receives the new buffer size.
        pub fn resize(&self, rows: u16, cols: u16) -> std::io::Result<()> {
            let Some(hpc) = self.hpc else { return Ok(()) };
            // SAFETY: valid HPCON, by-value COORD.
            if unsafe {
                ResizePseudoConsole(
                    hpc,
                    COORD {
                        X: cols as i16,
                        Y: rows as i16,
                    },
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        /// End the session: closing the console releases the output pipe
        /// so readers see EOF, and conhost signals its clients.
        pub fn close(&mut self) {
            if let Some(hpc) = self.hpc.take() {
                // SAFETY: valid HPCON, closed exactly once (taken above).
                unsafe { ClosePseudoConsole(hpc) };
            }
        }
    }

    // SAFETY: a pseudo-console handle is an owned kernel object usable
    // from any thread; it is only touched through the ConPTY API and
    // closed exactly once in Drop. This keeps the capability engine
    // (and therefore the host store data) Send, which WasiView requires.
    unsafe impl Send for Console {}

    impl Drop for Console {
        fn drop(&mut self) {
            self.close();
        }
    }

    /// A spawned console-attached process.
    pub struct ProcessHandle {
        handle: HANDLE,
        pid: u32,
    }

    impl ProcessHandle {
        /// Whether the process has exited, without waiting on it: a
        /// zero-time Wait on the handle answers in microseconds.
        pub fn exited(&self) -> bool {
            // WAIT_OBJECT_0 is zero: the object is signaled.
            // SAFETY: the handle is open for this process's lifetime.
            let signaled = unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(self.handle, 0)
            };
            signaled == 0
        }

        /// The spawned process id, for Job Object attachment.
        pub fn pid(&self) -> Option<u32> {
            (self.pid != 0).then_some(self.pid)
        }

        /// Block until exit; returns the exit code.
        pub fn wait(&mut self) -> i32 {
            // SAFETY: valid process handle owned by us.
            unsafe {
                WaitForSingleObject(self.handle, INFINITE);
                let mut code: u32 = 0;
                if GetExitCodeProcess(self.handle, &mut code) != 0 {
                    code as i32
                } else {
                    -1
                }
            }
        }

        /// Terminate the process.
        pub fn kill(&mut self) {
            // SAFETY: valid handle for a process we created.
            unsafe {
                TerminateProcess(self.handle, 1);
                WaitForSingleObject(self.handle, INFINITE);
            }
        }
    }

    // SAFETY: an owned process handle, terminated/waited only through the
    // Win32 process API from any thread; closed exactly once in Drop.
    unsafe impl Send for ProcessHandle {}

    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            // SAFETY: the handle is ours; closed exactly once.
            unsafe { CloseHandle(self.handle) };
        }
    }

    /// Spawn `program args` in `cwd` attached to `console`.
    pub fn spawn_with_console(
        program: &str,
        args: &[String],
        cwd: &Path,
        console: &Console,
    ) -> std::io::Result<ProcessHandle> {
        let quote = |arg: &str| {
            if arg.contains(' ') || arg.contains('"') {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.to_string()
            }
        };
        let mut cmdline = format!("\"{program}\"");
        for arg in args {
            cmdline.push(' ');
            cmdline.push_str(&quote(arg));
        }
        let mut cmdline_wide: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
        let cwd = crate::process::without_verbatim(cwd);
        let cwd_wide: Vec<u16> = cwd
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let mut attr_size: usize = 0;
        // SAFETY: documented two-call probe with a null list.
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
        }
        if attr_size == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut attr_list = vec![0u8; attr_size];
        // SAFETY: the buffer matches the probe's requested size.
        if unsafe {
            InitializeProcThreadAttributeList(
                attr_list.as_mut_ptr() as *mut _,
                1,
                0,
                &mut attr_size,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: STARTUPINFOW zeroed is the documented initialization.
        let zeroed_startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        let startup = STARTUPINFOEXW {
            StartupInfo: STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
                ..zeroed_startup
            },
            lpAttributeList: attr_list.as_mut_ptr() as *mut _,
        };
        // The attribute's value parameter is a pointer TO an HPCON,
        // not the handle itself: passing the handle's bits as an
        // address made the call fail (its return was ignored), the
        // list carried no pseudoconsole, and every child was born on
        // the parent's own console - its output went to the runner's
        // stdout instead of the pipe, which is why three Windows pty
        // tests saw a program that ran and printed nothing they could
        // read, and the panel's keystrokes echoed into the void.
        let hpc_value = console.hpc.unwrap_or(0);
        let updated = unsafe {
            UpdateProcThreadAttribute(
                startup.lpAttributeList,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                &raw const hpc_value as *const core::ffi::c_void,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if updated == 0 {
            return Err(std::io::Error::last_os_error());
        }
        eprintln!("TEMP-DIAG spawn cmdline={cmdline:?} hpc={hpc_value:?}");
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: all pointers valid and owned across the call;
        // CreateProcessW may mutate the buffers we own.
        let created = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmdline_wide.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                std::ptr::null_mut(),
                cwd_wide.as_ptr(),
                &startup.StartupInfo,
                &mut info,
            )
        };
        // SAFETY: the list is finished; its buffer is dropped with Vec.
        unsafe { DeleteProcThreadAttributeList(startup.lpAttributeList) };
        if created == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: close the thread handle we do not need; the process
        // handle transfers to ProcessHandle below.
        unsafe { CloseHandle(info.hThread) };
        Ok(ProcessHandle {
            handle: info.hProcess,
            pid: info.dwProcessId,
        })
    }

    /// Wrap a raw pipe end as a `File`.
    pub fn file_from_pipe(handle: HANDLE) -> std::fs::File {
        // SAFETY: caller transfers ownership of an open pipe handle.
        unsafe { std::fs::File::from_raw_handle(handle as *mut _) }
    }
}

#[cfg(windows)]
fn spawn_impl(
    program: &str,
    args: &[String],
    cwd: &Path,
    rows: u16,
    cols: u16,
) -> std::io::Result<Inner> {
    let (console, input_write, output_read) = windows_conpty::Console::new(rows, cols)?;
    let input = windows_conpty::file_from_pipe(input_write);
    let output = windows_conpty::file_from_pipe(output_read);
    let child = windows_conpty::spawn_with_console(program, args, cwd, &console)?;
    let job = child.pid().and_then(crate::ops::windows_job::Job::attach);
    Ok(Inner {
        input,
        output,
        console,
        child,
        job,
    })
}

#[cfg(windows)]
// The crate exemption for raw handles covers this function the same
// way it covers `windows_conpty`: the peek is over an owned pipe end.
#[allow(unsafe_code)]
fn read_impl(inner: &mut Inner, max: usize) -> std::io::Result<Option<Vec<u8>>> {
    // The ConPTY output pipe reaches EOF only when the console is
    // closed, and the console is closed after the child is waited -
    // which callers do after this function reports the end of the
    // stream. A reader waiting for that EOF was therefore waiting on
    // its own next call: every Windows pty consumer deadlocked into a
    // timeout (conformance, the ui panel, the conformance diff all
    // tripped over it). Peek first, so no data is "nothing yet" like
    // the nonblocking master on POSIX, and report the end of the
    // stream when the child has exited and the pipe has drained.
    use std::os::windows::io::AsRawHandle;
    let mut available: u32 = 0;
    // SAFETY: the file is an open pipe end; the out params are valid.
    let ok = unsafe {
        windows_sys::Win32::System::Pipes::PeekNamedPipe(
            inner.output.as_raw_handle() as *mut _,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let err = std::io::Error::last_os_error();
        return match err.kind() {
            // Broken pipe: the console side is gone; that is EOF.
            std::io::ErrorKind::BrokenPipe => Ok(None),
            std::io::ErrorKind::UnexpectedEof => Ok(None),
            _ => Err(err),
        };
    }
    if available > 0 {
        let chunk = crate::process::read_up_to(&mut inner.output, max)?;
        eprintln!(
            "TEMP-DIAG read avail={available} got={:?}",
            chunk.as_ref().map(|c| c.len())
        );
        return Ok(chunk);
    }
    let exited = inner.child.exited();
    eprintln!("TEMP-DIAG read avail=0 exited={exited}");
    if !exited {
        // Nothing yet: yield the CPU rather than hammering. The
        // ConPTY pump lives in this same process, and a reader in a
        // tight loop on a loaded runner can crowd out the very thread
        // whose output it is waiting for. Two milliseconds is
        // imperceptible to a frame poll.
        std::thread::sleep(std::time::Duration::from_millis(2));
        return Ok(Some(Vec::new()));
    }
    // The child is gone and the pipe is empty - but ConPTY can hold
    // the program's output in its own conversion pipeline until the
    // console goes away, and a fast child (cmd /C echo) exits before
    // anything it wrote reaches the pipe: tests saw only the
    // console's opening escape sequences. Closing the console is the
    // documented flush - wait_impl's comment says it releases the
    // output pipe for exactly this reason - and it is idempotent
    // (hpc.take), so the reader can do it too: the final bytes come
    // out, then the next call reads the EOF that follows.
    inner.console.close();
    crate::process::read_up_to(&mut inner.output, max)
}

#[cfg(windows)]
fn write_impl(inner: &mut Inner, bytes: &[u8]) -> std::io::Result<u64> {
    crate::process::write_all(&mut inner.input, bytes)
}

#[cfg(windows)]
fn resize_impl(inner: &mut Inner, rows: u16, cols: u16) -> std::io::Result<()> {
    inner.console.resize(rows, cols)
}

#[cfg(windows)]
fn wait_impl(inner: &mut Inner) -> std::io::Result<i32> {
    let code = inner.child.wait();
    // Closing the console releases the output pipe so readers see EOF.
    inner.console.close();
    Ok(code)
}

#[cfg(windows)]
fn kill_impl(inner: &mut Inner) {
    if let Some(job) = inner.job.take() {
        job.kill();
    }
    inner.child.kill();
    inner.console.close();
}
