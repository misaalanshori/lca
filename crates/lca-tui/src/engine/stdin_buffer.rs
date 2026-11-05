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
//!
//! # Reply fragments are not text (gh #366 gap 3)
//!
//! A terminal's answer to one of our startup queries can straddle two packets
//! by more than the 50 ms flush timeout over SSH or a multiplexer. The old
//! `flush` delivered the head as **text**, where it is indistinguishable from
//! typing, so `[?62;1;2c` landed in the composer on startup.
//!
//! The decision therefore lives **here**, in the reassembly seam, not in the
//! editor: by the time a fragment reaches `handle_key` nothing knows it was
//! ever part of a sequence. [`is_reply_prefix`] answers "could this be the
//! start of a reply we asked for", [`StdinBuffer::flush`] drops such a head
//! instead of delivering it, and a bounded **content** window
//! ([`REPLY_TAIL_MAX_CHARS`]) keeps the tail - which now arrives as ordinary
//! characters - from being typed either. Content, not a clock: a link slow
//! enough to split the intro is slow enough to stretch the tail past any fixed
//! deadline, which a 200 ms window demonstrated by failing a 400 ms split.
//!
//! The recognition is deliberately **narrow**: only the four replies the
//! startup queries can elicit are candidates, so pasted content containing
//! `ESC [` is still delivered (see the narrowness rows).

const ESC: char = '\x1b';
/// Default maximum wait for an incomplete (non-lone-ESC) sequence.
pub const DEFAULT_SEQUENCE_TIMEOUT_MS: u64 = 50;
/// Default maximum wait after a lone ESC (Alt+key reassembly window).
pub const DEFAULT_ESCAPE_TIMEOUT_MS: u64 = 10;
/// How many characters after a discarded reply head can still belong to its
/// tail.
///
/// Deliberately a **content** budget, not a clock. A time window cannot cover
/// an arbitrary split - a link slow enough to split the intro can be slow
/// enough to stretch the tail past any fixed deadline (a 200 ms window failed a
/// 400 ms split, which is exactly the shape this bug is about). A reply's tail
/// is a short run of digits, `;`, `?`, `:` and one final byte, so bounding it
/// by *shape* closes the gap on any link and gives up after a bounded number of
/// characters instead of never.
///
/// 64 is generous for every reply we ask for (`?62;1;2c` is 8; the longest
/// realistic `DA1` is under 30) while a user typing ordinary prose passes
/// through it in a fraction of a second - and even then the window only opens
/// after a discarded reply head, which a user cannot produce by typing.
pub const REPLY_TAIL_MAX_CHARS: usize = 64;

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
    // Gh #368 bug 3: Kitty flag `4` (which LCA itself negotiates) reports a
    // printable as `ESC [ cp : shifted? : base? u`, so the colon sub-parameters
    // are part of the protocol rather than noise. pi's grammar is
    // `^\x1b\[(\d+)(?::\d*)?(?::\d+)?u$`: after the codepoint, an optional
    // `:` with **zero or more** digits (the shifted field may be empty), then an
    // optional `:` with one or more (the base-layout field).
    //
    // Anything else declines rather than guessing. A parser that half-read the
    // sequence would arm the echo dedup on the wrong codepoint and then drop a
    // real keystroke - a worse failure than the duplicate it fixes.
    for field in 0..2 {
        if body.get(i) != Some(&b':') {
            break;
        }
        i += 1;
        let digits_start = i;
        while i < body.len() && body[i].is_ascii_digit() {
            i += 1;
        }
        // The first sub-parameter may be empty (`97:u` is a legal spelling of
        // `97::u`); the second may not, or a trailing `97:` would parse as a
        // complete sequence with a missing field.
        if field == 1 && i == digits_start {
            return None;
        }
    }
    if i != body.len() {
        return None;
    }
    if codepoint >= 32 {
        Some(codepoint)
    } else {
        None
    }
}

