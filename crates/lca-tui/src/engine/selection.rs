//! Application-owned text selection, ported from pi's
//! `tui-alt-screen.ts` selection subsystem
//! (`pi-tui-re/src_re/tui-widgets/tui-alt-screen.md` §4).
//!
//! Selection lives in **content** coordinates (row/column in the rendered
//! document), not screen coordinates, so it survives scrolling. Granularity
//! is chosen by click count: 1 = character, 2 = word (paths and kebab-case
//! kept whole), 3 = line. Copy-on-release with honest OSC 52 reporting is
//! the caller's job; this module produces the text and the highlight.

use super::text::{extract_ansi_code, slice_by_column, strip_terminal_sequences, visible_width};

/// Selection granularity, by click count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Granularity {
    /// One character.
    #[default]
    Char,
    /// One word (joiners kept whole).
    Word,
    /// One whole line.
    Line,
}

impl Granularity {
    /// From a click count (1, 2, 3; clamped).
    pub fn from_click_count(count: u8) -> Self {
        match count {
            2 => Granularity::Word,
            3 => Granularity::Line,
            _ => Granularity::Char,
        }
    }
}
/// A point in content coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionPoint {
    /// Row.
    pub row: u16,
    /// Column (visible).
    pub col: u16,
}

/// The word or line a press grabbed, remembered for the whole drag (gh #374
/// gap 3): pi's `selectionInitialRange`.
///
/// The anchor word is the **reference frame** of a word/line drag: both a
/// forward and a backward drag have to keep it inside the normalized range, so
/// its columns have to be remembered rather than recomputed. Re-running word
/// detection on the drag's current line cannot reproduce them - which is exactly
/// what made the drag's direction matter, and what deselected the word the user
/// double-clicked.
///
/// The row is part of the range for the same reason on the vertical axis: a drag
/// up has to be recognised as "before" even when its column is larger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AnchorRange {
    /// The row the range is on.
    row: u16,
    /// First column, inclusive.
    start: u16,
    /// Last column, exclusive.
    end: u16,
}

/// The active selection state.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Where the selection started.
    pub anchor: Option<SelectionPoint>,
    /// Where the selection currently ends.
    pub focus: Option<SelectionPoint>,
    /// The granularity.
    pub granularity: Granularity,
    /// The word/line range the press grabbed, and the row it is on: the
    /// reference frame both drag directions are measured against.
    anchor_range: Option<AnchorRange>,
    /// Whether a press is still unreleased — the gesture is **in progress**.
    ///
    /// Not "is anything selected" and not "did the pointer move": a press that
    /// has not been released yet is the one state that means an interrupted
    /// gesture, whose highlight has no coherent meaning to keep. A finished
    /// selection is the opposite - it is exactly what the user is about to
    /// paste - so this is the predicate a focus loss must ask (gh #374 gap 2).
    ///
    /// pi keeps the same state as `selectionPressActive`
    /// (`tui-alt-screen.ts:226`); it is set by [`Self::start`] and cleared by
    /// [`Self::end`] (the mouse release) as well as by [`Self::clear`], so it
    /// answers both ways without focus loss being involved - which is what makes
    /// a guard on it live code rather than a branch that can never be false.
    pub press_active: bool,
    /// The click count that started it.
    pub click_count: u8,
    /// The painted scrollbar's `(column, rows)` when one is on the frame
    /// (gh #35): the selection's right edge stops before that column, so
    /// the adornment never reaches the copied text or the highlight.
    scrollbar: Option<(u16, u16)>,
}

/// Whether a character is part of a selectable word. Joiners `/`, `-`, `.`
/// and `~` are kept so paths and kebab-case tokens stay whole (pi's
/// `TERMINAL_WORD_SELECTION_JOINERS`).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '/' || c == '-' || c == '.' || c == '~'
}

