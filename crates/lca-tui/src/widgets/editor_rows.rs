//! The editor's visual-row geometry: pi's `wordWrapLine` +
//! `buildVisualLineMap` (`pi-tui-re/src_re/tui-widgets/editor.md` §3).
//!
//! One geometry, two consumers: [`crate::widgets::editor::Editor::render`]
//! draws these rows and the vertical movements walk them, which is the
//! constraint issues #27 and #28 turn on — navigation that computes its own
//! wrapping drifts from what is on screen and Up becomes Home again.
//!
//! Two differences from the transcript's wrapper (`engine::text::
//! wrap_text_with_ansi`), both pi's:
//!
//! - **Trailing whitespace is kept.** The transcript trims its rows ("line
//!   trailing whitespace can cause lines to exceed the requested width");
//!   the editor's rows are the buffer the user typed, and a space is a cell
//!   that holds what follows (gh #27). A break sits *after* the whitespace,
//!   so the continuation starts at the next word instead of eating it.
//! - **Spans, not just text.** Each row carries the char range it draws, so
//!   the caret's position in the buffer and the row it is painted on are
//!   computed from the same numbers (gh #28's visual-row navigation).

use unicode_segmentation::UnicodeSegmentation as _;

use crate::engine::text::visible_width;

/// One visual row of one logical line: the char range it draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualRow {
    /// Which logical line the row continues.
    pub line: usize,
    /// The row's first char index within that line.
    pub start: usize,
    /// One past the row's last char index within that line.
    pub end: usize,
}

impl VisualRow {
    /// The number of chars this row draws.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether the row draws no characters (an empty logical line).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The row's own text within `line`.
    pub fn text<'a>(&self, line: &'a str) -> &'a str {
        slice_chars(line, self.start, self.end)
    }
}

/// The chars `start..end` of `line`, by char index (the buffer's cursor is
/// char-indexed, so the spans are too).
pub fn slice_chars(line: &str, start: usize, end: usize) -> &str {
    let Some(start_byte) = line.char_indices().nth(start).map(|(byte, _)| byte) else {
        return "";
    };
    let end_byte = line
        .char_indices()
        .nth(end)
        .map(|(byte, _)| byte)
        .unwrap_or(line.len());
    if end_byte < start_byte {
        return "";
    }
    &line[start_byte..end_byte]
}

/// Word-wrap one logical line into char spans (pi's `wordWrapLine`).
///
/// Breaks at the last whitespace before the overflow — after the whitespace,
/// so the spaces stay in the row they filled — and force-breaks where no
/// opportunity fits (a word longer than the row, CJK, a wide glyph).
/// `width == 0` (no render has happened yet) means "do not wrap": one row
/// per logical line, which is what the buffer already is.
pub fn wrap_row(line: &str, width: usize) -> Vec<(usize, usize)> {
    let total = line.chars().count();
    if width == 0 || visible_width(line) <= width {
        return vec![(0, total)];
    }

    let mut graphemes: Vec<(usize, &str)> = Vec::new();
    let mut char_index = 0usize;
    for (_, grapheme) in line.grapheme_indices(true) {
        graphemes.push((char_index, grapheme));
        char_index += grapheme.chars().count();
    }

    let mut rows: Vec<(usize, usize)> = Vec::new();
    let mut chunk_start = 0usize;
    let mut current_width = 0usize;
    // (char index the break falls at, row width up to and including the
    // whitespace before it) - pi's `wrapOppIndex`/`wrapOppWidth`.
    let mut opportunity: Option<(usize, usize)> = None;

    for (position, (start, grapheme)) in graphemes.iter().enumerate() {
        let grapheme_width = visible_width(grapheme);

        if current_width + grapheme_width > width {
            if let Some((break_at, break_width)) = opportunity
                && current_width - break_width + grapheme_width <= width
            {
                rows.push((chunk_start, break_at));
                chunk_start = break_at;
                current_width -= break_width;
            } else if chunk_start < *start {
                // No viable opportunity: force-break where we are.
                rows.push((chunk_start, *start));
                chunk_start = *start;
                current_width = 0;
            }
            opportunity = None;
        }

        current_width += grapheme_width;

        // A wrap opportunity: whitespace immediately followed by a
        // non-whitespace grapheme (multiple spaces join; the break falls
        // after the last one, at the start of the next word).
        let is_whitespace = !grapheme.is_empty() && grapheme.trim().is_empty();
        if is_whitespace
            && let Some((next_start, next)) = graphemes.get(position + 1)
            && !next.trim().is_empty()
        {
            opportunity = Some((*next_start, current_width));
        }
    }
    rows.push((chunk_start, total));
    rows
}

/// The visual-row map of a whole buffer (pi's `buildVisualLineMap`). An
/// empty logical line still takes one row.
pub fn visual_rows(lines: &[String], width: usize) -> Vec<VisualRow> {
    let mut rows = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        for (start, end) in wrap_row(line, width) {
            rows.push(VisualRow {
                line: index,
                start,
                end,
            });
        }
    }
    rows
}

/// Whether `index` is the last visual row of its logical line.
pub fn is_last_row_of_line(rows: &[VisualRow], index: usize) -> bool {
    rows.get(index + 1)
        .is_none_or(|next| next.line != rows[index].line)
}

/// The visual row holding a logical position (pi's `findVisualLineAt`):
/// the row whose span contains `col`, with the last row of a line also
/// accepting its own end position — the caret sits after the last char.
pub fn find_visual_row(rows: &[VisualRow], line: usize, col: usize) -> usize {
    for (index, row) in rows.iter().enumerate() {
        if row.line != line || col < row.start {
            continue;
        }
        let offset = col - row.start;
        if offset < row.len() || (is_last_row_of_line(rows, index) && offset == row.len()) {
            return index;
        }
    }
    rows.len().saturating_sub(1)
}
