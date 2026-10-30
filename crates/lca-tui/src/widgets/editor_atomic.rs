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