/// A reply we asked for at startup, in the shapes `parse_negotiation` and
/// the theme/status handlers recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplyKind {
    /// Kitty keyboard flags: `ESC [ ? <digits> u`.
    KittyFlags,
    /// Device attributes (`DA1`): `ESC [ ? <digits>[;<digits>]* c`.
    DeviceAttributes,
    /// DEC colour-scheme report: `ESC [ ? 996 [;1|;2] n`.
    ColorScheme,
    /// `OSC 11` background colour: `ESC ] 11 ; <body> (BEL | ST)`.
    Osc11,
    /// `OSC 7501` program-status support reply: `ESC ] 7501 ; ? [;...] (BEL | ST)`.
    ProgramStatus,
    /// `DCS > | … ST` - an XTVersion reply (gh #368 bug 1).
    XtVersion,
    /// `APC G <params> ; … ST` - a Kitty graphics reply (gh #368 bug 1).
    KittyGraphics,
}

impl ReplyKind {
    /// The final byte, once the sequence has produced one. `0` for the
    /// delimited-string families, which end on a terminator instead.
    fn final_byte(self) -> u8 {
        match self {
            Self::KittyFlags => b'u',
            Self::DeviceAttributes => b'c',
            Self::ColorScheme => b'n',
            Self::Osc11 | Self::ProgramStatus | Self::XtVersion | Self::KittyGraphics => 0,
        }
    }

    /// Whether this family is a **delimited string** - a body of arbitrary
    /// printable text ended by a terminator, rather than a parameter run ended
    /// by a final byte (gh #368 bug 1).
    ///
    /// The distinction is what the tail rules hang on: a colour body may
    /// contain `c` and `n`, and an XTVersion body contains `>`, `|` and `(`,
    /// so a shared "parameter alphabet" would cut both short and then type the
    /// rest.
    fn is_delimited(self) -> bool {
        !matches!(
            self,
            Self::KittyFlags | Self::DeviceAttributes | Self::ColorScheme
        )
    }

    /// The sequence this family ends on: `BEL` or `ST` for the `OSC` families,
    /// **`ST` only** for `DCS`/`APC` (gh #368 design call 4), and the final byte
    /// for the `CSI` families.
    ///
    /// `BEL` is deliberately not a `DCS`/`APC` terminator: inside those strings
    /// it is an ordinary body byte, so accepting it would cut a version reply in
    /// half and type the remainder.
    fn terminates_on_bel(self) -> bool {
        matches!(self, Self::Osc11 | Self::ProgramStatus)
    }
}

/// Whether `fragment` is a strict *prefix* of a reply we asked for.
///
/// This is the narrowness the fix turns on. `ESC [` alone is **not** a reply
/// prefix (a cursor-position CSI, a pasted `ESC [` and any other CSI all start
/// the same way), so it stays typing. A prefix qualifies only once it has
/// committed to the reply family:
///
/// - `ESC [ ?` — every reply in the `CSI ?` family starts here (the Kitty
///   flags answer, `DA1`, the colour-scheme report), so the bare introducer
///   is enough;
/// - `ESC [ ? 9 9 6` — only the colour-scheme reply starts with `996`, which
///   is what keeps an unrelated `ESC [ ? 9 9 z` typing;
/// - `ESC ] 1 1 ;` — only `OSC 11` starts here (`OSC 10` is the foreground,
///   `OSC 110` a reset), so the separator commits;
/// - `ESC ] 7 5 0 1` — only the program-status reply starts here;
/// - `ESC ] 11` / `ESC ] 7501` with the separator still outstanding.
///
/// A fragment that has already produced its final byte **and** left the intro
/// grammar is *not* a prefix: `ESC [ ? a` cannot become any of these replies,
/// so it is text.
#[must_use]
pub fn is_reply_prefix(fragment: &str) -> bool {
    let Some(kind) = reply_matches_so_far(fragment) else {
        return false;
    };
    // A fragment that already ended its sequence is a complete reply, which
    // `process` would have emitted; as a prefix it is no longer growing into
    // one of ours, so it is not our business.
    !fragment_is_finished(fragment, kind)
}

