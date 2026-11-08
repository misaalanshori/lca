//! The `process` capability's stream I/O: stdout, stderr, stdin (gh #459, gh #569).
//!
//! Split from `mod.rs` when the file crossed gate 11's 1,200-line ceiling. Two
//! stderr readers live here, and the difference is load-bearing:
//!
//! - [`Capabilities::process_read_stderr`] is the **ABI's** `read-stderr`: an
//!   extension asking for output waits for it. Blocking is the contract.
//! - [`Capabilities::process_read_stderr_buffered`] answers "is there anything
//!   yet", so a caller collecting an error tail from a live child is not stuck on a
//!   pipe that has never been written to - the hang gh #459 fixed.
//!
//! Keeping them in one file is deliberate: a future tidy-up that "merges the two"
//! would delete the fix, and a reader who sees them side by side sees why both
//! exist.
//!
//! # The table is never held across a blocking pipe operation (gh #569)
//!
//! Every stream method here does the same three steps: **take the pipe out of the
//! handle table under the lock, do the read or write with no lock held, then put
//! the pipe back** (or, if the handle is gone, leave it absent). The middle step is
//! the point. A stdout read of a silent child blocks indefinitely, and the handle
//! table is what [`Capabilities::process_kill`] needs - so holding it across the
//! read made the reader wait for output while the killer waited for the lock, with
//! the child in between waiting for stdin. Nothing moved, and that is exactly what
//! cancellation does: kill the server (gh #569's deadlock, measured before the fix:
//! `process_kill` had not returned after three seconds with a `cat` mid-read).
//!
//! The table stays the **single owner** of a pipe - there is no secondary map or
//! per-stream mutex. What crosses the lock is a lookup result, not a second store.
//! Putting it back uses the same slot it came from, and a handle removed meanwhile
//! (a concurrent kill or wait, both legal) simply gets no pipe back: the bytes are
//! dropped with it rather than resurrected into a handle nobody owns.
//!
//! While a pipe is out, a second caller asking for the same stream is **refused
//! by name** rather than left to read someone else's half. The old code serialised
//! them only by accident, through the very lock it should not have been holding;
//! with the pipe out that accident is gone, so the refusal is explicit. Two readers
//! of one pipe would each get part of the stream, which is worse than a clear
//! "another call holds this handle's stdout" - and nothing in the tree does it
//! (`lca-mcp` reads one pipe from one thread, and the ABI's `read-stdout` is a
//! single import).

use super::{CapabilityError, HandleEntry, lock};
use crate::process::TreeChild;
use std::process::{ChildStderr, ChildStdin, ChildStdout};

/// Where one pipe lives in a process entry, so take and return use the same door.
type Slot<T> = fn(&mut HandleEntry) -> Option<&mut Option<T>>;

fn stdout_slot(entry: &mut HandleEntry) -> Option<&mut Option<ChildStdout>> {
    match entry {
        HandleEntry::Process { stdout, .. } => Some(stdout),
        _ => None,
    }
}

fn stderr_slot(entry: &mut HandleEntry) -> Option<&mut Option<ChildStderr>> {
    match entry {
        HandleEntry::Process { stderr, .. } => Some(stderr),
        _ => None,
    }
}

fn stdin_slot(entry: &mut HandleEntry) -> Option<&mut Option<ChildStdin>> {
    match entry {
        HandleEntry::Process { stdin, .. } => Some(stdin),
        _ => None,
    }
}

impl super::Capabilities {
    /// Read up to `max` bytes of the child's stdout; `None` at EOF.
    pub fn process_read_stdout(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut reader = self.take_pipe(handle, "stdout", stdout_slot, |child| child.stdout())?;
        let read = crate::process::read_up_to(&mut reader, max);
        self.return_pipe(handle, stdout_slot, reader);
        Ok(read?)
    }

    /// Read up to `max` bytes of the child's stderr; `None` at EOF.
    pub fn process_read_stderr(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut reader = self.take_pipe(handle, "stderr", stderr_slot, |child| child.stderr())?;
        let read = crate::process::read_up_to(&mut reader, max);
        self.return_pipe(handle, stderr_slot, reader);
        Ok(read?)
    }

