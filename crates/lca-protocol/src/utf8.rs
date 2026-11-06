//! Streaming UTF-8 decoding: hold a split codepoint, never invent a replacement
//! character for one that is merely late (gh #405 gap 4).
//!
//! # The defect this closes
//!
//! A pipe read is a *byte* boundary, not a character boundary. Every caller that
//! did `String::from_utf8_lossy(&chunk)` per read therefore turned a codepoint
//! straddling two reads into one or two `U+FFFD`: measured on the shipped tool-output
//! path, `"output: 日本語の日本語の日本語の日本語"` delivered in 8-byte chunks
//! produced **10 replacement characters** and the bash card painted them. The
//! character was never invalid — it was *late*, which is a different thing, and the
//! information needed to decode it still exists in the bytes that already arrived.
//!
//! # Incomplete is not invalid
//!
//! That distinction is the whole contract, and it is why this cannot be a lossy
//! decode with a bigger buffer:
//!
//! - **Incomplete so far** — the trailing bytes are a valid prefix of a codepoint
//!   (a lead byte and some of its continuations). They are *held* until the next
//!   chunk completes them. [`Utf8Stream::push`] never emits anything for them.
//! - **Invalid** — a byte that cannot begin any UTF-8 sequence, or a continuation
//!   that contradicts its lead. That is a real transport fault, so it is replaced
//!   **once, visibly**, and the decoder continues after it. Silently dropping it
//!   would hide a provider or a shell emitting something that is not text.
//! - **Incomplete at end of stream** — bytes still held when the stream closes can
//!   never be decoded, so [`Utf8Stream::finish`] replaces them once. Held forever
//!   they would be lost silently; that is the `TextDecoder` behaviour too.
//!
//! # Why it lives here
//!
//! `lca-protocol` performs no I/O (ADR-0002): this is a pure function from bytes to
//! text, which is why every crate that reads a byte stream — core's tool output,
//! the TUI's stdin reassembler — can reach it without a new dependency or a second
//! copy of the decoder.

/// The replacement character, spelled once (U+FFFD).
pub const REPLACEMENT: char = '\u{FFFD}';

/// A UTF-8 decoder that carries a partial codepoint across chunk boundaries.
#[derive(Debug, Default, Clone)]
pub struct Utf8Stream {
    /// Bytes of a codepoint whose continuations have not all arrived (at most 3).
    pending: Vec<u8>,
}

impl Utf8Stream {
    /// A decoder with nothing pending.
    pub fn new() -> Utf8Stream {
        Utf8Stream::default()
    }

    /// Whether a partial codepoint is being held.
    ///
    /// Exposed for the caller that has to decide whether "no text this read" means
    /// "nothing arrived" or "a character is mid-flight" - a distinction a UI that
    /// shows a spinner cares about.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Append bytes and return every character they complete.
    ///
    /// Text returned is always valid UTF-8. Bytes that form a valid *prefix* of a
    /// codepoint are held (they are not lost, and they are not replaced); a byte
    /// sequence that is genuinely not UTF-8 contributes one [`REPLACEMENT`] per
    /// invalid sequence, exactly as `str::from_utf8` reports it.
    pub fn push(&mut self, chunk: &[u8]) -> String {
        if chunk.is_empty() {
            return String::new();
        }
        // Join with anything held: a codepoint can straddle *several* reads, so the
        // held prefix is part of this chunk, not a separate stage.
        let mut input: Vec<u8> = std::mem::take(&mut self.pending);
        input.extend_from_slice(chunk);
        self.decode(&input)
    }

    /// Close the stream, returning whatever the held bytes could not become.
    ///
    /// A held prefix at end of stream is a truncated character - the transfer was
    /// cut, or the source is not UTF-8 at all. It is reported **once**, visibly,
    /// rather than dropped: an invisible loss of the last character of an answer is
    /// the class of defect this module exists to end.
    pub fn finish(&mut self) -> String {
        if self.pending.is_empty() {
            return String::new();
        }
        self.pending.clear();
        REPLACEMENT.to_string()
    }

    /// Decode one joined buffer, holding the trailing incomplete prefix.
    ///
    /// `str::from_utf8` is the authority here, so there is no hand-rolled byte-class
    /// table to drift from the standard: `valid_up_to` is how much text there is, and
    /// `error_len` says whether the rest is *invalid* (a known bad sequence length) or
    /// merely *incomplete* (`None`).
    fn decode(&mut self, input: &[u8]) -> String {
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    return out;
                }
                Err(err) => {
                    let valid = err.valid_up_to();
                    // SAFETY-equivalent: `valid_up_to` is documented to be a UTF-8
                    // boundary of the input, so this cannot fail. Written with
                    // `get` so a future upstream change degrades to "nothing decoded
                    // this round" instead of a panic in a render path.
                    if let Some(text) = rest.get(..valid) {
                        // Cannot fail either: it is the validated prefix.
                        out.push_str(&String::from_utf8_lossy(text));
                    }
                    match err.error_len() {
                        // A definite invalid sequence: replace it once and continue
                        // after it, so one bad byte cannot swallow the rest.
                        Some(bad) => {
                            out.push(REPLACEMENT);
                            let skip = valid.saturating_add(bad);
                            if skip >= rest.len() {
                                return out;
                            }
                            rest = &rest[skip..];
                        }
                        // Incomplete at the end: hold it for the next chunk (or for
                        // `finish`).
                        None => {
                            self.pending = rest[valid..].to_vec();
                            return out;
                        }
                    }
                }
            }
        }
    }
}
