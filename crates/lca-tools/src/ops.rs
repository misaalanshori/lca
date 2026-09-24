//! Process execution for the built-in shell tool, behind the swappable
//! backend trait from ADR-0013: native on desktop, host-delegated on the
//! web target (FR-WEB-3).
//!
//! # Unsafe-code exemption
//!
//! Windows process-tree termination needs Job Object API calls; the
//! `windows` module below carries the exemption and a `SAFETY` note per
//! item. POSIX process groups use only safe `std`/`tokio` APIs.

#![deny(unsafe_code)]

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use crate::CancelFlag;

/// One directory entry as the tools see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path relative to the walked root, `/`-separated.
    pub rel_path: String,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// Size in bytes (0 for directories).
    pub len: u64,
}

/// File metadata the tools need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    /// Directory or file.
    pub is_dir: bool,
    /// Size in bytes.
    pub len: u64,
}

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecOutcome {
    /// The command exited on its own.
    Exit {
        /// Exit code (128 + signal when signalled).
        code: i32,
    },
    /// The timeout fired and the process tree was killed (FR-TOOL-5).
    Timeout,
    /// The turn was cancelled and the process tree was killed (FR-CONC-3).
    Cancelled,
}

/// Filesystem and process operations: the seam ADR-0013 names between the
/// build-time backends.
pub trait ToolOps: Send + Sync {
    /// Read a file's bytes.
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;
    /// Write a file, creating parent directories.
    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()>;
    /// Metadata for one path.
    fn stat(&self, path: &Path) -> std::io::Result<Stat>;
    /// Entries of one directory.
    fn list(&self, dir: &Path) -> std::io::Result<Vec<Entry>>;
    /// Recursive walk of `root`, symlinked entries skipped, emitting paths
    /// relative with `/` separators.
    fn walk(&self, root: &Path) -> std::io::Result<Vec<Entry>>;
    /// Run a command, streaming output chunks to `on_output` as they arrive
    /// (FR-TOOL-4), stopping on timeout or cancellation (FR-TOOL-5).
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a Path,
        timeout: Duration,
        on_output: &'a mut (dyn FnMut(&[u8]) + Send),
        cancel: CancelFlag,
    ) -> ExecFuture<'a>;
}

use std::future::Future;

/// The boxed, sendable future an [`ToolOps::exec`] call returns.
pub type ExecFuture<'a> =
    Pin<Box<dyn Future<Output = std::io::Result<(ExecOutcome, Vec<u8>)>> + Send + 'a>>;

/// The desktop backend: real files, real processes.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeOps;

const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules"];

impl ToolOps for NativeOps {
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)
    }

    fn stat(&self, path: &Path) -> std::io::Result<Stat> {
        let meta = std::fs::metadata(path)?;
        Ok(Stat {
            is_dir: meta.is_dir(),
            len: meta.len(),
        })
    }

    fn list(&self, dir: &Path) -> std::io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            entries.push(Entry {
                rel_path: entry.file_name().to_string_lossy().into_owned(),
                is_dir: file_type.is_dir(),
                len: entry.metadata().map(|m| m.len()).unwrap_or(0),
            });
        }
        Ok(entries)
    }

    fn walk(&self, root: &Path) -> std::io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
        while let Some((dir, prefix)) = stack.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if file_type.is_symlink() {
                    continue; // never follow a link out of the workspace
                }
                let rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                if file_type.is_dir() {
                    if SKIP_DIRS.contains(&name.as_str()) {
                        continue;
                    }
                    stack.push((entry.path(), rel.clone()));
                    out.push(Entry {
                        rel_path: rel,
                        is_dir: true,
                        len: 0,
                    });
                } else {
                    out.push(Entry {
                        rel_path: rel,
                        is_dir: false,
                        len: entry.metadata().map(|m| m.len()).unwrap_or(0),
                    });
                }
            }
        }
        Ok(out)
    }

    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a Path,
        timeout: Duration,
        on_output: &'a mut (dyn FnMut(&[u8]) + Send),
        cancel: CancelFlag,
    ) -> ExecFuture<'a> {
        Box::pin(async move { platform_exec(command, cwd, timeout, on_output, cancel).await })
    }
}