fn line_word_range(line: &str, col: u16) -> (u16, u16) {
    // Work in visible columns; strip ANSI first (selection is on visible text).
    let text = strip_terminal_sequences(line);
    let chars: Vec<char> = text.chars().collect();
    let target = col as usize;
    let mut cells: Vec<(usize, usize)> = Vec::with_capacity(chars.len());
    let mut c = 0usize;
    for ch in &chars {
        let w = super::text::grapheme_width(&ch.to_string());
        cells.push((c, c + w));
        c += w;
    }
    let Some(idx) = cells.iter().position(|&(s, e)| target >= s && target < e) else {
        return (col, col);
    };
    if !is_word_char(chars[idx]) {
        return (col, col + 1);
    }
    let mut start = idx;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = idx + 1;
    while end < chars.len() && is_word_char(chars[end]) {
        end += 1;
    }
    (cells[start].0 as u16, cells[end - 1].1 as u16)
}

impl Selection {
    /// A new, empty selection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Tell the selection where a painted virtual scrollbar sits (gh #35):
    /// `(column, rows)` of the frame it was drawn on, `None` when the
    /// frame carries no scrollbar. The selection's right edge stops
    /// before that column on those rows - the hard rule that a copy of
    /// transcript text is exactly the transcript's text.
    pub fn set_scrollbar(&mut self, scrollbar: Option<(u16, u16)>) {
        self.scrollbar = scrollbar;
    }

    /// Begin a selection at a point with the given granularity.
    pub fn start(
        &mut self,
        point: SelectionPoint,
        granularity: Granularity,
        click_count: u8,
        line: &str,
    ) {
        self.granularity = granularity;
        self.click_count = click_count;
        self.press_active = true;
        let range = match granularity {
            Granularity::Word => {
                let (start, end) = line_word_range(line, point.col);
                Some(AnchorRange {
                    row: point.row,
                    start,
                    end,
                })
            }
            Granularity::Line => Some(AnchorRange {
                row: point.row,
                start: 0,
                end: visible_width(line) as u16,
            }),
            Granularity::Char => None,
        };
        self.anchor_range = range;
        match range {
            Some(range) => {
                self.anchor = Some(SelectionPoint {
                    row: range.row,
                    col: range.start,
                });
                self.focus = Some(SelectionPoint {
                    row: range.row,
                    col: range.end,
                });
            }
            None => {
                self.anchor = Some(point);
                self.focus = Some(point);
            }
        }
    }

    /// Update the focus during a drag (gh #374 gap 3: in **both** directions).
    ///
    /// Word and line drags are measured against the remembered range, not
    /// against the drag's current line: the target's own word/line range is
    /// computed, and where it falls relative to the anchor range decides which
    /// end of the anchor range is held.
    ///
    /// - **Before** the anchor range (row first, then column, so a drag up
    ///   counts whatever its column): hold the anchor's *far* end and point the
    ///   focus at the target's start.
    /// - **Otherwise**: hold the anchor's *start* and point the focus at the
    ///   target's end.
    ///
    /// That is pi's `updateSelectionFocus` (`tui-alt-screen.ts:1220`). The
    /// lexicographic comparison is the whole reason the diagonals work: a
    /// column-only test calls a down-left drag "before" and an up-right drag
    /// "after", and the anchor word leaves the range in both.
    ///
    /// Because the endpoints are derived from the stored range, a drag that
    /// reverses back past the anchor restores exactly what a forward-only drag
    /// to the same cell would have produced - there is nothing to re-derive and
    /// so nothing to drift.
    ///
    /// Character granularity has no anchor range and keeps point-to-point
    /// semantics.
    pub fn update(&mut self, point: SelectionPoint, line: &str) {
        if !self.press_active {
            return;
        }
        let Some(initial) = self.anchor_range else {
            self.focus = Some(point);
            return;
        };
        let (target_start, target_end) = match self.granularity {
            Granularity::Line => (0, visible_width(line) as u16),
            // `Char` never has an anchor range, so this arm is the word case.
            Granularity::Word | Granularity::Char => line_word_range(line, point.col),
        };
        let before = (point.row, target_start) < (initial.row, initial.start);
        let (anchor_col, focus) = if before {
            (
                initial.end,
                SelectionPoint {
                    row: point.row,
                    col: target_start,
                },
            )
        } else {
            (
                initial.start,
                SelectionPoint {
                    row: point.row,
                    col: target_end,
                },
            )
        };
        self.anchor = Some(SelectionPoint {
            row: initial.row,
            col: anchor_col,
        });
        self.focus = Some(focus);
    }

    /// End a drag (release): the gesture is finished, and from here on the
    /// highlight is a finished selection rather than an in-progress one.
    pub fn end(&mut self) {
        self.press_active = false;
    }

