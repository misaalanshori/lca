//! Direct program execution (no shell) with whole-tree lifetime, shared by
//! the built-in shell tool's platform code and the `process` capability.
//!
//! `TreeChild` owns a spawned program and whatever it forked: POSIX kills
//! the process group the spawn created, Windows kills the Job Object the
//! spawn joined (docs/platform-notes.md).

#![deny(unsafe_code)]

use std::io::{Read, Write};
use std::path::Path;

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
            let _ = std::process::Command::new("/bin/kill")
                .args(["-9", &format!("-{pgid}")])
                .status();
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
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
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

/// Read up to `max` bytes, blocking until at least one byte arrives or the
/// stream closes (`None` = EOF).
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