    /// Whether the child has exited, without reaping it.
    ///
    /// The tail reader needs this to choose its read: a live child may never close
    /// its pipe, so only what is buffered *now* is available; a child that has
    /// **exited** will close it, so draining to EOF is safe and complete. Without
    /// the distinction, a dying server's last write can be missed simply because the
    /// read ran before the kernel delivered it (measured: 2 of 5 runs through
    /// `lca mcp list`).
    pub fn process_has_exited(&self, handle: u32) -> Result<bool, CapabilityError> {
        let mut table = lock(&self.handles);
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        Ok(child.try_wait()?.is_some())
    }

    /// Read whatever the child has **already** written to stderr, without waiting
    /// (gh #459).
    ///
    /// Distinct from [`Self::process_read_stderr`], which is the ABI's blocking
    /// `read-stderr` - an extension asking for output waits for it, and that must
    /// keep waiting. This one is for a caller asking "is there anything yet": a
    /// live server with an empty pipe answers `Some(empty)` instead of hanging,
    /// and a closed pipe answers `None`.
    pub fn process_read_stderr_buffered(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut reader = self.take_pipe(handle, "stderr", stderr_slot, |child| child.stderr())?;
        // Not blocking on a live child, but a read is still a syscall: the same
        // discipline applies, so a concurrent `process_kill` never waits for one.
        let read = crate::process::read_buffered(&mut reader, max);
        self.return_pipe(handle, stderr_slot, reader);
        Ok(read?)
    }

    /// Write to the child's stdin.
    pub fn process_write_stdin(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
        let mut writer = self.take_pipe(handle, "stdin", stdin_slot, |child| child.stdin())?;
        // A child that is not reading its stdin fills the pipe buffer and this
        // write blocks; gh #569's second shape, and the same lock discipline covers
        // it, so a concurrent kill still reaches the child.
        let written = crate::process::write_all(&mut writer, bytes);
        self.return_pipe(handle, stdin_slot, writer);
        Ok(written?)
    }

    /// Take one of a child's pipes out of the handle table, leaving the table
    /// unlocked (gh #569).
    ///
    /// The returned pipe is the caller's for the duration of the I/O and **must**
    /// come back through [`Self::return_pipe`]. Order matters: the type check runs
    /// first, so a handle of another kind is refused without being touched, exactly
    /// as gh #318 requires for the entry kinds that `wait`/`kill` also remove.
    fn take_pipe<T>(
        &self,
        handle: u32,
        name: &str,
        slot: Slot<T>,
        fill: impl FnOnce(&mut TreeChild) -> Option<T>,
    ) -> Result<T, CapabilityError> {
        let mut table = lock(&self.handles);
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        if !matches!(entry, HandleEntry::Process { .. }) {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        }
        if let Some(pipe) = slot(entry).and_then(Option::take) {
            return Ok(pipe);
        }
        // Nothing stored yet: the spawn left the pipe on the child, and taking it
        // now is the same lazy move as before - just inside the lock, because
        // `child.stdin()`/`stdout()` needs `&mut`.
        let HandleEntry::Process { child, .. } = entry else {
            // Unreachable: the check above ran under the same lock.
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        fill(child).ok_or_else(|| {
            CapabilityError::Invalid(format!(
                "handle {handle} has no {name} to use (another call holds this \
                 handle's {name})"
            ))
        })
    }

    /// Put a pipe back where [`Self::take_pipe`] found it, without blocking.
    ///
    /// A handle that is gone (a concurrent `process_kill` or a `process_wait` that
    /// reaped it) is the legal case, not an error: the pipe is dropped with the
    /// bytes it carries, because nothing owns them any more.
    fn return_pipe<T>(&self, handle: u32, slot: Slot<T>, pipe: T) {
        let mut table = lock(&self.handles);
        if let Some(slot) = table.entries.get_mut(&handle).and_then(slot) {
            *slot = Some(pipe);
        }
    }
}
