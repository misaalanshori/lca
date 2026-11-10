//! The transport driver: one Responses streaming completion over the host's
//! `ProviderCap` surface, and the guard that owns its response handle (gh #577).
//!
//! A host handle acquired from a capability and followed by **fallible** calls gets
//! a [`ResponseGuard`] on the line after the call that produced it: a driver
//! constructed later does not own the handle yet, so its `Drop` cannot be the answer
//! - and that is exactly how the leak shipped.

use crate::error::{StreamFailure, failure_for_status, json_error_message};

use super::stream::{ResponsesStream, single_error};

/// The shared owner of the handle (gh #577, lifted to `lca_protocol` in gh #579's
/// follow-up when seven extensions needed the same rule).
///
/// The two things that matter and are not obvious:
///
/// * it is constructed **on the line after `net_request`**, because everything
///   between there and the driver construction is fallible, and a `?` in that
///   window used to return with the handle still open (measured: zero closes);
/// * `close()` consumes it, so a manual close leaves `Drop` nothing to do - which is
///   what makes "closed exactly once" a statement a test can check.
use lca_protocol::ResponseGuard;

/// A pull-based driver over one Responses streaming completion: open
/// sends the request and checks the status (a 401 purges the stored
/// tokens, gh #179's rule, shared by every gateway on this kit);
/// `next_event` yields typed events as the host yields body chunks.
pub struct ResponseStreamDriver<'a> {
    cap: &'a dyn lca_protocol::ProviderCap,
    /// Owns the host handle: `handle()` for reads, closed in `Drop` (gh #577).
    guard: ResponseGuard<'a>,
    /// Raw bytes until a complete frame is available (gh #337): never a
    /// `String`, or a codepoint straddling a chunk boundary would be
    /// corrupted.
    buffer: crate::sse::FrameBuffer,
    mapper: ResponsesStream,
    pending: std::collections::VecDeque<lca_protocol::StreamEvent>,
    finished: bool,
    /// Whether any frame carried a `data:` payload: a 200 body with none is
    /// not an empty answer (gh #359).
    saw_frame: bool,
}

impl<'a> ResponseStreamDriver<'a> {
    /// Send the request and check the status. A 401 calls
    /// `on_unauthorized` (the caller's credential purge, gh #179's
    /// rule) before the status error is built.
    pub fn open(
        cap: &'a dyn lca_protocol::ProviderCap,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        on_unauthorized: &dyn Fn(),
    ) -> Result<ResponseStreamDriver<'a>, StreamFailure> {
        // Gh #577: **the guard goes on the line after the fallible call that
        // produced the handle**, because everything between here and the driver
        // construction can return early - and a host handle table entry outliving a
        // failed request is a leak. Measured before this fix: zero closes on an early
        // error.
        let handle = cap
            .net_request("POST", url, headers, Some(body))
            .map_err(StreamFailure::from)?;
        let guard = ResponseGuard::new(cap, handle);
        let handle = guard.handle();
        let status = cap
            .net_response_status(handle)
            .map_err(StreamFailure::from)?;
        if !(200..300).contains(&status) {
            if status == 401 {
                on_unauthorized();
            }
            let mut detail = Vec::new();
            while let Some(chunk) = cap
                .net_read_body(handle, 64 * 1024)
                .map_err(StreamFailure::from)?
            {
                detail.extend_from_slice(&chunk);
                if detail.len() > 1024 * 1024 {
                    break;
                }
            }
            // The guard closes it here, before the failure is built: the manual
            // close was on this path only, which is why an `Err` above leaked.
            guard.close();
            let text = String::from_utf8_lossy(&detail);
            return Err(failure_for_status(status, &json_error_message(&text)));
        }
        Ok(ResponseStreamDriver {
            cap,
            guard,
            buffer: crate::sse::FrameBuffer::new(),
            mapper: ResponsesStream::new(),
            pending: std::collections::VecDeque::new(),
            finished: false,
            saw_frame: false,
        })
    }

    /// The next typed event.
    pub fn next_event(&mut self) -> Option<Result<lca_protocol::StreamEvent, StreamFailure>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            if self.finished {
                return None;
            }
            match self
                .cap
                .net_read_body(self.guard.handle(), 64 * 1024)
                .map_err(StreamFailure::from)
            {
                Ok(Some(chunk)) => {
                    // Gh #337: bytes until a complete frame exists, then
                    // each *complete* frame decodes on its own.
                    let mut frames: Vec<String> = Vec::new();
                    self.buffer
                        .feed(&chunk, &mut |frame| frames.push(frame.to_string()));
                    for frame in frames {
                        self.handle_frame(&frame);
                    }
                }
                Ok(None) => {
                    self.finished = true;
                    self.finish_stream();
                }
                Err(err) => {
                    self.finished = true;
                    return Some(Err(err));
                }
            }
        }
    }

    /// One complete frame's `data:` payloads into events.
    fn handle_frame(&mut self, frame: &str) {
        for payload in crate::sse::frame_payloads(frame) {
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            self.saw_frame = true;
            if let Some(event) = single_error(&value) {
                self.pending.push_back(event);
                continue;
            }
            self.pending.extend(self.mapper.feed(&value));
        }
    }

    /// End of stream (gh #359).
    ///
    /// A truncated tail is the last thing a body can carry, so it is parsed
    /// rather than dropped: a tail with a `data:` line is a frame that lost
    /// its terminator, and one without surfaces as a transport error. Then,
    /// if the body never carried a frame at all, that is reported as a
    /// transport failure - an empty assistant message is indistinguishable
    /// from a model that said nothing, and it must never mean that.
    fn finish_stream(&mut self) {
        if let Some(tail) = self.buffer.take_tail() {
            let parseable = tail.lines().any(|line| {
                line.trim_end_matches('\r')
                    .trim_start()
                    .starts_with("data:")
            });
            self.handle_frame(&tail);
            // A tail with no `data:` line cannot have been part of a frame,
            // so it is a truncated frame whether or not earlier frames
            // arrived; report it either way (gh #359: no received byte
            // disappears silently).
            if !parseable && !tail.trim().is_empty() {
                self.pending
                    .push_back(crate::sse::truncated_tail_event(&tail));
            }
        }
        if !self.saw_frame {
            self.pending.push_back(lca_protocol::StreamEvent::Error {
                message: crate::sse::no_frames_failure("").message,
                retryable: false,
            });
        }
    }
}
