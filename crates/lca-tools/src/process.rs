//! Direct program execution (no shell) with whole-tree lifetime, shared by
//! the built-in shell tool's platform code and the `process` capability.
//!
//! `TreeChild` owns a spawned program and whatever it forked: POSIX kills
//! the process group the spawn created, Windows kills the Job Object the
//! spawn joined (docs/platform-notes.md).

#![deny(unsafe_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// How a spawned tree is terminated as one unit.
enum Tree {
    /// The child leads its own process group (POSIX).
    #[cfg(unix)]
    Group {
        /// The group id, equal to the child's pid after `process_group(0)`.
        pgid: u32,
    },
    /// The child is assigned to a Job Object killed on close (Windows).
    #[cfg(windows)]
    Job(crate::ops::windows_job::Job),
}

/// Signal a whole process group, sharing the one correct argv shape.
/// The `--` is load-bearing, not style: without it procps `/bin/kill`
/// (before 4.0.5, e.g. Ubuntu 24.04's 4.0.2) truncates `-<pgid>` to its
/// first digit, so a pgid starting with `1` becomes `kill(-1, SIGKILL)`
/// and ends every process of our uid, including the test runner and the CI
/// agent itself (silent 30-minute cancel, 2026-10-15). The `pgid > 1`
/// guard keeps `-0` (own group) and `-1` (everyone) unreachable.
#[cfg(unix)]
pub(crate) fn kill_process_group(pgid: u32) {
    if pgid > 1 {
        // Silence both pipes: a group that already exited is the *common*
        // case here (the child died on its own, and teardown still runs), and
        // `/bin/kill` prints `kill: (-1234): No such process` on stderr for
        // it. That line leaked into `lca mcp list`'s output once
        // (gh #418 Phase F fix-up) - a teardown detail is not the user's
        // business, and the exit status is deliberately discarded anyway.
        let _ = std::process::Command::new("/bin/kill")
            .args(kill_group_argv(pgid))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// The argv `kill_process_group` passes: `kill -9 -- -PGID`.
#[cfg(any(unix, test))]
fn kill_group_argv(pgid: u32) -> [String; 3] {
    ["-9".to_string(), "--".to_string(), format!("-{pgid}")]
}

/// A spawned child plus its pipes and tree identity.
pub struct TreeChild {
    child: std::process::Child,
    tree: Tree,
}

impl TreeChild {
    /// The child's pid (also its pgid on POSIX).
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Take the child's stdin pipe.
    pub fn stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.child.stdin.take()
    }

    /// Take the child's stdout pipe.
    pub fn stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.stdout.take()
    }

    /// Take the child's stderr pipe.
    pub fn stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.stderr.take()
    }

    /// Wait for exit; returns the shell-convention code (128+signal when
    /// signalled).
    pub fn wait(&mut self) -> std::io::Result<i32> {
        let status = self.child.wait()?;
        Ok(exit_code(&status))
    }

    /// Non-blocking check for exit.
    pub fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        Ok(self.child.try_wait()?.map(|status| exit_code(&status)))
    }

    /// Kill the whole tree and reap the child.
    pub fn kill_tree(&mut self) {
        #[cfg(unix)]
        {
            let Tree::Group { pgid } = self.tree;
            // The child owns its process group; the group kill reaches
            // everything it forked. `/bin/kill` exists on every POSIX
            // target we ship; avoids a `libc` dependency here.
            // ponytail: swap to libc::killpg if a platform lacks /bin/kill.
            kill_process_group(pgid);
        }
        #[cfg(windows)]
        {
            // Single-variant on Windows: the Job Object kill is the tree kill.
            let Tree::Job(job) = &self.tree;
            job.kill();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for TreeChild {
    fn drop(&mut self) {
        // Nothing survives the handle: capability and tool spawns are
        // bounded work (capability catalog: long-lived background tasks are
        // deliberately absent from1.0).
        self.kill_tree();
    }
}

#[cfg(unix)]
fn exit_code(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

#[cfg(windows)]
fn exit_code(status: &std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

/// Drop Windows' verbatim prefix before a path becomes somebody
/// else's working directory: `\?\C:\...` is legal for our own
/// filesystem calls (the scope check canonicalizes into exactly that
/// form), but cmd.exe reads it as a UNC path, refuses it, and defaults
/// to C:\Windows - the platform-notes class of "subtly wrong rather
/// than obviously wrong". `\?\UNC\server\share` keeps its meaning
/// as `\server\share`.
pub fn without_verbatim(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

/// [`spawn_direct`] with extra environment variables.
///
/// The variables are **overlaid** on the inherited environment, never
/// substituted for it: a child that lost `PATH` could not run anything, so
/// "set these" has to mean "in addition to what I already have" (pi's rule,
/// `stdio.ts`: `{ ...process.env, ...this.options.env }`). gh #418 Phase
/// C#C1 needs this for an MCP server's `env` block.
pub fn spawn_direct_with_env(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &std::collections::BTreeMap<String, String>,
) -> std::io::Result<TreeChild> {
    if !cwd.is_dir() {
        return Err(std::io::Error::other(format!(
            "working directory does not exist: {}",
            cwd.display()
        )));
    }
    let cwd = without_verbatim(cwd);
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .envs(env)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group so a kill reaches the whole tree.
        cmd.process_group(0);
    }

    let child = cmd.spawn()?;
    #[cfg(unix)]
    let tree = Tree::Group { pgid: child.id() };
    #[cfg(windows)]
    let tree = match crate::ops::windows_job::Job::attach(child.id()) {
        Some(job) => Tree::Job(job),
        None => {
            // No job: killing the tree falls back to the child only; the
            // platform notes name this as the Windows orphan risk, so fail
            // the spawn instead of shipping a spawn that can orphan.
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other(
                "cannot assign the spawned process to a Job Object",
            ));
        }
    };
    Ok(TreeChild { child, tree })
}

/// Spawn `program` with an explicit argv in `cwd`, no shell, pipes for
/// stdio, and a tree identity that dies together (FR-TOOL-5's process
/// semantics without the shell layer).
pub fn spawn_direct(program: &str, args: &[String], cwd: &Path) -> std::io::Result<TreeChild> {
    if !cwd.is_dir() {
        return Err(std::io::Error::other(format!(
            "working directory does not exist: {}",
            cwd.display()
        )));
    }
    let cwd = without_verbatim(cwd);
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group so a kill reaches the whole tree.
        cmd.process_group(0);
    }

    let child = cmd.spawn()?;
    #[cfg(unix)]
    let tree = Tree::Group { pgid: child.id() };
    #[cfg(windows)]
    let tree = match crate::ops::windows_job::Job::attach(child.id()) {
        Some(job) => Tree::Job(job),
        None => {
            // No job: killing the tree falls back to the child only; the
            // platform notes name this as the Windows orphan risk, so fail
            // the spawn instead of shipping a spawn that can orphan.
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other(
                "cannot assign the spawned process to a Job Object",
            ));
        }
    };

    Ok(TreeChild { child, tree })
}

/// Read up to `max` bytes, **blocking** until at least one byte arrives or the
/// stream closes (`None` = EOF).
///
/// This is the ABI's `read-stderr`/`read-stdout` contract: an extension asking for
/// output waits for it. Do not use it to ask "is there anything yet" - on a live
/// child with an empty pipe it waits for data that may never come. See
/// [`read_buffered`] for that.
pub fn read_up_to(reader: &mut impl Read, max: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut buffer = vec![0u8; max.clamp(1, 64 * 1024)];
    let read = reader.read(&mut buffer)?;
    if read == 0 {
        Ok(None)
    } else {
        buffer.truncate(read);
        Ok(Some(buffer))
    }
}

/// Write bytes to a pipe, returning how many were accepted.
pub fn write_all(writer: &mut impl Write, bytes: &[u8]) -> std::io::Result<u64> {
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(bytes.len() as u64)
}

/// Read whatever is **already buffered** on a child's stderr, without waiting
/// (gh #459).
///
/// Returns `Ok(None)` at end of stream - the child closed its end - and
/// `Ok(Some(empty))` when the pipe is open but has nothing right now.
///
/// That distinction is the whole point. [`read_up_to`] cannot express the second
/// case at all: it blocks on a live child with an empty pipe, which is how the
/// error-surfacing path came to hang before it could surface an error. A caller
/// collecting an error tail from a server that is **still alive** needs "it has
/// written nothing yet" as an answer, not as a wait.
///
/// POSIX flips the descriptor to `O_NONBLOCK` for one read and restores the flags
/// before returning; Windows peeks the pipe first. Both are the shapes `pty.rs`
/// already uses for the ConPTY output pipe, where a blocking read deadlocked every
/// consumer.
#[cfg(unix)]
#[allow(unsafe_code)] // documented crate exemption: F_GETFL/F_SETFL on our own pipe
pub fn read_buffered(
    reader: &mut std::process::ChildStderr,
    max: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    use std::os::unix::io::AsRawFd;
    let fd = reader.as_raw_fd();
    // SAFETY: our own descriptor, and F_GETFL/F_SETFL with libc::O_NONBLOCK is the
    // portable POSIX spelling (the constant differs between Linux and macOS, which
    // is why libc decides it rather than a literal here).
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut buffer = vec![0u8; max.clamp(1, 64 * 1024)];
    let read = reader.read(&mut buffer);
    // Restore before doing anything else, so a later blocking caller still blocks
    // even when this read errored.
    // SAFETY: same descriptor, restoring the flags we just read.
    unsafe {
        libc::fcntl(fd, libc::F_SETFL, flags);
    }
    match read {
        Ok(0) => Ok(None),
        Ok(read) => {
            buffer.truncate(read);
            Ok(Some(buffer))
        }
        // Nothing buffered: the pipe is open and empty. Not an error, and not EOF.
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(Some(Vec::new())),
        Err(err) => Err(err),
    }
}

/// Read whatever is **already buffered** on a child's stderr, without waiting
/// (gh #459) - the Windows half.
///
/// `PeekNamedPipe` first, so "no data" is a fact rather than a blocking read; then a
/// read only when the pipe says there is something. `Ok(None)` at end of stream (the
/// child closed its end, which Windows reports as a broken pipe), `Ok(Some(empty))`
/// when the pipe is open and empty. The Unix half above carries the full rationale;
/// both must exist or one platform keeps the hang.
#[cfg(windows)]
#[allow(unsafe_code)] // documented crate exemption: PeekNamedPipe on our own pipe
pub fn read_buffered(
    reader: &mut std::process::ChildStderr,
    max: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    use std::os::windows::io::AsRawHandle;
    let mut available: u32 = 0;
    // SAFETY: the handle is an open pipe end; the out param is a valid u32.
    let ok = unsafe {
        windows_sys::Win32::System::Pipes::PeekNamedPipe(
            reader.as_raw_handle() as *mut _,
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
            // The child closed its end: EOF, not a failure.
            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof => Ok(None),
            _ => Err(err),
        };
    }
    if available == 0 {
        // Open and empty. Windows has no cheap "is EOF" separate from a failed
        // peek, and a closed pipe reports `BrokenPipe` above, so this is "nothing
        // yet" - exactly the case the POSIX half maps `WouldBlock` to.
        return Ok(Some(Vec::new()));
    }
    let mut buffer = vec![0u8; max.clamp(1, 64 * 1024)];
    let read = reader.read(&mut buffer)?;
    if read == 0 {
        return Ok(None);
    }
    buffer.truncate(read);
    Ok(Some(buffer))
}
#[cfg(test)]
mod tests {
    use super::{kill_group_argv, read_buffered, read_up_to, without_verbatim};
    use std::path::Path;

    // Verifies: the group-kill operand is unambiguous on every procps.
    // Without the `--` separator procps `/bin/kill` before 4.0.5 (Ubuntu
    // 24.04's 4.0.2) truncates `-<pgid>` to its first digit, so a pgid
    // starting with `1` becomes `kill(-1, SIGKILL)` - every process of
    // our uid, including the test runner and the CI agent (silent
    // 30-minute cancel, 2026-10-15). The `pgid > 1` guard in
    // `kill_process_group` keeps `-0`/`-1` unreachable.
    #[test]
    fn group_kill_argv_separates_the_negative_pgid() {
        assert_eq!(
            kill_group_argv(12345),
            ["-9".to_string(), "--".to_string(), "-12345".to_string()]
        );
        assert_eq!(
            kill_group_argv(488),
            ["-9".to_string(), "--".to_string(), "-488".to_string()]
        );
    }

    // A regression here breaks every child spawn on Windows at once:
    // cmd.exe reads `\\?\C:\...` as a UNC path and refuses it.
    #[test]
    fn without_verbatim_strips_the_windows_prefix() {
        #[cfg(windows)]
        {
            assert_eq!(
                without_verbatim(Path::new(r"\\?\C:\Users\me\proj")),
                Path::new(r"C:\Users\me\proj")
            );
            assert_eq!(
                without_verbatim(Path::new(r"\\?\UNC\server\share")),
                Path::new(r"\\server\share")
            );
        }
        // A plain path is unchanged on every platform.
        assert_eq!(
            without_verbatim(Path::new("/home/me/proj")),
            Path::new("/home/me/proj")
        );
        assert_eq!(
            without_verbatim(Path::new(r"C:\plain")),
            Path::new(r"C:\plain")
        );
    }

    #[test]
    fn read_up_to_reports_eof_and_bounds_the_read() {
        // EOF is `None`, not an empty chunk.
        assert_eq!(read_up_to(&mut &b""[..], 16).expect("read"), None);
        let data = [1u8; 100];
        let chunk = read_up_to(&mut &data[..], 4096)
            .expect("read")
            .expect("some");
        assert_eq!(chunk.len(), 100);
    }

    // Verifies: gh #459 - the bounded read distinguishes the three states, and
    // "open but empty" is not EOF. `read_up_to` cannot express that third state at
    // all: on a live pipe with no data it waits, which is how the error-surfacing
    // path came to hang.
    #[test]
    fn read_buffered_separates_empty_from_eof() {
        // A live, silent child: the pipe is open and has nothing.
        let mut silent = std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn a silent child");
        let mut stderr = silent.stderr.take().expect("piped stderr");
        match read_buffered(&mut stderr, 64).expect("read") {
            Some(bytes) => assert!(
                bytes.is_empty(),
                "open and empty is `Some(empty)`, not EOF and not a wait: {bytes:?}"
            ),
            None => panic!("a live child's pipe is open, so it is not EOF"),
        }
        let _ = silent.kill();
        let _ = silent.wait();

        // A child that wrote and exited: its bytes, then EOF.
        let mut done = std::process::Command::new("sh")
            .arg("-c")
            .arg("printf 'boom' >&2")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn a writing child");
        let mut stderr = done.stderr.take().expect("piped stderr");
        let _ = done.wait();
        let mut collected = Vec::new();
        while let Some(bytes) = read_buffered(&mut stderr, 64).expect("read") {
            if bytes.is_empty() {
                break;
            }
            collected.extend_from_slice(&bytes);
        }
        assert_eq!(
            String::from_utf8_lossy(&collected),
            "boom",
            "a closed pipe drains to the end, which is what the tail is for"
        );
    }
}
