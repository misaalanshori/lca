//! Stdin sequence reassembly, ported from pi's
//! `packages/tui/src/stdin-buffer.ts`
//! (`pi-tui-re/src_re/tui-engine/stdin-buffer.md`).
//!
//! stdin chunks split escape sequences; a partial SGR-mouse read as a
//! keypress is a real bug class, so bytes accumulate until a sequence is
//! provably complete.
//!
//! Attribution: the completeness rules derive from OpenTUI
//! (<https://github.com/anomalyco/opentui>), MIT, Copyright (c) 2025 opentui.
//!
//! Difference from pi: no timers here. [`StdinBuffer::process`] returns the
//! events it can emit synchronously and leaves any incomplete remainder in
//! the buffer; the caller's read loop uses [`StdinBuffer::pending_timeout_ms`]
//! and [`StdinBuffer::flush`] to reproduce pi's dual 50/10 ms timeout.

const ESC: char = '\x1b';
/// Default maximum wait for an incomplete (non-lone-ESC) sequence.
pub const DEFAULT_SEQUENCE_TIMEOUT_MS: u64 = 50;
/// Default maximum wait after a lone ESC (Alt+key reassembly window).
pub const DEFAULT_ESCAPE_TIMEOUT_MS: u64 = 10;
const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

/// One event emitted by [`StdinBuffer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdinEvent {
    /// One complete input sequence (or one non-escape character).
    Data(String),
    /// One bracketed paste's content, markers stripped.
    Paste(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Completeness {
    Complete,
    Incomplete,
    NotEscape,
}

fn is_complete_sequence(data: &str) -> Completeness {
    if !data.starts_with(ESC) {
        return Completeness::NotEscape;
    }
    if data.chars().count() == 1 {
        return Completeness::Incomplete;
    }
    let after: &str = &data[1..]; // ESC is 1 byte
    if after.starts_with('[') {
        // Old-style X10 mouse: ESC [ M + 3 bytes = 6 total.
        if after.starts_with("[M") {
            return if data.len() >= 6 {
                Completeness::Complete
            } else {
                Completeness::Incomplete
            };
        }
        return is_complete_csi(data);
    }
    if after.starts_with(']') {
        return if data.ends_with("\x1b\\") || data.ends_with('\x07') {
            Completeness::Complete
        } else {
            Completeness::Incomplete
        };
    }
    if after.starts_with('P') {
        return if data.ends_with("\x1b\\") {
            Completeness::Complete
        } else {
            Completeness::Incomplete
        };
    }
    if after.starts_with('_') {
        return if data.ends_with("\x1b\\") {
            Completeness::Complete
        } else {
            Completeness::Incomplete
        };
    }
    if after.starts_with('O') {
        // SS3: ESC O + one char.
        return if after.chars().count() >= 2 {
            Completeness::Complete
        } else {
            Completeness::Incomplete
        };
    }
    // Meta: ESC + a single character.
    if after.chars().count() == 1 {
        return Completeness::Complete;
    }
    Completeness::Complete
}

fn is_complete_csi(data: &str) -> Completeness {
    if !data.starts_with("\x1b[") {
        return Completeness::Complete;
    }
    if data.len() < 3 {
        return Completeness::Incomplete;
    }
    let payload = &data[2..];
    let Some(last) = payload.chars().last() else {
        return Completeness::Incomplete;
    };
    let code = last as u32;
    if (0x40..=0x7e).contains(&code) {
        if let Some(rest) = payload.strip_prefix('<') {
            // SGR mouse: <digits;digits;digits[Mm]
            if rest.ends_with('M') || rest.ends_with('m') {
                let inner = &rest[..rest.len() - 1];
                let parts: Vec<&str> = inner.split(';').collect();
                if parts.len() == 3
                    && parts
                        .iter()
                        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                {
                    return Completeness::Complete;
                }
            }
            return Completeness::Incomplete;
        }
        return Completeness::Complete;
    }
    Completeness::Incomplete
}

fn parse_unmodified_kitty_printable_codepoint(sequence: &str) -> Option<u32> {
    let bytes = sequence.as_bytes();
    if !bytes.starts_with(b"\x1b[") || bytes.last() != Some(&b'u') {
        return None;
    }
    let body = &bytes[2..bytes.len() - 1];
    let mut i = 0;
    let start = i;
    while i < body.len() && body[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return None;
    }
    let codepoint: u32 = std::str::from_utf8(&body[start..i]).ok()?.parse().ok()?;
    // Only the plain form `ESC [ cp u`, no colons/semicolons.
    if i != body.len() {
        return None;
    }
    if codepoint >= 32 {
        Some(codepoint)
    } else {
        None
    }
}

/// Split `buffer` into complete sequences plus an incomplete remainder.
fn extract_complete_sequences(buffer: &str) -> (Vec<String>, String) {
    let mut sequences = Vec::new();
    let bytes = buffer.as_bytes();
    let mut pos = 0usize;

    while pos < bytes.len() {
        let remaining = &buffer[pos..];
        if remaining.starts_with(ESC) {
            let mut seq_end = 1usize;
            loop {
                if seq_end > remaining.len() {
                    return (sequences, remaining.to_string());
                }
                if !remaining.is_char_boundary(seq_end) {
                    seq_end += 1;
                    continue;
                }
                let candidate = &remaining[..seq_end];
                match is_complete_sequence(candidate) {
                    Completeness::Complete => {
                        // WezTerm sends Escape press as raw `\x1b` and the
                        // release as a Kitty CSI-u, concatenated. The generic
                        // ESC+char meta rule would swallow `\x1b\x1b` and type
                        // `[27;…u` as text. Emit only the first ESC and
                        // restart at the second.
                        if candidate == "\x1b\x1b" {
                            let next = remaining[seq_end..].chars().next();
                            if matches!(next, Some('[' | ']' | 'O' | 'P' | '_')) {
                                sequences.push(ESC.to_string());
                                pos += 1;
                                break;
                            }
                        }
                        sequences.push(candidate.to_string());
                        pos += seq_end;
                        break;
                    }
                    Completeness::Incomplete => {
                        seq_end += 1;
                    }
                    Completeness::NotEscape => {
                        sequences.push(candidate.to_string());
                        pos += seq_end;
                        break;
                    }
                }
            }
        } else {
            let Some(ch) = remaining.chars().next() else {
                break;
            };
            sequences.push(ch.to_string());
            pos += ch.len_utf8();
        }
    }
    (sequences, String::new())
}

/// Buffers stdin input and emits complete sequences. See the module docs.
#[derive(Debug, Clone)]
pub struct StdinBuffer {
    buffer: String,
    sequence_timeout_ms: u64,
    escape_timeout_ms: u64,
    paste_mode: bool,
    paste_buffer: String,
    pending_kitty_printable_codepoint: Option<u32>,
}

impl Default for StdinBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl StdinBuffer {
    /// A buffer with pi's default 50 ms / 10 ms timeouts.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            sequence_timeout_ms: DEFAULT_SEQUENCE_TIMEOUT_MS,
            escape_timeout_ms: DEFAULT_ESCAPE_TIMEOUT_MS,
            paste_mode: false,
            paste_buffer: String::new(),
            pending_kitty_printable_codepoint: None,
        }
    }

    /// A buffer with custom timeouts (the terminal sets the escape timeout
    /// from [`crate::engine::terminal::resolve_escape_timeout_ms`]).
    pub fn with_timeouts(sequence_timeout_ms: u64, escape_timeout_ms: u64) -> Self {
        Self {
            sequence_timeout_ms,
            escape_timeout_ms,
            ..Self::new()
        }
    }

    /// Feed a UTF-8 string; returns every event it completes.
    pub fn process(&mut self, data: &str) -> Vec<StdinEvent> {
        if data.is_empty() && self.buffer.is_empty() {
            return vec![StdinEvent::Data(String::new())];
        }
        self.buffer.push_str(data);
        self.drain()
    }

    /// Feed raw bytes, applying pi's legacy single-high-byte rule:
    /// a lone byte > 127 becomes `ESC + (byte - 128)`.
    pub fn process_bytes(&mut self, data: &[u8]) -> Vec<StdinEvent> {
        if data.len() == 1 && data[0] > 127 {
            let mut s = String::from(ESC);
            s.push((data[0] - 128) as char);
            return self.process(&s);
        }
        match std::str::from_utf8(data) {
            Ok(s) => self.process(s),
            Err(_) => {
                // Lossy fallback: never drop bytes.
                let s = String::from_utf8_lossy(data).into_owned();
                self.process(&s)
            }
        }
    }

    fn drain(&mut self) -> Vec<StdinEvent> {
        let mut events = Vec::new();
        if self.paste_mode {
            self.paste_buffer
                .push_str(&std::mem::take(&mut self.buffer));
            if let Some(end) = self.paste_buffer.find(BRACKETED_PASTE_END) {
                let content = self.paste_buffer[..end].to_string();
                let remaining = self.paste_buffer[end + BRACKETED_PASTE_END.len()..].to_string();
                self.paste_mode = false;
                self.paste_buffer.clear();
                self.pending_kitty_printable_codepoint = None;
                events.push(StdinEvent::Paste(content));
                if !remaining.is_empty() {
                    events.extend(self.process(&remaining));
                }
            }
            return events;
        }

        if let Some(start) = self.buffer.find(BRACKETED_PASTE_START) {
            if start > 0 {
                let before = self.buffer[..start].to_string();
                let (sequences, _) = extract_complete_sequences(&before);
                for seq in sequences {
                    self.push_data(&mut events, seq);
                }
            }
            self.pending_kitty_printable_codepoint = None;
            let after = self.buffer[start + BRACKETED_PASTE_START.len()..].to_string();
            self.buffer.clear();
            self.paste_mode = true;
            self.paste_buffer = after;
            // Recurse into the paste branch to handle data already present.
            return {
                let mut more = self.drain();
                events.append(&mut more);
                events
            };
        }

        let (sequences, remainder) = extract_complete_sequences(&self.buffer);
        self.buffer = remainder;
        for seq in sequences {
            self.push_data(&mut events, seq);
        }
        events
    }

    fn push_data(&mut self, events: &mut Vec<StdinEvent>, sequence: String) {
        // Kitty "report alternate keys" sends a printable both as CSI-u and
        // as a plain char; drop the echo.
        let raw_codepoint = if sequence.chars().count() == 1 {
            sequence.chars().next().map(|c| c as u32)
        } else {
            None
        };
        if raw_codepoint.is_some() && raw_codepoint == self.pending_kitty_printable_codepoint {
            self.pending_kitty_printable_codepoint = None;
            return;
        }
        self.pending_kitty_printable_codepoint =
            parse_unmodified_kitty_printable_codepoint(&sequence);
        events.push(StdinEvent::Data(sequence));
    }

    /// After how long the caller should call `flush` when no further data
    /// arrives. `None` when the buffer is empty.
    pub fn pending_timeout_ms(&self) -> Option<u64> {
        if self.buffer.is_empty() {
            return None;
        }
        Some(if self.buffer == ESC.to_string() {
            self.escape_timeout_ms
        } else {
            self.sequence_timeout_ms
        })
    }

    /// Flush the incomplete remainder as raw data (the timeout path).
    pub fn flush(&mut self) -> Vec<StdinEvent> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let seq = std::mem::take(&mut self.buffer);
        self.pending_kitty_printable_codepoint = None;
        vec![StdinEvent::Data(seq)]
    }

    /// Discard all buffered state.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.paste_mode = false;
        self.paste_buffer.clear();
        self.pending_kitty_printable_codepoint = None;
    }

    /// The current incomplete remainder.
    pub fn buffered(&self) -> &str {
        &self.buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(events: Vec<StdinEvent>) -> Vec<String> {
        events
            .into_iter()
            .filter_map(|e| match e {
                StdinEvent::Data(s) => Some(s),
                StdinEvent::Paste(_) => None,
            })
            .collect()
    }

    #[test]
    fn splits_a_mouse_sequence_across_chunks() {
        let mut b = StdinBuffer::new();
        assert!(b.process("\x1b").is_empty());
        assert!(b.process("[<35").is_empty());
        let events = b.process(";20;5m");
        assert_eq!(data(events), vec!["\x1b[<35;20;5m"]);
    }

    #[test]
    fn incomplete_remainder_exposes_a_timeout() {
        let mut b = StdinBuffer::new();
        assert!(b.process("\x1b").is_empty());
        assert_eq!(b.pending_timeout_ms(), Some(DEFAULT_ESCAPE_TIMEOUT_MS));
        assert!(b.process("[").is_empty());
        assert_eq!(b.pending_timeout_ms(), Some(DEFAULT_SEQUENCE_TIMEOUT_MS));
        let flushed = data(b.flush());
        assert_eq!(flushed, vec!["\x1b["]);
        assert_eq!(b.pending_timeout_ms(), None);
    }

    #[test]
    fn wezterm_escape_split_rule_keeps_the_csi_u_intact() {
        let mut b = StdinBuffer::new();
        let events = b.process("\x1b\x1b[27;1:3u");
        assert_eq!(data(events), vec!["\x1b", "\x1b[27;1:3u"]);
    }

    #[test]
    fn bracketed_paste_is_verbatim_and_markers_are_stripped() {
        let mut b = StdinBuffer::new();
        let events = b.process("\x1b[200~a\x1b[201~");
        assert_eq!(events, vec![StdinEvent::Paste("a".into())]);
        // Content that looks like sequences survives untouched.
        let mut b = StdinBuffer::new();
        let events = b.process("\x1b[200~\x1b[Ax:3F\x1b[201~");
        assert_eq!(events, vec![StdinEvent::Paste("\x1b[Ax:3F".into())]);
    }

    #[test]
    fn kitty_printable_dedupe_drops_the_echo() {
        let mut b = StdinBuffer::new();
        let events = b.process("\x1b[97ua");
        assert_eq!(data(events), vec!["\x1b[97u"]);
    }

    #[test]
    fn os_then_bel_and_dcs_st_complete() {
        let mut b = StdinBuffer::new();
        assert_eq!(
            data(b.process("\x1b]11;rgb:1e1e/1e1e/1e1e\x07")),
            vec!["\x1b]11;rgb:1e1e/1e1e/1e1e\x07"]
        );
        assert_eq!(
            data(b.process("\x1bP>|XTerm(1)\x1b\\")),
            vec!["\x1bP>|XTerm(1)\x1b\\"]
        );
    }

    #[test]
    fn high_byte_legacy_conversion() {
        let mut b = StdinBuffer::new();
        let events = b.process_bytes(&[0xC1]); // 193 -> ESC + 'A'
        assert_eq!(data(events), vec!["\x1bA"]);
    }

    /// Reconstruct the stream a consumer sees: `Data` sequences verbatim,
    /// a `Paste` re-wrapped with its markers (the terminal layer does this
    /// before forwarding).
    fn reconstruct(events: Vec<StdinEvent>) -> String {
        let mut out = String::new();
        for event in events {
            match event {
                StdinEvent::Data(s) => out.push_str(&s),
                StdinEvent::Paste(s) => {
                    out.push_str(BRACKETED_PASTE_START);
                    out.push_str(&s);
                    out.push_str(BRACKETED_PASTE_END);
                }
            }
        }
        out
    }

    // Verifies: R6/R8 - the parser's golden contract: no byte is ever lost.
    // A stream arriving whole, fragmented at any split point, or only via
    // `flush` reconstructs to exactly the input. This is pi's
    // flush-back-into-input rule asserted as a property.
    #[test]
    fn no_byte_is_ever_lost_at_any_split() {
        // Deliberately exercises the tricky families: SGR mouse, a
        // bracketed paste, a kitty CSI-u, and a legacy meta pair. It
        // avoids the kitty-printable-echo pattern, which the dedupe rule
        // drops on purpose.
        let stream = "\x1b[<35;20;5mhi\x1b[200~pasted\nbytes\x1b[201~\x1b[27;1:3u\x1bA";
        for split in 0..=stream.len() {
            if !stream.is_char_boundary(split) {
                continue;
            }
            let mut b = StdinBuffer::new();
            let mut out = String::new();
            out.push_str(&reconstruct(b.process(&stream[..split])));
            out.push_str(&reconstruct(b.process(&stream[split..])));
            out.push_str(&reconstruct(b.flush()));
            assert_eq!(out, stream, "no byte lost when split at {split}");
        }
    }

    // Verifies: R6 - the tmux CSI-u paste dialect arrives verbatim: the
    // paste buffer does no sequence parsing, so the re-encoded control
    // bytes reach the field where the shared primitive decodes them.
    #[test]
    fn a_csi_u_paste_arrives_verbatim() {
        let mut b = StdinBuffer::new();
        let events = b.process("\x1b[200~sk-a\x1b[106;5ub\x1b[201~");
        assert_eq!(events, vec![StdinEvent::Paste("sk-a\x1b[106;5ub".into())]);
    }

    // Verifies: R6 - with paste mode never enabled, an unbracketed flood is
    // ordinary input: no crash, no swallowed content.
    #[test]
    fn an_unbracketed_flood_is_plain_input() {
        let mut b = StdinBuffer::new();
        let events = b.process("a flood of text\nwith a newline");
        assert_eq!(
            data(events).concat(),
            "a flood of text\nwith a newline",
            "every character comes out as input"
        );
    }
}
