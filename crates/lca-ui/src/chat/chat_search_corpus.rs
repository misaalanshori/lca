//! Gh #376 gap 1: the **wrap-insensitive search corpus** (FR-UI-12, pi's
//! `buildSearchCorpus` in `alt-screen-search.ts`).
//!
//! The search used to be `strip_ansi(line).to_lowercase().contains(query)` **per
//! rendered row**, so a phrase broken across a wrap boundary was zero hits — and a
//! narrow pane or CJK makes that the common case rather than an edge. The fix is
//! pi's: flatten the document into one linear string of non-space runs joined by a
//! single space, remember where each run came from, and map a linear match back to
//! the (row, column) spans it covers. One pass then finds a phrase however the pane
//! happened to wrap.
//!
//! **Columns are measured with `visible_width`, never byte length.** A run is sliced
//! back into its prefix and measured, so a wide glyph contributes two columns and a
//! match that ends inside CJK still reports the right one — the rule that has already
//! bitten this tree once (#384's table measurement).
//!
//! **Case folding is per character with an index map**, not `str::to_lowercase` on the
//! whole document: `İ` folds to *two* characters, so a whole-string fold shifts every
//! byte offset after it, which is exactly why the old highlighter gave up on such a
//! line (`if lower.len() != line_chars.len() { return line.to_string() }`). Folding
//! each character separately and carrying the original index keeps the mapping exact
//! and needs no dependency.

use lca_tui::engine::text::{strip_terminal_sequences, visible_width};

/// One non-space run of one rendered row, and where it landed in the linear text.
pub(crate) struct Span {
    /// Byte range of this run inside [`Corpus::text`].
    text: std::ops::Range<usize>,
    /// The document row the run came from.
    row: usize,
    /// The column the run starts at, in cells.
    start_col: usize,
    /// **Columns are linear with the bytes** (pi's `linearColumns`): the run is pure
    /// printable ASCII, so a byte offset inside it *is* a cell offset and the mapper
    /// needs no measurement. Measured, not assumed - see `build`'s note.
    linear_columns: bool,
}

/// The linear, wrap-insensitive view of a document.
pub(crate) struct Corpus {
    /// Non-space runs joined by a single space, row after row.
    text: String,
    spans: Vec<Span>,
}

/// One row's share of a match: the columns to underline on that row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Segment {
    /// Document (or slice) row.
    pub(crate) row: usize,
    /// First column of the match on that row, in cells.
    pub(crate) start_col: usize,
    /// One past the last column.
    pub(crate) end_col: usize,
}

/// A match, as the rows it paints on (pi's `AltScreenSearchMatch`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchMatch {
    /// One entry per row the match covers; a wrap-spanning match has more than one.
    pub(crate) segments: Vec<Segment>,
}

impl SearchMatch {
    /// pi's `getAltScreenSearchMatchKey`: `first.row:first.col:last.row:last.end`.
    ///
    /// The identity the selection is kept by. A match is *derived* from the document
    /// every frame, so the selection cannot be held as an index into a list that is
    /// rebuilt — the key survives a re-wrap, a stream, and a re-query.
    pub(crate) fn key(&self) -> (usize, usize, usize, usize) {
        let first = self.segments.first().copied().unwrap_or(Segment {
            row: 0,
            start_col: 0,
            end_col: 0,
        });
        let last = self.segments.last().copied().unwrap_or(first);
        (first.row, first.start_col, last.row, last.end_col)
    }

    /// The row the match starts on, which is what navigation and revealing use.
    pub(crate) fn first_row(&self) -> usize {
        self.segments.first().map_or(0, |segment| segment.row)
    }

    /// The row the match ends on.
    pub(crate) fn last_row(&self) -> usize {
        self.segments.last().map_or(0, |segment| segment.row)
    }
}

