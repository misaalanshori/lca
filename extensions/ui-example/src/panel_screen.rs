//! The panel's rolling screen: what one frame re-renders when no new bytes
//! arrived since the last one (gh #301).
//!
//! The panel is a *screen*, not a stream. A frame that drained zero bytes still
//! has to draw the session that is already on it, so the bytes a frame read are
//! kept and the tail of that history is what gets drawn. The previous shape
//! drained a chunk, drew it, and kept nothing - so any frame the ticker pulled
//! with no new output drew a bare header, and the panel flickered between
//! content and blank as the two cadences drifted.
//!
//! # The bound is a decision, not a guess
//!
//! `SCREEN_MAX_BYTES` is large enough to fill a tall panel several times over
//! (`PANEL_BODY_ROWS` rows at a generous 200 cells is ~4 KiB, so 16 KiB leaves
//! four panels of scrollback for a pane that grew) and small enough that a
//! `yes`-style child - megabytes per second, the adversarial case - costs
//! nothing: the buffer is trimmed on every append, so it never exceeds the bound
//! by more than one chunk. `SCREEN_MAX_ROWS` is the same argument in rows,
//! which is the unit that matters when the output is many short lines.
//!
//! # Trimming lands on a line boundary
//!
//! Half a row at the top reads as corruption, and a start in the middle of an
//! escape sequence paints the sequence's leftovers over the column. Cutting at
//! the first newline at or after the byte budget gives both guarantees at once:
//! the retained text begins a line, and no sequence can span a newline.
//!
//! The one line longer than the whole budget has no boundary to cut at, so the
//! tail is kept and the cut is moved back to its escape sequence's own introducer
//! - which `strip_ansi` then drops as an incomplete sequence. That is the only
//! path on which a cut can land inside a sequence, and it is handled on purpose
//! rather than by luck.
//!
//! # A partial escape sequence is never drawn
//!
//! `strip_ansi` consumes whole sequences and drops an incomplete one at the
//! end of the text, so a chunk that ended mid-sequence shows nothing this frame
//! and the completed sequence shows next frame. Rendering the raw bytes instead
//! would print `\x1b[31m` literally: the host's `sanitize_text` turns a control
//! character into visible `\xNN` text (NFR-28's rule, and the right one - a
//! sandboxed extension must never hand the terminal a sequence), so an
//! unsanitised panel shows escape debris where the author meant colour.

/// The most bytes the screen keeps, oldest dropped first.
///
/// See the module docs: four panels of scrollback at a generous cell width, and
/// small enough that the adversarial producer costs a bounded trim per chunk.
pub(crate) const SCREEN_MAX_BYTES: usize = 16 * 1024;

/// The most rows the screen keeps, oldest dropped first.
///
/// The byte bound alone would let a fast producer of short lines (one character
/// per row) hold [`SCREEN_MAX_BYTES`] rows, which is memory spent on rows no
/// panel can show. This is the bound in the unit that actually reaches the
/// screen.
pub(crate) const SCREEN_MAX_ROWS: usize = 200;

/// The rows the panel draws each frame.
///
/// A tall-ish panel's body on an 80x24 terminal, with the header rows above it.
/// The screen keeps far more than this so the tail a frame needs is always
/// present; drawing more would just be clipped by the host.
pub const PANEL_BODY_ROWS: usize = 20;

/// What the panel says instead of output once the program has exited.
///
/// A dead session that kept drawing its last output would be
/// indistinguishable from one that has gone quiet, which is the same class of
/// dishonesty as a blank frame.
#[must_use]
pub fn ended_label(code: i32) -> String {
    format!("session ended (exit {code})")
}

