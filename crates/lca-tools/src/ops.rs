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
    /// `None` runs until the command exits or the turn is cancelled.
    /// The session context (gh #129) reaches the child as `LCA_*`
    /// variables; `None` spawns with the ambient environment only.
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a Path,
        timeout: Option<Duration>,
        on_output: &'a mut (dyn FnMut(&[u8]) + Send),
        cancel: CancelFlag,
        env: Option<&'a crate::SessionEnv>,
    ) -> ExecFuture<'a>;

    /// The interpreter this backend runs commands in, when it owns one
    /// (the host-delegated web backend does not). The tool description and
    /// `/settings` report it (ADR-0041).
    fn shell(&self) -> Option<&crate::shell::Shell> {
        None
    }

    /// The shell resolution error, when the backend is broken (a configured
    /// interpreter that could not be found). `/settings` reports it instead
    /// of claiming a working interpreter.
    fn shell_error(&self) -> Option<&str> {
        None
    }
}

use std::future::Future;

/// The boxed, sendable future an [`ToolOps::exec`] call returns.
pub type ExecFuture<'a> =
    Pin<Box<dyn Future<Output = std::io::Result<(ExecOutcome, Vec<u8>)>> + Send + 'a>>;

/// The session context a shell child sees in its environment (gh #129,
/// pi's `PI_SESSION_ID`/`PI_PROVIDER`/`PI_MODEL` row under `LCA_*` names).
/// Identifiers and directory paths only: nothing here is a secret, so
/// echoing it into scrollback or logs is safe by construction.
#[derive(Debug, Clone, Default)]
pub struct SessionEnv {
    /// The session id (`echo $LCA_SESSION_ID` prints it, the literal
    /// acceptance).
    pub session_id: String,
    /// The session's storage directory.
    pub session_dir: PathBuf,
    /// The active provider extension's name.
    pub provider: String,
    /// The active model identifier.
    pub model: String,
    /// The effective thinking level, when the session sets one.
    pub thinking: Option<String>,
    /// The data home (`~/.lca`).
    pub data_dir: PathBuf,
}

impl SessionEnv {
    /// Export the six variables onto a child command, leaving every
    /// ambient variable untouched (an ambient `LCA_MODEL` from the
    /// user's own shell keeps its value — the session's wins here).
    pub fn apply(&self, cmd: &mut tokio::process::Command) {
        cmd.env("LCA_SESSION_ID", &self.session_id)
            .env("LCA_SESSION_DIR", &self.session_dir)
            .env("LCA_PROVIDER", &self.provider)
            .env("LCA_MODEL", &self.model)
            .env("LCA_DATA_DIR", &self.data_dir);
        if let Some(thinking) = &self.thinking {
            cmd.env("LCA_THINKING", thinking);
        }
    }
}

/// The built-in tool executor.
/// The desktop backend: real files, real processes.
#[derive(Debug, Clone)]
pub struct NativeOps {
    /// The interpreter the `shell` tool runs (ADR-0041), resolved once at
    /// startup so the tool description, `/settings`, and every call agree.
    shell: crate::shell::Shell,
    /// Set when a *configured* interpreter could not be resolved: every
    /// call fails with this message instead of silently running in some
    /// other shell (ADR-0041's "never silent fallback").
    error: Option<String>,
}

impl NativeOps {
    /// A backend running commands in `shell`.
    pub fn new(shell: crate::shell::Shell) -> NativeOps {
        NativeOps { shell, error: None }
    }

    /// A backend whose configured interpreter could not be resolved. The
    /// shell is the ladder's fallback for description purposes; every call
    /// fails with `error`.
    pub fn broken(shell: crate::shell::Shell, error: String) -> NativeOps {
        NativeOps {
            shell,
            error: Some(error),
        }
    }

    /// The resolution error, when this backend is broken.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The interpreter this backend will spawn.
    pub fn shell(&self) -> &crate::shell::Shell {
        &self.shell
    }
}

