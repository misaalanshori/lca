//! Row measurement on a rendered transcript (gh #403's hit map, gh #23's prompt
//! offsets): split from `mod.rs` under the 1,200-line ceiling when gh #399's header
//! entry grew that file past it.
//!
//! These are `impl Transcript` blocks rather than free functions because they read
//! the cached render, and the cache is private to the transcript.

use super::*;

impl Transcript {
    /// The entry owning a document row, if any, and whether the row
    /// belongs to its reasoning run (gh #11's hit map): blank separator
    /// rows belong to no entry. The render cache carries each entry's
    /// line count, so this walks lengths, never re-renders.
    /// What a transcript document row belongs to (gh #166): a
    /// thinking row cycles its run, a tool card's first row toggles its
    /// expansion, anything else is not clickable.
    pub fn entry_at_row(&self, width: u16, theme: &Theme, row: usize) -> Option<EntryHit> {
        let _ = self.render(width, theme);
        let cache = self.cache.borrow();
        let mut line = 0usize;
        for (i, slot) in cache.iter().enumerate() {
            if i > 0 {
                if line == row {
                    return None;
                }
                line += 1;
            }
            let Some((_, _, lines, thinking_rows)) = slot.as_ref() else {
                continue;
            };
            if row >= line && row < line + lines.len() {
                if row - line < *thinking_rows {
                    return Some(EntryHit::Thinking(i));
                }
                // A tool card paints as a state band: a blank band row,
                // then the header, then the output (`render_tool` owns
                // this shape, so the header is always row 1). Body rows
                // hit the card too (gh #247), so a click anywhere on
                // an expanded card collapses it.
                if matches!(self.entries.get(i), Some(Entry::Tool { .. })) {
                    if row - line == 1 {
                        return Some(EntryHit::ToolHeader(i));
                    }
                    return Some(EntryHit::ToolBody(i));
                }
                // Gh #393: the whole compaction card is the target, because the
                // card is the thing being opened.
                if matches!(self.entries.get(i), Some(Entry::Compaction { .. })) {
                    return Some(EntryHit::Compaction(i));
                }
                return None;
            }
            line += lines.len();
        }
        None
    }

    /// The document line index where each user message starts, at `width`
    /// (FR-UI-11's prompt jump).
    /// The document line index where each user message starts, at `width`
    /// (FR-UI-11's prompt jump).
    pub fn user_offsets(&self, width: u16, theme: &Theme) -> Vec<usize> {
        let mut offsets = Vec::new();
        let mut line = 0usize;
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                line += 1;
            }
            if matches!(entry, Entry::User(_)) {
                offsets.push(line);
            }
            let mut tmp = Vec::new();
            let _ = render_entry(
                entry,
                width,
                theme,
                self.tools_expanded,
                self.thinking,
                self.codeblock_border,
                &self.markdown_transformers,
                &self.tuning,
                self.keybindings.as_ref(),
                &mut tmp,
            );
            line += tmp.len();
        }
        offsets
    }

    /// The height (line count) at a width.
    /// The height (line count) at a width.
    pub fn height(&self, width: u16, theme: &Theme) -> usize {
        self.render(width, theme).len()
    }
}

/// A one-line image label (media type, dimensions, size) for resume lines.
pub fn image_label(media_type: &str, bytes: &[u8]) -> String {
    let info = ImageInfo::new(media_type, bytes);
    let dimensions = match (info.width, info.height) {
        (Some(w), Some(h)) => format!("{w}×{h}"),
        _ => "unknown size".to_string(),
    };
    format!("[image {media_type}, {dimensions}, {} bytes]", bytes.len())
}