    /// Clear the selection entirely.
    pub fn clear(&mut self) {
        self.anchor = None;
        self.focus = None;
        self.anchor_range = None;
        self.press_active = false;
        // Gh #403 gap 1: this mirror is written by `start` and read by
        // nothing, but leaving a stale count on a cleared selection invites
        // exactly the bug the reset exists to prevent - a later reader could
        // trust it. The renderer's own `click_count` is the authoritative
        // one; this just stops the copy from disagreeing.
        self.click_count = 0;
    }

    /// Whether anything is selected.
    pub fn is_active(&self) -> bool {
        self.anchor.is_some() && self.focus.is_some()
    }

    /// The normalized (start, end) point pair.
    pub fn range(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let a = self.anchor?;
        let f = self.focus?;
        if (a.row, a.col) <= (f.row, f.col) {
            Some((a, f))
        } else {
            Some((f, a))
        }
    }

    fn columns_for_row(&self, row: u16, line: &str) -> Option<(u16, u16)> {
        let (start, end) = self.range()?;
        let width = visible_width(line) as u16;
        if row < start.row || row > end.row {
            return None;
        }
        let (from, to) = if start.row == end.row {
            (start.col, end.col)
        } else if row == start.row {
            (start.col, width)
        } else if row == end.row {
            (0, end.col)
        } else {
            (0, width)
        };
        // gh #35: on the rows that carry the scrollbar, the range stops
        // where the content stops - the copied text and the highlight
        // read this same range, so the adornment can never leak into
        // either.
        let to = match self.scrollbar {
            Some((column, rows)) if row < rows => to.min(column),
            _ => to,
        };
        Some((from, to))
    }

    /// The selected text across `lines` (ANSI stripped, rows trimmed).
    pub fn active_text(&self, lines: &[String]) -> String {
        let Some((start, end)) = self.range() else {
            return String::new();
        };
        let mut out: Vec<String> = Vec::new();
        for row in start.row..=end.row {
            let Some(line) = lines.get(row as usize) else {
                continue;
            };
            let Some((from, to)) = self.columns_for_row(row, line) else {
                continue;
            };
            let slice = slice_by_column(
                line,
                from as usize,
                (to.saturating_sub(from)) as usize,
                false,
            );
            out.push(strip_terminal_sequences(&slice).trim_end().to_string());
        }
        out.join("\n")
    }

    /// Apply the inverse-video highlight to the selected column ranges.
    pub fn highlight(&self, lines: &mut [String]) {
        let Some((start, end)) = self.range() else {
            return;
        };
        for row in start.row..=end.row {
            let Some(line) = lines.get(row as usize).cloned() else {
                continue;
            };
            let Some((from, to)) = self.columns_for_row(row, &line) else {
                continue;
            };
            if to <= from {
                continue;
            }
            lines[row as usize] = highlight_range(&line, from, to);
        }
    }
}