/// Which reply family `fragment` has committed to, if any.
fn reply_matches_so_far(fragment: &str) -> Option<ReplyKind> {
    if !fragment.starts_with(ESC) {
        return None;
    }
    let body = &fragment[ESC.len_utf8()..];
    if let Some(rest) = body.strip_prefix('[') {
        // The `CSI ?` family.
        let rest = rest.strip_prefix('?')?;
        // `996` commits to the colour-scheme reply; a shorter run of digits
        // could still grow into any of the `CSI ?` replies.
        if rest.starts_with("996") {
            let tail = rest.strip_prefix("996")?;
            return tail_ok(tail, b'n', |c| c.is_ascii_digit() || c == ';')
                .then_some(ReplyKind::ColorScheme);
        }
        // Digits then a final byte. `u` is Kitty flags, `c` is DA1; any other
        // final byte is not a reply we asked for.
        //
        // `ESC [ ?` with nothing after it already commits to this family -
        // that is exactly the split the bug is about, and every `CSI ?` reply
        // we asked for starts here, so it is reported as the representative
        // kind. Which one it turns out to be is `parse_negotiation`'s problem
        // once the tail arrives; the buffer only needs to know it is not text.
        let digits_len = rest.bytes().take_while(u8::is_ascii_digit).count();
        let after = &rest[digits_len..];
        if after.is_empty() {
            return Some(ReplyKind::KittyFlags);
        }
        let first = after.as_bytes()[0];
        if first == b'u' {
            return (after.len() == 1).then_some(ReplyKind::KittyFlags);
        }
        if first == b'c' || first == b';' {
            return tail_ok(after, b'c', |c| c.is_ascii_digit() || c == ';')
                .then_some(ReplyKind::DeviceAttributes);
        }
        return None;
    }
    // The OSC families.
    if let Some(rest) = body.strip_prefix(']') {
        if rest == "1" {
            return Some(ReplyKind::Osc11);
        }
        if rest == "11" {
            return Some(ReplyKind::Osc11);
        }
        if let Some(after) = rest.strip_prefix("11;") {
            return osc_body_ok(after).then_some(ReplyKind::Osc11);
        }
        if rest == "7" || rest == "75" || rest == "750" || rest == "7501" {
            return Some(ReplyKind::ProgramStatus);
        }
        if let Some(after) = rest.strip_prefix("7501;") {
            return osc_body_ok(after).then_some(ReplyKind::ProgramStatus);
        }
        return None;
    }
    // Gh #368 bug 1: the two delimited-string families #366 did not name. Their
    // heads were delivered as text, so the tail was **typed into the prompt**.
    //
    // The commitment point is deliberately past the bare introducer, because the
    // introducer alone is an `Alt` keystroke (bug 2): `ESC P` is `Alt+P`+`P`, not
    // a version reply; `ESC P > |` is the reply.
    if let Some(rest) = body.strip_prefix('P') {
        return (rest.starts_with(">|")).then_some(ReplyKind::XtVersion);
    }
    if let Some(rest) = body.strip_prefix('_') {
        // `APC G <key=value,>… ;` is the Kitty graphics reply. The commitment is
        // the first whole parameter, so a user typing `G` after `Alt+_` is still
        // typing (pi's header pattern requires the same `;`).
        return graphics_header_committed(rest).then_some(ReplyKind::KittyGraphics);
    }
    None
}

/// Whether `rest` (what follows `ESC _`) has committed to the Kitty graphics
/// reply (gh #368 bug 1).
///
/// The commitment is `G` plus a parameter run containing at least one `=`, i.e.
/// the shape `G i=1` - a key and its value. Deliberately short of pi's
/// `(?:[a-z]=\d+,)*[a-z]=\d+;`, which needs the complete first pair, because a
/// flush boundary can fall *inside* the first pair (`ESC _ G i=1` …) and a head
/// that has not committed is delivered as text - so requiring the `;` would
/// leave exactly the leak this row set exists to close.
///
/// The narrowness that keeps bug 2 recoverable lives on the other side: `ESC _`
/// alone, and `ESC _ G` alone, have no `=` and stay typing, so `Alt+_` followed
/// by a `G` is still a keystroke.
fn graphics_header_committed(rest: &str) -> bool {
    let Some(rest) = rest.strip_prefix('G') else {
        return false;
    };
    let params: String = rest
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '=' || *c == ',')
        .collect();
    params.contains('=') && !params.ends_with('=')
}

