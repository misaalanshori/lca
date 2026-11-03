//! Chip-aware motion and renumbering for the editor (gh #381 gaps 1-2).
//!
//! Split out of `editor.rs` when that file crossed the 1,200-line ceiling, the
//! same reason `editor_rows.rs` and `word.rs` were split.
//!
//! The whole file exists to answer one question for one predicate: **is the caret
//! on a paste chip, and if so where does it begin and end**. `chip_span` is the only
//! place the editor asks `engine::word::atomic_span_at` (added by gh #383 for word
//! motion), so plain motion, vertical motion, the wrapper and deletion cannot
//! disagree about what a unit is - which is what let `Left` walk 21 keystrokes
//! through a marker and one `Backspace` orphan it into literal text the model read.

use super::Editor;

/// One undo step (gh #381 gap 3).
///
/// The text and its caret are what a restore traditionally needed; the **paste
/// registry** is what it needs once a marker's payload lives outside the text. The
/// registry is `Option` on purpose: a payload can be megabytes, so most steps (every
/// keystroke) carry `None` and only the steps that can change it - paste, chip
/// delete, renumber - carry `Some`. That keeps the undo stack's memory proportional
/// to the number of registry changes rather than to the number of keystrokes.
pub(super) struct UndoEntry {
    pub(super) lines: Vec<String>,
    pub(super) cursor_line: usize,
    pub(super) cursor_col: usize,
    /// `Some(pastes)` when this step could change the registry.
    pub(super) chips: Option<Vec<String>>,
}

impl UndoEntry {
    /// The registry bytes this step holds, for the memory row.
    #[cfg(test)]
    pub(super) fn payload_bytes(&self) -> usize {
        self.chips
            .as_ref()
            .map(|pastes| pastes.iter().map(String::len).sum())
            .unwrap_or(0)
    }
}

/// The registry entries `text` can still reach (gh #381 gap 3).
///
/// The registry is the truth and a marker is its label, so `set_text` must decide
/// what survives a wholesale text replacement. Two facts shape the answer:
///
/// * **Ids are positional** (`pastes[id - 1]`), so a payload's index *is* its
///   meaning. Compacting the list would silently re-point `#2` at `#1`'s content -
///   the exact corruption this gap is about, reintroduced in the fix. So entries keep
///   their positions and the list is only **truncated** to the highest id the text
///   still references.
/// * **A text with no markers keeps nothing**, which is what makes `/clear` and any
///   wholesale replacement correct without a second code path.
///
/// A marker whose id exceeds the registry contributes nothing, matching
/// `expand_paste_markers`, so the two cannot disagree about what is live.
pub(super) fn retain_live_payloads(text: &str, pastes: &[String]) -> Vec<String> {
    const OPEN: &str = "[paste #";
    let mut highest = 0usize;
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        let tail = &rest[start..];
        let digits: String = tail[OPEN.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        match (digits.parse::<usize>(), tail.find(']')) {
            (Ok(id), Some(close)) if id >= 1 && id <= pastes.len() => {
                highest = highest.max(id);
                rest = &tail[close + 1..];
            }
            _ => {
                // Not a live marker: step past the bracket so the search advances.
                rest = &tail[1..];
            }
        }
    }
    pastes[..highest.min(pastes.len())].to_vec()
}

