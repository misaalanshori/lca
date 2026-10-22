//! The stderr tail a failed stdio handshake carries (gh #418 Phase C#C1, gh #459).
//!
//! A server that fails the handshake usually explained itself on stderr - "config is
//! wrong", a Python traceback, a missing token - so reporting only "the handshake
//! failed" throws that away. This module keeps that tail.
//!
//! # gh #459: the read must not wait
//!
//! The first version read through the **blocking** `process_read_stderr` and its doc
//! promised a still-running child "gives us its partial output" - true only once it
//! had *written* something. With zero bytes written the read blocked, so the path
//! that exists to surface an error hung before it could surface one, and the server
//! sat on `Connecting` until the budget expired and then still said so.
//!
//! The read is now bounded: a live child with nothing buffered returns immediately,
//! a closed pipe still drains to the end, and a partial line still comes back. The
//! ABI's own `read-stderr` keeps its blocking semantics, because an extension asking
//! for output must wait for it - the bounded read is a second path, not a quiet
//! change under the first.

use std::sync::Arc;

/// The last ≤2,000 bytes the child wrote to stderr, as a ` stderr: ...`
/// suffix, or an empty string when it wrote nothing.
///
/// gh #418 Phase C#C1: a server that dies during the handshake usually
/// explained itself on stderr ("config is wrong", a Python traceback, a
/// missing token). Reporting only "the handshake failed" throws that away,
/// so `/mcp` cannot say why. Only read on the failure path, and only
/// non-blocking (see [`read_stderr_tail`]), because a live child's pipe may
/// never close.
pub(crate) fn stderr_tail(caps: &Arc<lca_tools::Capabilities>, handle: u32) -> String {
    let tail = read_stderr_tail(caps, handle);
    if tail.is_empty() {
        return String::new();
    }
    format!(" stderr: {tail}")
}

/// How much of the child's stderr is kept (pi keeps 64 KiB; the tail that
/// explains a crash is at the end, and 2 KiB is what `/mcp` can show).
const STDERR_TAIL_BYTES: usize = 2000;

/// Read what the child has **already** written to stderr, without waiting.
///
/// Reads through `process_read_stderr_buffered`, which answers `Some(empty)` for a
/// live child with nothing written yet and `None` at EOF. So the loop drains a
/// closed pipe to the end (the usual crash shape, which is what the tail is for)
/// and returns immediately for a live one.
///
/// The previous version called the blocking `process_read_stderr`, and its doc
/// claimed a still-running child "gives us its partial output" - true only once it
/// has *written* something. With zero bytes written the read blocked, so the path
/// that exists to surface an error hung before it could surface one (gh #459).
///
/// Never called on success, so it cannot steal a live server's stderr.
pub(crate) fn read_stderr_tail(caps: &Arc<lca_tools::Capabilities>, handle: u32) -> String {
    // Gh #459: a child that has **exited** will close its pipe, so draining to EOF
    // collects everything it wrote - no lost last words, which a purely-buffered read
    // missed whenever it ran before the kernel delivered the write (measured: 2 of 5
    // runs). A live child may never close its pipe, so that case stays bounded and
    // takes only what is already there.
    let exited = caps.process_has_exited(handle).unwrap_or(false);
    let mut collected = Vec::new();
    loop {
        let chunk = if exited {
            // EOF is guaranteed: the child is gone, so this returns rather than
            // waiting for a write that will never come.
            caps.process_read_stderr(handle, STDERR_TAIL_BYTES)
        } else {
            caps.process_read_stderr_buffered(handle, STDERR_TAIL_BYTES)
        };
        match chunk {
            Ok(Some(chunk)) if !chunk.is_empty() => {
                collected.extend_from_slice(&chunk);
                // Keep only the tail, so a chatty server cannot grow this.
                if collected.len() > STDERR_TAIL_BYTES {
                    let keep_from = collected.len() - STDERR_TAIL_BYTES;
                    let keep_from = String::from_utf8_lossy(&collected[keep_from..])
                        .char_indices()
                        .next()
                        .map(|(index, _)| index + keep_from)
                        .unwrap_or(keep_from);
                    collected.drain(..keep_from);
                }
            }
            _ => break,
        }
    }
    let text = String::from_utf8_lossy(&collected).trim().to_string();
    if text.is_empty() {
        return String::new();
    }
    // One line, so the notice stays one line; a traceback keeps its shape
    // in the log but not in the status row.
    text.replace(['\r', '\n'], " | ")
}