/// Whether `tail` is a permitted continuation of a `CSI ?` parameter run
/// that ends in `final_byte`.
///
/// `tail` may be a *prefix* of the finished sequence: `62;` and `62;1` are
/// both legitimate heads of `ESC [ ? 62 ; 1 ; 2 c`, and a head is exactly what
/// the flush boundary produces. So the rule is "digits and semicolons, and the
/// final byte only if present":
///
/// - all but the last character must be in the parameter alphabet;
/// - if the last character is not in that alphabet it must be `final_byte`;
/// - anything else (a different final byte) is not this reply.
fn tail_ok(tail: &str, final_byte: u8, is_param: impl Fn(char) -> bool) -> bool {
    let Some((last, rest)) = tail
        .char_indices()
        .next_back()
        .map(|(i, c)| (c, &tail[..i]))
    else {
        // Empty tail: the introducer alone is a valid head.
        return true;
    };
    if !rest.chars().all(&is_param) {
        return false;
    }
    is_param(last) || last == char::from(final_byte)
}

/// Whether an OSC body is still inside the reply's grammar (no terminator
/// required - an unterminated body is exactly a prefix).
fn osc_body_ok(body: &str) -> bool {
    // Printable ASCII only, and not yet terminated: a terminator means the
    // sequence is complete, which `process` emits on its own.
    body.chars().all(|c| (' '..='~').contains(&c))
}

/// Whether a fragment already ended a sequence, so it is not a prefix of
/// something longer.
fn fragment_is_finished(fragment: &str, kind: ReplyKind) -> bool {
    if kind.is_delimited() {
        return fragment.ends_with("\x1b\\")
            || (kind.terminates_on_bel() && fragment.ends_with('\x07'));
    }
    fragment.bytes().last() == Some(kind.final_byte())
}