/// Expand `[paste #N …]` markers to the content they stand for (pi's
/// `expandPasteMarkers`). A marker whose id has no live entry, or one with
/// no closing bracket, is left literal so a user who types the shape by hand
/// is not surprised. Markers are produced by [`Editor::insert_paste`].
///
/// **This is why a chip must never be orphaned** (gh #381 gap 2): an unrecognised
/// shape stays literal on purpose, so `[paste #1 +123 lines` - no longer matching -
/// reached the model as text instead of the pasted content. The fix removes the
/// whole chip on delete, so the input to this function is either a complete marker
/// or no marker at all.
pub(super) fn expand_paste_markers(text: &str, pastes: &[String]) -> String {
    const OPEN: &str = "[paste #";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let marker = &rest[start..];
        let digits: String = marker[OPEN.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        match (digits.parse::<usize>(), marker.find(']')) {
            (Ok(id), Some(close)) if id >= 1 && id <= pastes.len() => {
                out.push_str(&pastes[id - 1]);
                rest = &marker[close + 1..];
            }
            _ => {
                // Not a marker this editor wrote: keep it and move past the
                // opening bracket so the search cannot loop.
                out.push('[');
                rest = &marker[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

impl Editor {
    /// Renumber the chips in the buffer to `1..n` in document order, rebuilding
    /// `pastes` to match (gh #381 gap 2).
    ///
    /// Ids are **positional** - `insert_paste` derives the label from
    /// `pastes.len() + 1` - so deleting chip `#1` of two used to leave a chip
    /// labelled `#2` whose payload was `pastes[0]`: the label said one thing and the
    /// expansion did another. The next paste also took id 2 and collided outright.
    ///
    /// The rule is **labels follow payloads**, never the reverse: each marker's
    /// payload is re-registered under its new position, so a renumber can only
    /// change the text of a marker, never what it expands to. A marker whose id has
    /// no payload (hand-typed, or already orphaned) is left exactly as written -
    /// renumbering must not invent content for text the user typed.
    pub(super) fn renumber_pastes(&mut self) {
        const OPEN: &str = "[paste #";
        let mut payloads: Vec<String> = Vec::new();
        let mut rewritten: Vec<String> = Vec::with_capacity(self.lines.len());
        for line in &self.lines {
            let mut out = String::with_capacity(line.len());
            let mut rest = line.as_str();
            while let Some(start) = rest.find(OPEN) {
                out.push_str(&rest[..start]);
                let tail = &rest[start..];
                let digits: String = tail[OPEN.len()..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                let closed = tail.find(']');
                match (digits.parse::<usize>(), closed) {
                    (Ok(id), Some(close)) if id >= 1 && id <= self.pastes.len() => {
                        payloads.push(self.pastes[id - 1].clone());
                        out.push_str(&format!(
                            "{OPEN}{}{}",
                            payloads.len(),
                            &tail[OPEN.len() + digits.len()..=close]
                        ));
                        rest = &tail[close + 1..];
                    }
                    _ => {
                        // Not a marker this editor can account for: keep it.
                        out.push('[');
                        rest = &tail[1..];
                    }
                }
            }
            out.push_str(rest);
            rewritten.push(out);
        }
        self.lines = rewritten;
        self.pastes = payloads;
    }

    /// The chip spanning `col` on `line`, as `(start, end)`, when the caret is
    /// strictly inside or exactly at either edge of one (gh #381 gap 1).
    ///
    /// `None` outside a chip. This is the one place the editor asks
    /// `engine::word::atomic_span_at` - the predicate gh #383 added for word
    /// motion - so plain motion, word motion and deletion cannot disagree about
    /// what a unit is.
    pub(super) fn chip_span(line: &str, col: usize) -> Option<(usize, usize)> {
        let chars: Vec<char> = line.chars().collect();
        let col = col.min(chars.len());
        if let Some(end) = crate::engine::word::atomic_span_at(&chars, col) {
            return Some((col, end)); // the caret sits at a chip's start
        }
        if let Some(start) = crate::engine::word::atomic_start_ending_at(&chars, col) {
            return Some((start, col)); // at a chip's end
        }
        // Strictly inside: the chip whose span contains the column. The scan must
        // cover every start position, not only those before `col`: a chip opens at
        // `[`, and the caret may sit anywhere after it, so stopping the search early
        // finds nothing and the delete falls back to cluster-wise (the orphan bug).
        (0..col)
            .filter(|start| chars.get(*start) == Some(&'['))
            .find_map(|start| {
                crate::engine::word::atomic_span_at(&chars, start)
                    .filter(|end| col < *end)
                    .map(|end| (start, end))
            })
    }

    /// Replace the buffer **and** its paste registry (gh #381 gap 3).
    ///
    /// The companion `set_text` needs. A caller that swaps the whole buffer while
    /// keeping its chips alive - a tab switch restoring a saved draft, whose text
    /// holds markers whose payloads live only in the editor's registry - has to hand
    /// the payloads over, because the text alone cannot reconstruct them.
    ///
    /// `set_text` alone remains right for a *replacement* (`/clear`): it re-derives
    /// the registry from the new text, so nothing dead survives.
    pub fn set_text_with_pastes(&mut self, text: &str, pastes: Vec<String>) {
        self.set_text(text);
        // Only the payloads the text can still reach - a stale registry would let the
        // next marker reuse an id whose content does not belong to it.
        self.pastes = retain_live_payloads(&self.text(), &pastes);
    }

    /// The live paste payloads, for a caller about to save this buffer's text and
    /// restore it later (gh #381 gap 3). Pairs with [`Editor::set_text_with_pastes`].
    #[must_use]
    pub fn pastes(&self) -> &[String] {
        &self.pastes
    }

    pub(super) fn insert_paste(&mut self, content: &str) {
        // The shared primitive does the tmux CSI-u decode and the
        // CRLF/tab/non-printable normalization (TUI cycle 7, R1) so the
        // editor and every single-line field agree on what a paste is.
        let mut filtered = crate::widgets::paste::normalize(content);
        // A pasted path after a word character gets a leading space.
        if filtered.starts_with(['/', '~', '.']) {
            let before = self.text_before_cursor();
            if before
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                filtered.insert(0, ' ');
            }
        }
        let lines = filtered.split('\n').count();
        if lines > 10 || filtered.chars().count() > 1000 {
            // The registry can change here, so this step carries it (gh #381 gap 3).
            // The entry must hold the registry as it was **before** this paste - the
            // pre-paste `lines` and the pre-paste payloads - so undo removes both the
            // marker and the payload it introduced. (Snapshotting after the push
            // recorded the payload the undo was supposed to remove, which read as
            // "the snapshot carries the payload" and undid nothing.)
            let lines_before = self.lines.clone();
            let (line_before, col_before) = (self.cursor_line, self.cursor_col);
            let pastes_before = self.pastes.clone();
            // pi's `pasteCounter`: a monotone high-water mark, never `len() + 1`.
            // Deriving the id from the registry length made undo and delete renumber
            // the counter as a side effect, so a paste after an undo could take an id
            // a restored chip already holds.
            // Labels are **positions**, not a monotone counter. A counter cannot be
            // both unique across undo and contiguous after a delete: rewinding it
            // collides with a restored chip, and not rewinding it yields `#1,#3`.
            // So the label is the next free slot, and `renumber_pastes` maps the live
            // order onto `1..n` (compacting the registry with it) after every change -
            // which is what makes a restored or newly-pasted chip unique by
            // construction rather than by a counter's bookkeeping.
            let id = self.pastes.len() + 1;
            let marker = if lines > 10 {
                format!("[paste #{id} +{lines} lines]")
            } else {
                format!("[paste #{id} {} chars]", filtered.chars().count())
            };
            self.pastes.push(filtered);
            self.undo.push(UndoEntry {
                lines: lines_before,
                cursor_line: line_before,
                cursor_col: col_before,
                chips: Some(pastes_before),
            });
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.insert_str_no_snapshot(&marker);
            // Labels are **positions**, so they stay contiguous: `#1,#3` reads as
            // corruption, and `pastes[id - 1]` is positional. The counter's job is
            // uniqueness across undo, not labelling - `renumber_pastes` maps the live
            // order onto `1..n` and compacts the registry with it.
            self.renumber_pastes();
        } else {
            self.insert_str(&filtered);
        }
    }

    /// The char index of the previous grapheme-cluster boundary strictly
    /// before `col` (pi's grapheme-snapped movement: clusters are stepped over
    /// whole, never entered).
    pub(super) fn prev_cluster_boundary(line: &str, col: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation as _;
        let mut boundary = 0usize;
        let mut pos = 0usize;
        for cluster in line.graphemes(true) {
            let next = pos + cluster.chars().count();
            if next >= col {
                break;
            }
            boundary = next;
            pos = next;
        }
        boundary
    }

    /// The char index of the next grapheme-cluster boundary after `col`.
    pub(super) fn next_cluster_boundary(line: &str, col: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation as _;
        let mut pos = 0usize;
        for cluster in line.graphemes(true) {
            pos += cluster.chars().count();
            if pos > col {
                return pos;
            }
        }
        pos
    }

    /// Whether the caret sits on a chip in the current line (gh #381 gap 3).
    ///
    /// Used to decide whether an edit step needs to snapshot the registry: only a
    /// step that can change `pastes` pays for carrying it.
    pub(super) fn chip_at_cursor(&self) -> bool {
        Self::chip_span(self.current(), self.cursor_col).is_some()
    }

    /// The registry bytes held by the undo stack, for the memory row (gh #381 gap 3).
    #[cfg(test)]
    pub(super) fn undo_payload_bytes(&self) -> usize {
        self.undo.iter().map(UndoEntry::payload_bytes).sum()
    }

    /// The column one `Left` lands on from `col` (gh #381 gap 1).
    ///
    /// A paste chip is **one unit**: `[paste #1 +123 lines]` is 21 characters, so
    /// grapheme-wise motion needed 21 presses to leave it, and `Up` could park the
    /// caret in the middle of one. The grapheme snap still applies outside a chip,
    /// so combined emoji keep working.
    pub(super) fn atomic_boundary_left(line: &str, col: usize) -> usize {
        match Self::chip_span(line, col) {
            Some((start, _)) => start,
            None => Self::prev_cluster_boundary(line, col),
        }
    }

    /// The column one `Right` lands on from `col` (gh #381 gap 1).
    pub(super) fn atomic_boundary_right(line: &str, col: usize) -> usize {
        match Self::chip_span(line, col) {
            Some((start, end)) if col == start => end,
            Some((_, end)) => end,
            None => Self::next_cluster_boundary(line, col),
        }
    }

    /// Snap a column that would land inside a chip out to its start (gh #381 gap 1).
    ///
    /// Used by vertical motion, where landing in the middle would put the caret on
    /// text the user never typed. `preferred_col` still holds the column the user
    /// aimed at, so moving back restores it (pi's `snappedFromCursorCol`).
    pub(super) fn snap_out_of_chip(line: &str, col: usize) -> usize {
        match Self::chip_span(line, col) {
            Some((start, end)) if col > start && col < end => start,
            _ => col,
        }
    }
}