/// The rolling screen behind the panel.
///
/// Holds the newest output plus whether the program has exited, and nothing
/// else: the pty handle lives in the extension handle, so this is data a frame
/// can re-render as many times as the ticker asks.
#[derive(Debug, Default)]
pub struct PanelScreen {
    /// The kept output, raw (escape sequences intact) - stripped on the way out,
    /// so an incomplete sequence at the end completes on the next chunk instead
    /// of being lost.
    text: String,
    /// The program's exit code once it has exited.
    ended: Option<i32>,
    /// Whether anything was ever dropped, so the panel can say that the history
    /// it shows is not the whole session.
    trimmed: bool,
}

impl PanelScreen {
    /// An empty screen.
    #[must_use]
    pub fn new() -> PanelScreen {
        PanelScreen::default()
    }

    /// Record what one read returned.
    ///
    /// `chunk` is the bytes the pty produced (empty when it had none);
    /// `ended` is the exit code when the read reported the session over.
    /// Both may arrive together - a program commonly exits with its last output
    /// still in the pipe - and neither is required.
    pub fn observe(&mut self, chunk: &[u8], ended: Option<i32>) {
        if ended.is_some() {
            self.ended = ended;
        }
        if chunk.is_empty() {
            return;
        }
        // Lossy per chunk is safe here because a buffer that always begins a
        // line can still begin mid-character only if the cut split one, and
        // `from_utf8_lossy` renders that as U+FFFD - a visible, honest mojibake
        // for one cell, rather than a second byte-level representation of the
        // same text kept in step with this one.
        self.text.push_str(&String::from_utf8_lossy(chunk));
        self.trim();
    }

    /// The retained output with escape sequences removed.
    #[must_use]
    pub fn text(&self) -> String {
        strip_ansi(&self.text)
    }

    /// The output as display rows, oldest first, at most [`PANEL_BODY_ROWS`] of
    /// them, preceded by a trim notice when the history shown is not everything.
    #[must_use]
    pub fn rows(&self) -> Vec<String> {
        let text = self.text();
        let all: Vec<&str> = text.lines().collect();
        let start = all.len().saturating_sub(PANEL_BODY_ROWS);
        let mut rows: Vec<String> = Vec::with_capacity(all.len() - start + 1);
        if self.trimmed && start > 0 {
            rows.push("earlier output dropped".to_string());
        }
        rows.extend(all[start..].iter().map(|line| (*line).to_string()));
        rows
    }

    /// The exit code once the program has exited.
    #[must_use]
    pub fn ended(&self) -> Option<i32> {
        self.ended
    }

    /// Whether anything is on the screen yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows().is_empty()
    }

    /// Drop the oldest output until the screen is within both bounds.
    fn trim(&mut self) {
        // Rows first: it is the cheaper bound to check and the one a burst of
        // short lines hits, and trimming to it can only make the byte check
        // easier to satisfy.
        let mut rows = self.text.lines().count();
        while rows > SCREEN_MAX_ROWS {
            match self.text.find('\n') {
                Some(nl) => {
                    self.text.drain(..=nl);
                    self.trimmed = true;
                    rows -= 1;
                }
                // One line longer than the row bound: there is no older row to
                // drop, so the byte bound below is the only one that can help.
                None => break,
            }
        }
        if self.text.len() > SCREEN_MAX_BYTES {
            let cut = safe_cut(&self.text);
            if cut > 0 {
                // `drain` would need a char boundary; `safe_cut` returns one.
                self.text.drain(..cut);
                self.trimmed = true;
            }
        }
    }
}