/// Whether `c` can appear in the tail of a reply of this family.
///
/// For a **delimited string** (`OSC`, `DCS`, `APC`) the body is arbitrary
/// printable text - `rgb:1e1e/1e1e/1e1e`, `>|XTerm(390)` - so the only shape
/// rule is "printable". A control byte ends the window.
///
/// For a **parameter run** (`CSI ?`) it is digits and `;`/`:` with a letter
/// admitted too, because a `CSI ?` head that has not reached its final byte is
/// *uncommitted*: `ESC [ ? 62` could still become `…62;1;2c` (`DA1`) or `…996n`
/// (the colour-scheme report), and each ends on a different letter. Which one it
/// is gets decided by the byte that arrives, so the window admits any letter and
/// the family's own final byte closes it.
fn is_reply_tail_char(kind: ReplyKind, c: char) -> bool {
    if kind.is_delimited() {
        return (' '..='~').contains(&c);
    }
    c.is_ascii_digit() || matches!(c, ';' | ':' | '/' | '?' | '#' | '.') || c.is_ascii_alphabetic()
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
    /// How many reply heads have been discarded (gh #366 gap 3). A counter, not
    /// a flag, so a test can say "the drop happened" rather than "nothing was
    /// printed".
    discarded_fragments: u32,
    /// Set when a reply head was discarded: which reply is being completed and
    /// how many more characters may still belong to its tail. After a flush the
    /// tail arrives as ordinary text, so content is the only way the buffer can
    /// still recognise it (see [`REPLY_TAIL_MAX_CHARS`]).
    reply_tail: Option<(ReplyKind, usize)>,
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
            discarded_fragments: 0,
            reply_tail: None,
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
        // Gh #366 gap 3: after a discarded reply head, the reply's tail arrives
        // as ordinary characters (`62;1;2c`). Drop them so the fragment cannot
        // reassemble itself in the composer.
        //
        // The window closes on **shape**, not on a clock (the budget rationale
        // is on `REPLY_TAIL_MAX_CHARS`). Each character is judged in turn, so a
        // tail that arrives whole in one read is handled exactly like one that
        // dribbles in: a character that cannot appear in a reply tail - a
        // space, a control byte - ends the window, and everything from it
        // onward is the user's own input and is delivered.
        if let Some((kind, left)) = self.reply_tail {
            // `ST` (`ESC \`) is emitted by the reassembler as one two-character
            // chunk, because the generic "ESC + one char is a meta pair" rule
            // treats it as such. It terminates an OSC reply, so swallow the
            // chunk whole rather than reading the ESC as prose.
            if kind.is_delimited() && sequence == "\x1b\\" {
                self.reply_tail = None;
                return;
            }
            let mut kept = String::new();
            let mut remaining = left;
            let mut open = true;
            let mut previous_esc = false;
            for c in sequence.chars() {
                if !open {
                    kept.push(c);
                    continue;
                }
                // A delimited string ends on its terminator (`ST`, or `BEL` for
                // the `OSC` families only); a `CSI ?` reply ends on its own final
                // byte. Family matters: an `OSC 11` colour body legitimately
                // contains `c` (`#1e1e2c`) and `n`, so a blanket "a letter ends
                // it" rule would cut a colour short and then deliver the rest as
                // typing.
                let ends = if kind.is_delimited() {
                    (kind.terminates_on_bel() && c == '\x07') || (previous_esc && c == '\\')
                } else {
                    // An uncommitted `CSI ?` head ends on **any** of the family's
                    // final bytes, not just the representative one: which reply it
                    // turns out to be is only known from the byte that arrives.
                    matches!(c as u8, b'u' | b'c' | b'n')
                };
                previous_esc = c == ESC;
                if ends {
                    self.reply_tail = None;
                    open = false;
                    continue;
                }
                if !is_reply_tail_char(kind, c) {
                    // Not reply-shaped: the user is typing. Close and keep it.
                    self.reply_tail = None;
                    open = false;
                    kept.push(c);
                    continue;
                }
                remaining = remaining.saturating_sub(1);
                if remaining == 0 {
                    self.reply_tail = None;
                    open = false;
                }
            }
            if open {
                self.reply_tail = Some((kind, remaining));
            }
            if kept.is_empty() {
                return;
            }
            events.push(StdinEvent::Data(kept));
            return;
        }
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

    /// The sequence timeout this buffer was built with.
    #[must_use]
    pub fn sequence_timeout_ms(&self) -> u64 {
        self.sequence_timeout_ms
    }

    /// The lone-`ESC` timeout this buffer was built with.
    #[must_use]
    pub fn escape_timeout_ms(&self) -> u64 {
        self.escape_timeout_ms
    }

    /// How many reply fragments have been discarded instead of delivered
    /// (gh #366 gap 3).
    #[must_use]
    pub fn discarded_fragments(&self) -> u32 {
        self.discarded_fragments
    }

    /// Flush the incomplete remainder (the timeout path).
    ///
    /// A remainder that is a **prefix of a reply we asked for** is dropped
    /// rather than delivered (gh #366 gap 3), and the tail window opens so the
    /// rest of that reply - which now arrives as ordinary characters - is
    /// dropped too. Anything else is the user's input and is delivered.
    pub fn flush(&mut self) -> Vec<StdinEvent> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let seq = std::mem::take(&mut self.buffer);
        self.pending_kitty_printable_codepoint = None;
        if is_reply_prefix(&seq) {
            self.discarded_fragments = self.discarded_fragments.saturating_add(1);
            if let Some(kind) = reply_matches_so_far(&seq) {
                self.reply_tail = Some((kind, REPLY_TAIL_MAX_CHARS));
            }
            return Vec::new();
        }
        revoke_alt_key(&seq)
    }

    /// Whether a discarded reply's tail may still be arriving: the characters
    /// now are the remainder of a reply whose head was flushed away.
    #[must_use]
    pub fn discarding_reply_tail(&self) -> bool {
        self.reply_tail.is_some_and(|(_, left)| left > 0)
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

/// Split an unterminated control-string remainder back into the keystroke it
/// was and the typing that followed it (gh #368 bug 2).
///
/// # The bug
///
/// In a legacy terminal `Alt+]` sends the `OSC` introducer (`ESC ]`), `Alt+P`
/// the `DCS` one (`ESC P`) and `Alt+_` the `APC` one (`ESC _`). None of them
/// carries a terminator, so `is_complete_sequence` reports *incomplete* forever
/// and the buffer keeps accumulating: the key and every character after it
/// become one glued event (`ESC ] a b c`) that no parser can read. **Every
/// subsequent keystroke is swallowed** and the session looks frozen.
///
/// # The rule
///
/// **An introducer with no terminator is not a sequence.** On expiry the first
/// two bytes are the meta pair the user pressed, and the rest is what they
/// typed - so the remainder is split back into input rather than delivered
/// glued, and `parse_key` reads `ESC ]` as `alt+]` exactly as it does when the
/// key arrives alone.
///
/// # Why this does not undo #366
///
/// Not every control-string remainder is a keystroke. A fragment that has
/// **committed** to a reply we asked for (`ESC ] 11 ; …`) is dropped with its
/// tail window, which `flush` does before it reaches here: a reply is not
/// typing, and the two are told apart by the prefix set
/// ([`is_reply_prefix`](is_reply_prefix)), not by the introducer.
///
/// # Why reparse rather than emit two chunks
///
/// pi's shape (`stdin-buffer.ts`'s flush handler): emit the two-byte introducer
/// and *reprocess* the tail, so a `Ctrl+M` or a `Ctrl+C` in what the user typed
/// becomes the key it is. Emitting `ESC ]` + the raw tail as one string per
/// piece would type the literal two characters into the composer instead of
/// submitting.
fn revoke_alt_key(remainder: &str) -> Vec<StdinEvent> {
    /// The three control-string introducers, and deliberately not `ESC [`.
    ///
    /// A `CSI` is not a keystroke plus typing: `ESC [` alone is `ctrl+alt+[` *and*
    /// the head of a half-arrived arrow key, so splitting it would trade one
    /// ambiguity for another. pi draws the line in the same place
    /// (`isControlString`).
    fn is_introducer(text: &str) -> bool {
        text.starts_with("\x1b]") || text.starts_with("\x1bP") || text.starts_with("\x1b_")
    }

    let mut events = Vec::new();
    let mut rest = remainder.to_string();
    // **One buffer, one loop.** Two earlier shapes are recorded here rather than
    // forgotten: recursing once per introducer made the depth grow with the input
    // and a burst of a few thousand aborted the process with a stack overflow, and
    // rebuilding a fresh buffer per pass over the whole tail was O(n²) - 4000
    // introducers took 8.8 s, a freeze of its own. Every pass here either emits
    // bytes or ends.
    let mut tail = StdinBuffer::new();
    loop {
        while rest.len() > 2 && is_introducer(&rest) {
            // The two introducer bytes are the meta pair the user pressed.
            events.push(StdinEvent::Data(rest[..2].to_string()));
            rest = rest[2..].to_string();
        }
        if rest.is_empty() {
            return events;
        }
        if rest.len() <= 2 && is_introducer(&rest) {
            // A lone introducer is already the meta pair `parse_key` reads as
            // `alt+]`: one event, nothing to split.
            events.push(StdinEvent::Data(rest));
            return events;
        }
        // Whatever is left is typing, and may itself contain sequences - so it goes
        // through the buffer, which is drained here and reused next pass.
        events.extend(tail.process(&rest));
        let buffered = tail.buffered().to_string();
        if buffered.is_empty() {
            return events;
        }
        if !is_introducer(&buffered) || buffered.len() <= 2 {
            events.push(StdinEvent::Data(buffered));
            return events;
        }
        // The tail *ended* on an abandoned introducer: peel it and go again.
        events.push(StdinEvent::Data(buffered[..2].to_string()));
        rest = buffered[2..].to_string();
    }
}

#[cfg(test)]
#[path = "stdin_buffer/reply_fragment_tests.rs"]
mod reply_fragment_tests;

#[cfg(test)]
#[path = "stdin_buffer/recovery_tests.rs"]
mod recovery_tests;

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