impl Default for NativeOps {
    fn default() -> NativeOps {
        use crate::shell::Probe as _;
        NativeOps::new(crate::shell::Shell::fallback(crate::shell::Real.os()))
    }
}

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
        // ripgrep's walker (gh #118, #119): `.gitignore`, global and
        // parent excludes, and negations are honored; hidden files are
        // still walked (pi passes `--hidden`), symlinks never followed.
        // The build-output floor stays: `.git`, `target`, and
        // `node_modules` never enumerate even when no ignore file names
        // them. `require_git(false)` keeps ignores working outside a
        // repo, pi's `fd --no-require-git` posture.
        if !root.is_dir() {
            return Err(std::io::Error::other(format!(
                "not a directory: {}",
                root.display()
            )));
        }
        let mut out = Vec::new();
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .require_git(false)
            .filter_entry(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_none_or(|name| !SKIP_DIRS.contains(&name))
            })
            .build();
        for entry in walker {
            let entry = entry.map_err(std::io::Error::other)?;
            let path = entry.path();
            if path == root {
                continue;
            }
            let Some(rel) = path
                .strip_prefix(root)
                .ok()
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            else {
                continue;
            };
            let file_type = entry
                .file_type()
                .ok_or_else(|| std::io::Error::other(format!("cannot stat {}", path.display())))?;
            if file_type.is_symlink() {
                continue; // never follow a link out of the workspace
            }
            out.push(Entry {
                rel_path: rel,
                is_dir: file_type.is_dir(),
                len: if file_type.is_dir() {
                    0
                } else {
                    entry.metadata().map(|meta| meta.len()).unwrap_or(0)
                },
            });
        }
        Ok(out)
    }

    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a Path,
        timeout: Option<Duration>,
        on_output: &'a mut (dyn FnMut(&[u8]) + Send),
        cancel: CancelFlag,
        env: Option<&'a crate::SessionEnv>,
    ) -> ExecFuture<'a> {
        let error = self.error.clone();
        Box::pin(async move {
            if let Some(error) = error {
                return Err(std::io::Error::other(error));
            }
            platform_exec(&self.shell, command, cwd, timeout, on_output, cancel, env).await
        })
    }

    fn shell(&self) -> Option<&crate::shell::Shell> {
        Some(&self.shell)
    }

    fn shell_error(&self) -> Option<&str> {
        self.error()
    }
}

/// The most bytes the shell tool keeps for its final result. Output streams to
/// the interface as it arrives; only the tail is buffered (and the result is
/// truncated to the configured limit anyway), so a command that emits
/// gigabytes cannot grow this buffer without bound.
const OUTPUT_BUFFER_CAP: usize = 4 * 1024 * 1024;

fn push_capped(buffer: &mut Vec<u8>, chunk: &[u8]) {
    buffer.extend_from_slice(chunk);
    if buffer.len() > OUTPUT_BUFFER_CAP {
        let excess = buffer.len() - OUTPUT_BUFFER_CAP;
        buffer.drain(..excess);
    }
}

#[cfg(unix)]
async fn platform_exec(
    shell: &crate::shell::Shell,
    command: &str,
    cwd: &Path,
    timeout: Option<Duration>,
    on_output: &mut (dyn FnMut(&[u8]) + Send),
    cancel: CancelFlag,
    env: Option<&crate::SessionEnv>,
) -> std::io::Result<(ExecOutcome, Vec<u8>)> {
    use tokio::io::AsyncReadExt;

    let cwd = crate::process::without_verbatim(cwd);
    if !cwd.is_dir() {
        return Err(std::io::Error::other(format!(
            "working directory does not exist: {}",
            cwd.display()
        )));
    }
    // The POSIX path is unchanged (ADR-0041): `-c` receives the command as
    // one argv element, which `execve` carries byte for byte. The
    // configured prefix joins first (gh #133), so profile lines take
    // effect for the command.
    let mut cmd = tokio::process::Command::new(&shell.program);
    // gh #129: the session context reaches the child as `LCA_*`.
    if let Some(env) = env {
        env.apply(&mut cmd);
    }
    cmd.arg("-c")
        .arg(shell.command_text(command))
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Own process group so a kill reaches the whole tree (ADR-0014,
    // platform notes): the pgid equals the child's pid.
    cmd.process_group(0);
    let mut child = cmd.spawn()?;
    let Some(pgid) = child.id() else {
        return Err(std::io::Error::other("the spawned shell has no pid"));
    };
    let Some(mut stdout) = child.stdout.take() else {
        return Err(std::io::Error::other("the shell's stdout was not piped"));
    };
    let Some(mut stderr) = child.stderr.take() else {
        return Err(std::io::Error::other("the shell's stderr was not piped"));
    };

    let mut collected: Vec<u8> = Vec::new();
    let mut outcome: Option<ExecOutcome> = None;
    let exec_start = tokio::time::Instant::now();
    let deadline = timeout.map(|limit| tokio::time::Instant::now() + limit);
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
                else { on_output(&out_buf[..n]); push_capped(&mut collected, &out_buf[..n]); }
            }
            read = stderr.read(&mut err_buf), if stderr_open => {
                let n = read?;
                if n == 0 { stderr_open = false; }
                else { on_output(&err_buf[..n]); push_capped(&mut collected, &err_buf[..n]); }
            }
            _ = async { match deadline { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending().await } } => {
                kill_group(pgid);
                wait_after_kill(&mut child, pgid, exec_start).await;
                outcome = Some(ExecOutcome::Timeout);
                break;
            }
            _ = cancel.wait_cancelled() => {
                kill_group(pgid);
                wait_after_kill(&mut child, pgid, exec_start).await;
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
    let _ = std::process::Command::new("/bin/kill")
        .args(["-9", "--", &format!("-{pgid}")])
        .status();
}

/// The backstop: signal every surviving member by its own pid. The
/// group form (`kill -9 -PGID`) reported success on a hosted Linux
/// runner while both members sat alive - the dump of the group two
/// seconds later is in the run artifacts - and a cancellation that
/// does not cancel is exactly the failure FR-TOOL-5 exists to catch.
/// Individual pids leave no parsing to get wrong.
#[cfg(unix)]
fn kill_members(pgid: u32) {
    let out = std::process::Command::new("ps")
        .args(["-eo", "pid=,pgid=,cmd="])
        .output();
    let Ok(out) = out else { return };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let mut cols = line.split_whitespace();
        let (Some(pid), Some(g)) = (cols.next(), cols.next()) else {
            continue;
        };
        if g == pgid.to_string()
            && let Ok(pid) = pid.parse::<i32>()
        {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-9", &pid.to_string()])
                .status();
        }
    }
}