/// Where to cut so the retained tail begins a line and never inside a sequence.
///
/// Returns a byte offset into `text` that is a char boundary.
fn safe_cut(text: &str) -> usize {
    let budget_start = text.len() - SCREEN_MAX_BYTES;
    // Prefer the first newline at or after the budget: the retained text then
    // begins a row, and no escape sequence spans a newline.
    if let Some(offset) = text[budget_start..].find('\n') {
        return budget_start + offset + 1;
    }
    // A single line longer than the budget. Cut at the budget, but move the cut
    // back to the introducer of whatever sequence it landed inside, so the
    // partial sequence is dropped whole rather than printed as debris.
    let cut = escape_boundary(text, budget_start);
    // Never split a UTF-8 character either; a lossy round trip on the next
    // append would otherwise invent a replacement character at the join.
    let mut cut = cut;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

/// Move `cut` back to the start of an escape sequence the cut would land inside.
///
/// Only the bytes immediately before `cut` can hold the introducer of a sequence
/// reaching it, and a CSI/OSC introducer is a handful of bytes, so the look-back
/// is bounded rather than a scan of the whole buffer.
fn escape_boundary(text: &str, cut: usize) -> usize {
    const LOOKBACK: usize = 64;
    let start = cut.saturating_sub(LOOKBACK);
    let window = &text[start..cut];
    let Some(rel) = window.rfind('\x1b') else {
        return cut;
    };
    if sequence_len(&window[rel..]).is_some() {
        // The sequence the last ESC began ends before the cut, so the cut is
        // already between sequences.
        cut
    } else {
        // Unterminated at the cut: drop from its introducer.
        start + rel
    }
}

/// The length of the complete escape sequence at the start of `text`, or `None`
/// when one begins there but is not finished.
fn sequence_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&0x1b) {
        return None;
    }
    let kind = bytes.get(1)?;
    match kind {
        // CSI: parameters/intermediates, then a final byte in 0x40..=0x7e.
        b'[' => bytes[2..]
            .iter()
            .position(|b| (0x40..=0x7e).contains(b))
            .map(|pos| pos + 3),
        // OSC: terminated by BEL or ST (ESC \).
        b']' => {
            let rest = &text[2..];
            if let Some(bel) = rest.find('\x07') {
                return Some(bel + 3);
            }
            rest.find('\x1b')
                .filter(|at| rest.as_bytes().get(at + 1) == Some(&b'\\'))
                .map(|at| at + 4)
        }
        // Every other escape takes exactly one more byte.
        _ => Some(2),
    }
}