/// pi's `normalizeQuery`: whitespace runs collapse to one space, then trim.
///
/// The corpus joins runs with a single space, so `"error   handling"` typed by hand
/// must match the same thing as `"error handling"` — otherwise the normalization
/// would be a rule the corpus does not share.
pub(crate) fn normalize_query(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Flatten `lines` into a corpus; `base_row` is the document row of `lines[0]`.
///
/// The paint path passes the visible window and its first document row, so a key
/// computed from the whole document still matches a match found in the window.
pub(crate) fn build(lines: &[String], base_row: usize) -> Corpus {
    let mut text = String::new();
    let mut spans = Vec::new();
    let mut pending_separator = false;
    for (offset, line) in lines.iter().enumerate() {
        let plain = strip_terminal_sequences(line);
        // **The measurement that decided the shape of this loop.** Walking each
        // character and calling `visible_width` on a one-glyph string cost **8.9 s**
        // for a 2,000-row document (100 k calls, each an allocation plus a cache
        // probe) - far past a frame. pi's `charCodeAt` fast path exists for the same
        // reason in JS (`Intl.Segmenter` is slow); here the cheap discriminator is
        // **ASCII**, where a byte offset inside a run *is* a cell offset, so the run
        // needs no per-character measurement at all. Non-ASCII lines still measure
        // (CJK is double-width), but once per line rather than once per glyph.
        let ascii = plain.is_ascii();
        let mut column = 0usize;
        let mut run_start: Option<(usize, usize)> = None;
        for (byte, ch) in plain.char_indices() {
            if ch.is_whitespace() {
                if let Some((start_byte, start_col)) = run_start.take() {
                    push_run(
                        &mut text,
                        &mut spans,
                        &mut pending_separator,
                        &plain[start_byte..byte],
                        base_row + offset,
                        start_col,
                        ascii,
                    );
                }
                column += if ascii {
                    1
                } else {
                    visible_width(&ch.to_string())
                };
                continue;
            }
            if run_start.is_none() {
                run_start = Some((byte, column));
            }
            column += if ascii {
                1
            } else {
                visible_width(&ch.to_string())
            };
        }
        if let Some((start_byte, start_col)) = run_start {
            push_run(
                &mut text,
                &mut spans,
                &mut pending_separator,
                &plain[start_byte..],
                base_row + offset,
                start_col,
                ascii,
            );
        }
    }
    Corpus { text, spans }
}

fn push_run(
    text: &mut String,
    spans: &mut Vec<Span>,
    pending_separator: &mut bool,
    run: &str,
    row: usize,
    start_col: usize,
    linear_columns: bool,
) {
    if run.is_empty() {
        return;
    }
    if *pending_separator && !text.is_empty() {
        text.push(' ');
    }
    *pending_separator = false;
    let start = text.len();
    text.push_str(run);
    spans.push(Span {
        text: start..text.len(),
        row,
        start_col,
        linear_columns,
    });
    *pending_separator = true;
}

/// Find every occurrence of `query` in `corpus`, as per-row segments.
///
/// The needle and the haystack are both folded **per character**, with the haystack
/// carrying each folded character's original index, so a fold that expands one
/// character into two still maps back to the character it came from.
pub(crate) fn find(corpus: &Corpus, query: &str) -> Vec<SearchMatch> {
    let needle = normalize_query(query);
    if needle.is_empty() || corpus.text.is_empty() {
        return Vec::new();
    }
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let (hay, origin) = fold_with_origin(&corpus.text);
    // **Byte offsets, computed once.** Characters are the unit the fold works in, but a
    // span is a byte range; converting per match by walking the document from the start
    // measured 8.3 s for 2,000 matches on a 100 k-character corpus (O(matches × length)).
    // One pass over the text gives every character's byte offset, so the conversion is
    // an index.
    let byte_of_char = byte_offsets(&corpus.text);
    let mut matches = Vec::new();
    let mut span_cursor = 0usize;
    let mut at = 0usize;
    while at + needle.len() <= hay.len() {
        if hay[at..at + needle.len()] == needle[..] {
            let start_char = origin[at];
            let end_char = origin[at + needle.len() - 1] + 1;
            let start = byte_of_char[start_char];
            let end = byte_of_char
                .get(end_char)
                .copied()
                .unwrap_or(corpus.text.len());
            if let Some(segments) = segments_for(corpus, start..end, &mut span_cursor) {
                matches.push(SearchMatch { segments });
            }
            at += needle.len().max(1);
        } else {
            at += 1;
        }
    }
    matches
}

/// Every character's byte offset in `text`, plus one past the end (pi's monotonic
/// cursor equivalent: one pass, then O(1) lookups).
fn byte_offsets(text: &str) -> Vec<usize> {
    let mut offsets: Vec<usize> = text.char_indices().map(|(byte, _)| byte).collect();
    offsets.push(text.len());
    offsets
}

/// The folded characters of `text`, and, for each, the index of the character it
/// came from.
fn fold_with_origin(text: &str) -> (Vec<char>, Vec<usize>) {
    let mut folded = Vec::new();
    let mut origin = Vec::new();
    for (index, ch) in text.chars().enumerate() {
        for lower in ch.to_lowercase() {
            folded.push(lower);
            origin.push(index);
        }
    }
    (folded, origin)
}

/// Map a linear byte range to the per-row segments it paints.
///
/// A partially covered span reports the columns of the covered **text**, measured by
/// re-slicing the run's prefix through `visible_width` - so the start of a match
/// inside CJK lands on the cell the glyph actually occupies.
fn segments_for(
    corpus: &Corpus,
    range: std::ops::Range<usize>,
    span_cursor: &mut usize,
) -> Option<Vec<Segment>> {
    let mut segments: Vec<Segment> = Vec::new();
    // pi's monotonic span cursor: matches arrive in ascending order, so the scan
    // resumes where the previous one stopped instead of re-walking from row 0.
    while *span_cursor < corpus.spans.len() && corpus.spans[*span_cursor].text.end <= range.start {
        *span_cursor += 1;
    }
    for span in &corpus.spans[*span_cursor..] {
        if span.text.start >= range.end {
            break;
        }
        let local_start = range.start.max(span.text.start) - span.text.start;
        let local_end = range.end.min(span.text.end) - span.text.start;
        // A linear-column span needs no measurement: `local_*` are byte offsets into an
        // ASCII run, so they are already cell offsets. The grapheme path measures the
        // prefix, which is where a wide glyph's two cells enter.
        let (start_off, end_off) = if span.linear_columns {
            (local_start, local_end)
        } else {
            let run = &corpus.text[span.text.clone()];
            (
                visible_width(&prefix_by_chars(run, local_start)),
                visible_width(&prefix_by_chars(run, local_end)),
            )
        };
        let start_col = span.start_col + start_off;
        let end_col = span.start_col + end_off;
        let (row, start_col, end_col) = (span.row, start_col, end_col);
        match segments.last_mut() {
            // Adjacent pieces on the same row merge (pi's merge rule): a match that
            // crosses a wrap boundary yields one segment per row, and two pieces of
            // the same row are one underline.
            Some(previous) if previous.row == row && start_col <= previous.end_col => {
                previous.end_col = previous.end_col.max(end_col);
            }
            _ => segments.push(Segment {
                row,
                start_col,
                end_col,
            }),
        }
    }
    (!segments.is_empty()).then_some(segments)
}

/// The first `bytes` bytes of `run`, cut on a character boundary.
fn prefix_by_chars(run: &str, bytes: usize) -> String {
    let end = run
        .char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(run.len()))
        .take_while(|&offset| offset <= bytes)
        .last()
        .unwrap_or(0);
    run[..end].to_string()
}
