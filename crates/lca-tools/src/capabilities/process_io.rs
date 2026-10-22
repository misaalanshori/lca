//! The `process` capability's stream I/O: stdout, stderr, stdin (gh #459).
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

use super::{CapabilityError, HandleEntry, lock};

impl super::Capabilities {
    /// Read up to `max` bytes of the child's stdout; `None` at EOF.
    pub fn process_read_stdout(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut table = lock(&self.handles);
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stdout, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let reader = match stdout.as_mut() {
            Some(reader) => reader,
            None => {
                let Some(reader) = child.stdout() else {
                    return Err(CapabilityError::Invalid(format!(
                        "handle {handle} has no stdout"
                    )));
                };
                stdout.insert(reader)
            }
        };
        Ok(crate::process::read_up_to(reader, max)?)
    }

    /// Read up to `max` bytes of the child's stderr; `None` at EOF.
    pub fn process_read_stderr(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut table = lock(&self.handles);
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stderr, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let reader = match stderr.as_mut() {
            Some(reader) => reader,
            None => {
                let Some(reader) = child.stderr() else {
                    return Err(CapabilityError::Invalid(format!(
                        "handle {handle} has no stderr"
                    )));
                };
                stderr.insert(reader)
            }
        };
        Ok(crate::process::read_up_to(reader, max)?)
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
        let mut table = lock(&self.handles);
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stderr, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let reader = match stderr.as_mut() {
            Some(reader) => reader,
            None => {
                let Some(reader) = child.stderr() else {
                    return Err(CapabilityError::Invalid(format!(
                        "handle {handle} has no stderr"
                    )));
                };
                stderr.insert(reader)
            }
        };
        let _ = child;
        Ok(crate::process::read_buffered(reader, max)?)
    }

    /// Write to the child's stdin.
    pub fn process_write_stdin(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
        let mut table = lock(&self.handles);
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stdin, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let writer = match stdin.as_mut() {
            Some(writer) => writer,
            None => {
                let Some(writer) = child.stdin() else {
                    return Err(CapabilityError::Invalid(format!(
                        "handle {handle} has no stdin"
                    )));
                };
                stdin.insert(writer)
            }
        };
        Ok(crate::process::write_all(writer, bytes)?)
    }
}