/// Remove escape sequences from terminal output, dropping an incomplete one at
/// the end (it completes on a later chunk).
///
/// This is display cleanup, not a security boundary: the host's own
/// `sanitize_text` is what guarantees no sequence reaches the terminal. Doing it
/// here is what makes the panel *readable* instead of covered in `\x1b[31m`
/// debris, because that sanitizer renders a control character as visible text.
#[must_use]
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(esc) = rest.find('\x1b') {
        out.push_str(&rest[..esc]);
        let tail = &rest[esc..];
        match sequence_len(tail) {
            Some(len) => rest = &tail[len..],
            // Incomplete: nothing after it can be drawn yet.
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

    use super::*;

    /// Rows a screen would draw, for the bound assertions.
    fn row_count(screen: &PanelScreen) -> usize {
        screen.rows().len()
    }

    // Verifies: gh #301 defect 2's core property - a frame with no new bytes
    // still has the previous output to draw.
    #[test]
    fn a_read_that_returned_nothing_leaves_the_previous_output_in_place() {
        let mut screen = PanelScreen::new();
        screen.observe(b"$ echo hi\r\nhi\r\n", None);
        let before = screen.text();
        screen.observe(&[], None);
        assert_eq!(screen.text(), before, "an empty read changes nothing");
        assert!(screen.text().contains("hi"), "{}", screen.text());
    }

    // Verifies: new bytes append rather than replace, and the tail is what the
    // rows show.
    #[test]
    fn new_bytes_append_and_the_rows_show_the_tail() {
        let mut screen = PanelScreen::new();
        screen.observe(b"first\r\n", None);
        screen.observe(b"second\r\n", None);
        assert!(screen.text().contains("first"), "{}", screen.text());
        assert!(screen.text().contains("second"), "{}", screen.text());
        let rows = screen.rows();
        assert_eq!(rows.last().map(String::as_str), Some("second"));
    }

    // Verifies: the rows a frame draws are capped, oldest first dropped, so a
    // tall history does not overflow a fixed-height panel.
    #[test]
    fn the_rows_drawn_are_capped_to_the_panel_body() {
        let mut screen = PanelScreen::new();
        for index in 0..(PANEL_BODY_ROWS + 10) {
            screen.observe(format!("line{index}\n").as_bytes(), None);
        }
        let rows = screen.rows();
        assert_eq!(rows.len(), PANEL_BODY_ROWS);
        assert_eq!(rows.last().map(String::as_str), Some("line29"));
        assert!(!rows[0].contains("line0"), "the oldest rows go first");
    }

    // Verifies: the byte bound, and that it trims on a line boundary so no
    // half row is left at the top.
    #[test]
    fn trimming_starts_on_a_line_boundary() {
        let mut screen = PanelScreen::new();
        // Fewer rows than the row bound, and long enough that the **byte** bound
        // is what trims - otherwise the row bound fires first and this row never
        // exercises the line-boundary cut at all (the fixture, not the code, was
        // wrong the first time: 2000 short rows are only ~5 KiB, so `safe_cut`
        // was never reached and a byte-off cut escaped).
        for index in 0..(SCREEN_MAX_ROWS - 20) {
            screen.observe(
                format!("row{index:03}-{}\n", "x".repeat(200)).as_bytes(),
                None,
            );
        }
        assert!(
            screen.text.len() <= SCREEN_MAX_BYTES,
            "kept {} bytes",
            screen.text.len()
        );
        assert!(
            screen.text.len() >= SCREEN_MAX_BYTES - 800,
            "the byte bound is what trimmed here, near its limit"
        );
        // The retained text begins *at* a line start. Cutting one byte later
        // would keep the separating newline and leave an empty leading row - the
        // off-by-one that "starts on a line boundary" exists to prevent, and
        // that the drawn rows alone cannot show.
        let kept = screen.text();
        assert!(
            !kept.starts_with('\n'),
            "the retained text opens with the newline it trimmed at: {:?}",
            &kept[..kept.len().min(40)]
        );
        let rows = screen.rows();
        for row in &rows {
            assert!(!row.starts_with('x'), "a row begins mid-line: {row:?}");
            assert!(!row.is_empty(), "a blank row is a cut artefact: {rows:?}");
        }
        // The last row the producer wrote is still there, whole.
        assert!(
            kept.ends_with(&format!("{}\n", "x".repeat(200))),
            "the tail is kept whole"
        );
    }

    // Verifies: a partial escape sequence is never drawn - the row that shows as
    // visible garbage when this is wrong.
    #[test]
    fn a_trimmed_start_never_leaves_a_partial_escape_sequence() {
        let mut screen = PanelScreen::new();
        // Coloured lines, long enough to force a byte trim.
        for index in 0..4000 {
            screen.observe(
                format!("\x1b[31mred{index}\x1b[0m plain\n").as_bytes(),
                None,
            );
        }
        let rows = screen.rows();
        for row in &rows {
            assert!(
                !row.contains("\x1b"),
                "an escape sequence survived into a row: {row:?}"
            );
            assert!(
                !row.starts_with('m') && !row.starts_with('['),
                "a row begins inside a sequence: {row:?}"
            );
        }
        assert!(
            rows.iter().any(|row| row.contains("plain")),
            "the visible text is kept: {rows:?}"
        );
    }

    // Verifies: a single line longer than the whole budget - the case with no
    // newline to cut at - still never starts inside a sequence.
    #[test]
    fn one_line_longer_than_the_budget_is_cut_at_an_escape_safe_point() {
        let mut screen = PanelScreen::new();
        let mut line = String::new();
        // Sequences early, then a long tail: the cut lands inside the padding,
        // where there is no sequence, and the assertion is that it stays clean.
        for index in 0..2000 {
            line.push_str(&format!("\x1b[32mg{index}\x1b[0m"));
        }
        screen.observe(line.as_bytes(), None);
        screen.observe("TAIL".as_bytes(), None);
        let text = screen.text();
        assert!(text.ends_with("TAIL"), "{text:?}");
        assert!(!text.contains('\x1b'), "no sequence survives");
        assert!(
            !text.starts_with('m') && !text.starts_with('['),
            "starts inside a sequence: {:?}",
            &text[..text.len().min(20)]
        );
    }

    // Verifies: the buffer stops growing under a fast producer - asserted on the
    // bound, not on memory, because the bound is what makes the memory claim.
    #[test]
    fn the_screen_stops_growing_under_a_fast_producer() {
        let mut screen = PanelScreen::new();
        // 3000 chunks of 1 KiB is ~3 MB through a 16 KiB buffer.
        let chunk = vec![b'y'; 1024];
        for _ in 0..3000 {
            screen.observe(&chunk, None);
            assert!(
                screen.text.len() <= SCREEN_MAX_BYTES,
                "grew past the byte bound: {}",
                screen.text.len()
            );
            assert!(row_count(&screen) <= SCREEN_MAX_ROWS + 1);
        }
        // Many short lines, the other shape a fast producer takes.
        for _ in 0..(SCREEN_MAX_ROWS * 3) {
            screen.observe(b"y\n", None);
            assert!(
                row_count(&screen) <= SCREEN_MAX_ROWS + 1,
                "grew past the row bound: {}",
                row_count(&screen)
            );
        }
    }

    // Verifies: a dead child is visible rather than frozen on its last output.
    #[test]
    fn an_exited_program_is_recorded_with_its_code() {
        let mut screen = PanelScreen::new();
        screen.observe(b"bye\r\n", None);
        assert_eq!(screen.ended(), None, "still running");
        screen.observe(b"", Some(3));
        assert_eq!(screen.ended(), Some(3));
        assert_eq!(ended_label(3), "session ended (exit 3)");
        // Its last output survives the exit - the panel shows both.
        assert!(screen.text().contains("bye"), "{}", screen.text());
    }

    // Verifies: an incomplete sequence at the end is held, not printed as
    // debris, and completes when the rest of the bytes arrive.
    #[test]
    fn a_split_escape_sequence_completes_on_the_next_chunk() {
        let mut screen = PanelScreen::new();
        screen.observe(b"a\x1b[3", None);
        assert_eq!(screen.text(), "a", "the half sequence is not drawn");
        screen.observe(b"1mb\n", None);
        assert_eq!(screen.text(), "ab\n", "and it never becomes debris");
    }

    // Verifies: the trim notice appears only when something was actually
    // dropped, so a short session does not claim a loss it never had.
    #[test]
    fn the_dropped_output_notice_appears_only_after_a_trim() {
        let mut screen = PanelScreen::new();
        screen.observe(b"short\r\n", None);
        assert!(
            !screen.rows().iter().any(|row| row.contains("dropped")),
            "{:?}",
            screen.rows()
        );
        for index in 0..(SCREEN_MAX_ROWS * 2) {
            screen.observe(format!("line{index}\n").as_bytes(), None);
        }
        assert!(
            screen
                .rows()
                .first()
                .is_some_and(|row| row.contains("dropped")),
            "{:?}",
            screen.rows()
        );
    }

    // Verifies: `strip_ansi` handles the three sequence shapes a shell actually
    // emits, and drops an unterminated one rather than leaking it.
    #[test]
    fn strip_ansi_removes_csi_osc_and_two_byte_escapes() {
        assert_eq!(strip_ansi("\x1b[1;31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b]0;title\x07text"), "text");
        assert_eq!(strip_ansi("\x1b]0;title\x1b\\text"), "text");
        assert_eq!(strip_ansi("\x1bcreset"), "reset");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("trailing\x1b[3"), "trailing");
        assert_eq!(strip_ansi(""), "");
    }
}
