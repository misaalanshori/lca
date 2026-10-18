//! Byte-level SSE framing, shared by the two decoders on this kit
//! (gh #337, gh #359).
//!
//! Both the Chat Completions decoder and the Responses driver used to do
//! `buffer.push_str(&String::from_utf8_lossy(chunk))` per network chunk.
//! HTTP byte streams do not align with UTF-8 character boundaries, so a
//! codepoint straddling two chunks became two replacement characters and the
//! text was permanently corrupted: a 3-byte CJK character split after byte 1
//! arrived as `U+FFFD U+FFFD`.
//!
//! The fix is to keep the buffer as **bytes** and only decode *complete
//! frames*. A codepoint may straddle a chunk boundary inside the buffer -
//! that is fine, nothing looks at it yet - but a decoded frame is a whole
//! `\n\n`-terminated region, which is valid UTF-8 by construction.
//!
//! One helper for both decoders: the framing rule is identical, and having
//! two copies is how the twin drifted apart in the first place.

use lca_protocol::StreamEvent;

/// The frame terminator both decoders split on.
pub const FRAME_SEPARATOR: &[u8] = b"\n\n";

/// A byte buffer that yields complete `\n\n`-terminated frames.
///
/// Feed raw chunks; each complete frame comes back as a decoded `str` slice.
/// The trailing partial frame stays buffered, undecoded.
#[derive(Default, Debug)]
pub struct FrameBuffer {
    buffer: Vec<u8>,
}

impl FrameBuffer {
    /// An empty buffer.
    pub fn new() -> Self {
        FrameBuffer::default()
    }

    /// Append raw bytes and hand every complete frame to `on_frame`.
    ///
    /// The frame text is valid UTF-8: a complete SSE frame either parses as
    /// UTF-8 or the transport delivered something that was never SSE, in
    /// which case the bytes are passed lossily and the caller's JSON parse
    /// rejects them (the same treatment a malformed frame gets today).
    pub fn feed(&mut self, chunk: &[u8], on_frame: &mut dyn FnMut(&str)) {
        self.buffer.extend_from_slice(chunk);
        while let Some(end) = find(self.buffer.as_slice(), FRAME_SEPARATOR) {
            let frame: Vec<u8> = self.buffer.drain(..end + FRAME_SEPARATOR.len()).collect();
            on_frame(&decode(&frame));
        }
    }

    /// Take whatever is left at end of stream.
    ///
    /// A truncated tail (a transfer cut before its final blank line, a proxy
    /// that dropped the terminator) is the last thing a body can carry, so
    /// dropping it silently loses received bytes. `None` when the buffer is
    /// empty.
    pub fn take_tail(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        let tail = std::mem::take(&mut self.buffer);
        Some(decode(&tail))
    }

    /// Whether anything is still buffered (a tail that has not been taken).
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// Decode one complete frame. Lossy on purpose: a frame that is not valid
/// UTF-8 is not a frame this kit can parse, and the caller's JSON step
/// rejects it with its own error - losing one malformed frame must not take
/// down the stream, which is the behavior every other malformed-frame path
/// here already has.
fn decode(frame: &[u8]) -> String {
    String::from_utf8_lossy(frame).into_owned()
}

/// The index of `needle` in `haystack`, byte-wise.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The `data:` payloads of one frame, in order, trimmed. Blank lines and
/// non-`data:` lines (comments, `event:`, keep-alives) are skipped.
pub fn frame_payloads(frame: &str) -> impl Iterator<Item = &str> {
    frame.lines().filter_map(|line| {
        let line = line.trim_end_matches('\r');
        line.strip_prefix("data:").map(str::trim)
    })
}

/// The transport failure for a body that carried no SSE frame at all.
///
/// A 200 with a body that never spoke SSE is not "the model said nothing" -
/// it is a body that was not a stream (an HTML error page behind a 200, a
/// proxy page, an empty body). Reporting it as an empty answer is
/// indistinguishable from a silent model, so it is reported as what it is.
pub fn no_frames_failure(detail: &str) -> crate::error::StreamFailure {
    crate::error::StreamFailure {
        message: format!("the provider's response carried no server-sent event frames{detail}"),
        class: "transport",
        retryable: false,
    }
}

/// The transport event for a trailing buffer that could not be parsed at
/// end of stream (gh #359): received bytes are never silently discarded.
pub fn truncated_tail_event(tail: &str) -> StreamEvent {
    let preview: String = tail.chars().take(120).collect();
    StreamEvent::Error {
        message: format!(
            "the provider's stream ended mid-frame; the trailing {len} bytes are incomplete \
             and were not parsed: {preview:?}",
            len = tail.len()
        ),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #337 - a 4-byte codepoint split across two chunks inside
    // one frame decodes intact, because only the complete frame is decoded.
    #[test]
    fn a_codepoint_split_across_chunks_decodes_from_the_complete_frame() {
        let mut buffer = FrameBuffer::new();
        let mut frames = Vec::new();
        // "😀" is F0 9F 98 80; split after two bytes.
        buffer.feed(b"data: \xF0\x9F", &mut |frame| {
            frames.push(frame.to_string())
        });
        assert!(frames.is_empty(), "the frame is not complete yet");
        buffer.feed(b"\x98\x80\n\n", &mut |frame| frames.push(frame.to_string()));
        assert_eq!(frames.len(), 1);
        assert!(frames[0].contains("😀"), "{:?}", frames[0]);
        assert!(!frames[0].contains('\u{FFFD}'), "{:?}", frames[0]);
    }

    // Verifies: gh #359 - the tail is available rather than dropped.
    #[test]
    fn the_tail_is_returned_and_empties_the_buffer() {
        let mut buffer = FrameBuffer::new();
        buffer.feed(b"data: {\"a\":1}", &mut |_| {});
        assert!(!buffer.is_empty());
        assert_eq!(buffer.take_tail().as_deref(), Some("data: {\"a\":1}"));
        assert!(buffer.is_empty());
        assert_eq!(buffer.take_tail(), None);
    }

    // Verifies: framing is byte-wise, so a `\n\n` inside a multi-byte
    // codepoint's bytes cannot happen and a real one still splits.
    #[test]
    fn frames_split_on_byte_terminators_across_chunk_boundaries() {
        let mut buffer = FrameBuffer::new();
        let mut count = 0;
        for chunk in [b"data: a\n".as_slice(), b"\ndata: b\n\n".as_slice()] {
            buffer.feed(chunk, &mut |_| count += 1);
        }
        assert_eq!(count, 2, "the terminator itself straddled the chunks");
    }
}