#[cfg(unix)]
async fn platform_exec(
    command: &str,
    cwd: &Path,
    timeout: Duration,
    on_output: &mut (dyn FnMut(&[u8]) + Send),
    cancel: CancelFlag,
) -> std::io::Result<(ExecOutcome, Vec<u8>)> {
    use tokio::io::AsyncReadExt;

    let cwd = crate::process::without_verbatim(cwd);
    if !cwd.is_dir() {
        return Err(std::io::Error::other(format!(
            "working directory does not exist: {}",
            cwd.display()
        )));
    }
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Own process group so a kill reaches the whole tree (ADR-0014,
    // platform notes): the pgid equals the child's pid.
    cmd.process_group(0);
    let mut child = cmd.spawn()?;
    let pgid = child.id().expect("child has a pid");
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");

    let mut collected: Vec<u8> = Vec::new();
    let mut outcome: Option<ExecOutcome> = None;
    let exec_start = tokio::time::Instant::now();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut out_buf = [0u8; 8192];
    let mut err_buf = [0u8; 8192];
    // Read both pipes to EOF before waiting on the child: exiting early on
    // one stream's EOF can drop data still buffered in the other.
    let mut stdout_open = true;
    let mut stderr_open = true;
    while stdout_open || stderr_open {
        tokio::select! {
            read = stdout.read(&mut out_buf), if stdout_open => {
                let n = read?;
                if n == 0 { stdout_open = false; }
                else { on_output(&out_buf[..n]); collected.extend_from_slice(&out_buf[..n]); }
            }
            read = stderr.read(&mut err_buf), if stderr_open => {
                let n = read?;
                if n == 0 { stderr_open = false; }
                else { on_output(&err_buf[..n]); collected.extend_from_slice(&err_buf[..n]); }
            }
            _ = tokio::time::sleep_until(deadline) => {
                eprintln!("TEMP-DIAG timeout arm after {:?}", exec_start.elapsed());
                kill_group(pgid);
                let _ = child.wait().await;
                outcome = Some(ExecOutcome::Timeout);
                break;
            }
            _ = cancel.wait_cancelled() => {
                kill_group(pgid);
                let _ = child.wait().await;
                outcome = Some(ExecOutcome::Cancelled);
                break;
            }
        }
    }
    if let Some(outcome) = outcome {
        return Ok((outcome, collected));
    }
    let status = child.wait().await?;
    let code = status.code().unwrap_or_else(|| {
        // Signalled: shell convention is 128 + signal number.
        use std::os::unix::process::ExitStatusExt;
        128 + status.signal().unwrap_or(1)
    });
    Ok((ExecOutcome::Exit { code }, collected))
}

#[cfg(unix)]
fn kill_group(pgid: u32) {
    // The child owns its process group (process_group(0)), so signaling the
    // group stops the shell and everything it forked. `/bin/kill` exists on
    // every POSIX platform we target; using it avoids a `libc` dependency.
    // ponytail: swap to `libc::killpg` if a platform ships no /bin/kill.
    let status = std::process::Command::new("/bin/kill")
        .args(["-9", &format!("-{pgid}")])
        .status();
    eprintln!("TEMP-DIAG kill_group pgid={pgid} status={status:?}");
}

#[cfg(windows)]
#[allow(unsafe_code)] // documented crate exemption: Job Object handles
pub(crate) mod windows_job {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// A job object that kills every assigned process when it closes, so no
    /// orphan outlives a timeout or cancellation (docs/platform-notes.md).
    pub struct Job {
        handle: HANDLE,
    }

    impl Job {
        /// Join `pid` to a fresh kill-on-close job; `None` when the OS
        /// refuses (the caller must not spawn a tree it cannot kill).
        pub(crate) fn attach(pid: u32) -> Option<Job> {
            let job = Job::new()?;
            if job.assign(pid) { Some(job) } else { None }
        }
    }

    // SAFETY: an owned Windows kernel handle. It is only used through the
    // Job Object API (assign, terminate, close), all of which are safe to
    // call from any thread; the handle is closed exactly once in `Drop`.
    // This lets the execution future stay `Send` across await points.
    unsafe impl Send for Job {}

    impl Job {
        pub fn new() -> Option<Job> {
            // SAFETY: no preconditions for CreateJobObjectW with null names.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return None;
            }
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `handle` is a valid job handle and `info` outlives the call.
            unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            Some(Job { handle })
        }

        pub fn assign(&self, pid: u32) -> bool {
            // SAFETY: PROCESS_SET_QUOTA | PROCESS_TERMINATE is what
            // AssignProcessToJobObject requires; a failed open returns null
            // and is handled.
            let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
            if process.is_null() {
                return false;
            }
            // SAFETY: both handles are valid and open for the required access.
            let ok = unsafe { AssignProcessToJobObject(self.handle, process) };
            // SAFETY: `process` came from OpenProcess and is not used after.
            unsafe { CloseHandle(process) };
            ok != 0
        }

        pub fn kill(&self) {
            // SAFETY: valid job handle; exit code1 is arbitrary.
            unsafe { TerminateJobObject(self.handle, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                // SAFETY: valid job handle, closed exactly once; KILL_ON_JOB_CLOSE
                // reaps every process still assigned to it.
                unsafe { CloseHandle(self.handle) };
            }
        }
    }
}

#[cfg(windows)]
async fn platform_exec(
    command: &str,
    cwd: &Path,
    timeout: Duration,
    on_output: &mut (dyn FnMut(&[u8]) + Send),
    cancel: CancelFlag,
) -> std::io::Result<(ExecOutcome, Vec<u8>)> {
    use tokio::io::AsyncReadExt;

    // Same verbatim strip as everywhere else: cmd.exe refuses a
    // \?\ working directory as UNC (platform-notes' class of bug).
    let cwd = crate::process::without_verbatim(cwd);
    if !cwd.is_dir() {
        return Err(std::io::Error::other(format!(
            "working directory does not exist: {}",
            cwd.display()
        )));
    }
    let mut cmd = tokio::process::Command::new("cmd.exe");
    cmd.arg("/C")
        .arg(command)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let job = windows_job::Job::new();
    if let (Some(job), Some(pid)) = (job.as_ref(), child.id()) {
        job.assign(pid);
    }
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");

    let mut collected: Vec<u8> = Vec::new();
    let mut outcome: Option<ExecOutcome> = None;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut out_buf = [0u8; 8192];
    let mut err_buf = [0u8; 8192];
    // Both pipes to EOF before waiting on the child (same race as unix).
    let mut stdout_open = true;
    let mut stderr_open = true;
    while stdout_open || stderr_open {
        tokio::select! {
            read = stdout.read(&mut out_buf), if stdout_open => {
                let n = read?;
                if n == 0 { stdout_open = false; }
                else { on_output(&out_buf[..n]); collected.extend_from_slice(&out_buf[..n]); }
            }
            read = stderr.read(&mut err_buf), if stderr_open => {
                let n = read?;
                if n == 0 { stderr_open = false; }
                else { on_output(&err_buf[..n]); collected.extend_from_slice(&err_buf[..n]); }
            }
            _ = tokio::time::sleep_until(deadline) => {
                if let Some(job) = job.as_ref() { job.kill(); } else { let _ = child.start_kill(); }
                let _ = child.wait().await;
                outcome = Some(ExecOutcome::Timeout);
                break;
            }
            _ = cancel.wait_cancelled() => {
                if let Some(job) = job.as_ref() { job.kill(); } else { let _ = child.start_kill(); }
                let _ = child.wait().await;
                outcome = Some(ExecOutcome::Cancelled);
                break;
            }
        }
    }
    if let Some(outcome) = outcome {
        return Ok((outcome, collected));
    }
    let status = child.wait().await?;
    let code = status.code().unwrap_or(1);
    Ok((ExecOutcome::Exit { code }, collected))
}