/// Reap the child after a group kill, bounded: a process still alive
/// two seconds after SIGKILL is worth seeing rather than awaiting
/// into an unexplained thirty-second stall - print the group's state,
/// then make sure. The evidence trail that led here: the timeout arm
/// fired on time, `/bin/kill` reported success, and both processes -
/// the sh and its sleep - were still alive and printable two seconds
/// later on a hosted Linux runner (macOS and Windows never showed it).
/// The group call now uses `--` to make the operand unambiguous, and
/// this backstop signals each surviving member by its own pid.
#[cfg(unix)]
async fn wait_after_kill(
    child: &mut tokio::process::Child,
    pgid: u32,
    since: tokio::time::Instant,
) {
    if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let ps = std::process::Command::new("ps")
            .args(["-eo", "pid,ppid,pgid,stat,cmd"])
            .output()
            .map(|out| {
                let all = String::from_utf8_lossy(&out.stdout);
                all.lines()
                    .filter(|line| line.contains(&format!(" {pgid} ")))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        eprintln!(
            "process group {pgid} still alive2s after SIGKILL, {:?} since exec, killing members by pid: {ps}",
            since.elapsed()
        );
        kill_members(pgid);
        let _ = child.start_kill();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
    }
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

/// A per-call command script, deleted when the call ends (including
/// timeout, cancellation, and error paths, because a `Drop` guard cannot be
/// forgotten the way an explicit cleanup can).
///
/// Deliberately *not* `#[cfg(windows)]`: it is only used by the Windows
/// transport, but compiling and testing it everywhere is what keeps its
/// types checked on a Linux or macOS dev machine. Two Windows-only compile
/// errors reached CI before this note existed.
#[allow(dead_code)]
struct TempScript {
    path: PathBuf,
}

#[allow(dead_code)]
impl TempScript {
    /// Write `command` in `shell`'s script dialect under the system temp
    /// directory.
    fn write(shell: &crate::shell::Shell, command: &str) -> std::io::Result<TempScript> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "lca-cmd-{}-{seq}.{}",
            std::process::id(),
            shell.script_extension()
        ));
        // Windows PowerShell 5.1 decodes a BOM-less `.ps1` in the ANSI
        // codepage, so a non-ASCII command arrives mojibaked: a UTF-8 BOM
        // tells both PowerShell flavors to read UTF-8. PowerShell skips the
        // BOM, so the command itself is untouched (ADR-0041's fidelity rule
        // in its last platform-specific detail; the corpus row is what
        // caught it).
        let mut bytes = Vec::with_capacity(shell.script_text(command).len() + 3);
        if matches!(
            shell.kind,
            crate::shell::Kind::Pwsh | crate::shell::Kind::PowerShell
        ) {
            bytes.extend_from_slice(b"\xef\xbb\xbf");
        }
        bytes.extend_from_slice(shell.script_text(command).as_bytes());
        std::fs::write(&path, bytes)?;
        Ok(TempScript { path })
    }

    /// The path as the shell's argv wants it.
    fn path(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
async fn platform_exec(
    shell: &crate::shell::Shell,
    command: &str,
    cwd: &Path,
    timeout: Option<Duration>,
    on_output: &mut (dyn FnMut(&[u8]) + Send),
    cancel: CancelFlag,
    env: Option<&crate::SessionEnv>,
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
    // ADR-0041's fidelity rule: the command travels in a file, never as one
    // argv element through CreateProcess quoting into a shell that then
    // re-parses its command line. That transport is where `"double"` became
    // `\"double\"` and a second line vanished. The configured prefix
    // joins before the dialect normalization (gh #133).
    let script = TempScript::write(shell, &shell.command_text(command))?;
    let mut cmd = tokio::process::Command::new(&shell.program);
    // gh #129: the session context reaches the child as `LCA_*`.
    if let Some(env) = env {
        env.apply(&mut cmd);
    }
    cmd.args(shell.script_args(&script.path()))
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
    let Some(mut stdout) = child.stdout.take() else {
        return Err(std::io::Error::other(format!(
            "{}'s stdout was not piped",
            shell.program
        )));
    };
    let Some(mut stderr) = child.stderr.take() else {
        return Err(std::io::Error::other(format!(
            "{}'s stderr was not piped",
            shell.program
        )));
    };

    let mut collected: Vec<u8> = Vec::new();
    let mut outcome: Option<ExecOutcome> = None;
    let deadline = timeout.map(|limit| tokio::time::Instant::now() + limit);
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
                else { on_output(&out_buf[..n]); push_capped(&mut collected, &out_buf[..n]); }
            }
            read = stderr.read(&mut err_buf), if stderr_open => {
                let n = read?;
                if n == 0 { stderr_open = false; }
                else { on_output(&err_buf[..n]); push_capped(&mut collected, &err_buf[..n]); }
            }
            _ = async { match deadline { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending().await } } => {
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

#[cfg(test)]
mod script_transport_tests {
    use super::TempScript;

    // Verifies: FR-TOOL-9 (ADR-0041) - the Windows transport's script file
    // is written with the dialect's extension and the command's exact
    // bytes, and is deleted when the guard drops. Compiled and run on every
    // platform: this is the half of the Windows path that used to be
    // invisible to a Linux dev machine.
    #[test]
    fn the_script_transport_writes_and_removes_the_file() {
        use crate::shell::{Kind, Os, Shell, Transport};
        let cases = [
            (Kind::Bash, "sh", "echo \"double\"\necho two"),
            (Kind::Pwsh, "ps1", "'double'\r\n'two'"),
            (Kind::Cmd, "cmd", "echo \"double\"\r\necho two"),
        ];
        for (kind, extension, command) in cases {
            let shell = Shell {
                program: "shell.exe".to_string(),
                kind,
                explicit: false,
                transport: Transport::ScriptFile,
                command_prefix: None,
            };
            let script = TempScript::write(&shell, command).expect("write");
            let path = std::path::PathBuf::from(script.path());
            assert_eq!(
                path.extension().and_then(|e| e.to_str()),
                Some(extension),
                "extension for {kind:?}"
            );
            let raw = std::fs::read(&path).expect("read back");
            let bom = matches!(
                kind,
                crate::shell::Kind::Pwsh | crate::shell::Kind::PowerShell
            );
            let expected: Vec<u8> = if bom {
                let mut with_bom = b"\xef\xbb\xbf".to_vec();
                with_bom.extend_from_slice(shell.script_text(command).as_bytes());
                with_bom
            } else {
                shell.script_text(command).into_bytes()
            };
            assert_eq!(raw, expected, "{kind:?} bytes");
            assert!(path.is_file(), "script exists while in use");
            drop(script);
            assert!(!path.exists(), "script removed on drop: {path:?}");
        }
        // And for a POSIX shell the transport is argv, not a file: the
        // script text and argv are both the command itself.
        let sh = Shell {
            program: "sh".to_string(),
            kind: Kind::Sh,
            explicit: false,
            transport: Transport::Argv,
            command_prefix: None,
        };
        assert_eq!(sh.script_text("a\nb"), "a\nb");
        let _ = Os::Unix;
    }
}