/// Wrap the `[from, to)` column range of `line` in inverse video, re-arming
/// the reverse bit after any nested SGR so the highlight survives styles.
pub fn highlight_range(line: &str, from: u16, to: u16) -> String {
    let width = (to.saturating_sub(from)) as usize;
    let selected = slice_by_column(line, from as usize, width, false);
    let before = slice_by_column(line, 0, from as usize, false);
    let after = slice_by_column(line, to as usize, 10_000, false);
    let mut armed = String::from("\x1b[7m");
    let mut i = 0;
    while i < selected.len() {
        if let Some((code, len)) = extract_ansi_code(&selected, i) {
            armed.push_str(&code);
            // Re-arm reverse after any SGR that might have cleared it.
            if code.ends_with('m') && code != "\x1b[7m" {
                armed.push_str("\x1b[7m");
            }
            i += len;
        } else {
            let Some(ch) = super::text::char_at(&selected, i) else {
                break;
            };
            armed.push(ch);
            i += ch.len_utf8();
        }
    }
    armed.push_str("\x1b[27m");
    format!("{before}{armed}{after}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<String> {
        vec![
            "hello world foo".to_string(),
            "second line here".to_string(),
            "third /path/to-file.txt".to_string(),
        ]
    }

    // Verifies: gh #35 (the hard rule) - the virtual scrollbar column is
    // an adornment outside the selected width: a selection dragged across
    // it copies exactly the transcript's text, and the inverse highlight
    // stops before its cell instead of inverting it.
    #[test]
    fn the_scrollbar_column_never_reaches_the_copied_text() {
        let mut sel = Selection::new();
        // `hello world` then the scrollbar cell at column 12.
        let rows = vec!["hello world \u{2502}".to_string()];
        sel.start(
            SelectionPoint { row: 0, col: 0 },
            Granularity::Char,
            1,
            &rows[0],
        );
        // Dragged past the end of the row: the range wants column 13.
        sel.update(SelectionPoint { row: 0, col: 13 }, &rows[0]);
        sel.end();
        sel.set_scrollbar(Some((12, 1)));
        assert_eq!(
            sel.active_text(&rows),
            "hello world",
            "the scrollbar cell is not part of the text"
        );

        let mut painted = rows.clone();
        sel.highlight(&mut painted);
        assert!(
            painted[0].ends_with('\u{2502}'),
            "the highlight stops before the scrollbar cell: {:?}",
            painted[0]
        );
        assert!(
            !painted[0].contains("\u{1b}[7m\u{2502}"),
            "the cell itself is never inverted: {:?}",
            painted[0]
        );
    }

    #[test]
    fn char_selection_extracts_a_range() {
        let mut sel = Selection::new();
        sel.start(
            SelectionPoint { row: 0, col: 0 },
            Granularity::Char,
            1,
            "hello world foo",
        );
        sel.update(SelectionPoint { row: 0, col: 5 }, "hello world foo");
        sel.end();
        assert_eq!(sel.active_text(&lines()), "hello");
    }

    #[test]
    fn word_selection_keeps_paths_and_kebab_whole() {
        let mut sel = Selection::new();
        let line = "third /path/to-file.txt";
        sel.start(
            SelectionPoint { row: 2, col: 8 },
            Granularity::Word,
            2,
            line,
        );
        sel.end();
        assert_eq!(sel.active_text(&lines()), "/path/to-file.txt");
    }

    #[test]
    fn line_selection_takes_the_whole_line() {
        let mut sel = Selection::new();
        sel.start(
            SelectionPoint { row: 1, col: 3 },
            Granularity::Line,
            3,
            "second line here",
        );
        sel.end();
        assert_eq!(sel.active_text(&lines()), "second line here");
    }

    #[test]
    fn multi_row_selection_spans_lines() {
        let mut sel = Selection::new();
        sel.start(
            SelectionPoint { row: 0, col: 6 },
            Granularity::Char,
            1,
            "hello world foo",
        );
        sel.update(SelectionPoint { row: 1, col: 6 }, "second line here");
        sel.end();
        assert_eq!(sel.active_text(&lines()), "world foo\nsecond");
    }

    #[test]
    fn backward_drag_normalizes() {
        let mut sel = Selection::new();
        sel.start(
            SelectionPoint { row: 1, col: 6 },
            Granularity::Char,
            1,
            "second line here",
        );
        sel.update(SelectionPoint { row: 0, col: 6 }, "hello world foo");
        sel.end();
        assert_eq!(sel.active_text(&lines()), "world foo\nsecond");
    }

    #[test]
    fn highlight_rearms_reverse_across_nested_sgr() {
        let line = "ab\x1b[31mcd\x1b[0mef";
        let out = highlight_range(line, 0, 6);
        // The reverse bit is re-armed after the inner SGR codes.
        assert!(out.starts_with("\x1b[7m"));
        assert!(out.contains("\x1b[31m\x1b[7m"));
        assert!(out.ends_with("\x1b[27m"));
        assert_eq!(visible_width(&out), 6);
    }

    #[test]
    fn clear_removes_everything() {
        let mut sel = Selection::new();
        sel.start(SelectionPoint { row: 0, col: 0 }, Granularity::Char, 1, "x");
        assert!(sel.is_active());
        sel.clear();
        assert!(!sel.is_active());
        assert_eq!(sel.active_text(&lines()), "");
    }
}

// Gh #374 gap 3's direction matrix, split out for the 1,200-line ceiling.
#[cfg(test)]
#[path = "selection/backward_drag_tests.rs"]
mod backward_drag_tests;
