//! The transcript's side of kitty image cropping and placement (gh #324 gaps 1, 3).
//!
//! Split out of `mod.rs` for gate 11's 1,200-line ceiling. Both halves answer a question
//! the renderer cannot ask the emitted line: **where** an image block is in the document,
//! and what its picture's true pixel height is - neither is in the kitty sequence.

use super::*;

/// How far a rendered transcript has been post-processed (gh #324 gap 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStage {
    /// Entries rendered, image blocks still in their transmission form. The stage a
    /// caller that needs to rewrite a header (the window crop) must ask for.
    BeforePlacements,
    /// The whole pipeline, including the kitty placement cache.
    Final,
}

impl Transcript {
    /// Apply the kitty placement cache to an already-rendered document (gh #324 gap 3).
    ///
    /// The window crop runs between the render and this call, so a header it rewrote
    /// is already a placement and this leaves it alone; an untouched header still gets
    /// rewritten from a transmission to a placement, which is gap 1's property.
    pub fn apply_placements(&self, rows: &mut [String]) {
        self.uploaded_images.borrow_mut().substitute(rows);
    }

    /// The document row of every image block's header, with its picture's pixel
    /// height (gh #324 gap 3).
    ///
    /// The renderer that crops a block to a window needs two things the emitted line
    /// does not carry: **where** the block is, and the picture's real pixel height. The
    /// line carries the cell box; the pixel height lives here, on the `ImageInfo` the
    /// entry was pushed with. Deriving the height from the grid instead would put the
    /// slice boundaries up to a cell row off, so the height is answered from the entry
    /// rather than reconstructed.
    ///
    /// The row is computed by walking the same cached line counts [`Self::render`]
    /// produced, so this cannot disagree with the document it describes. Cheap: it
    /// only walks lengths, and only for entries that are images.
    #[must_use]
    pub fn image_blocks(&self) -> std::collections::HashMap<usize, u32> {
        let mut heights = std::collections::HashMap::new();
        // **No `self.render(..)` here.** It looks like a cheap cache warm, but `render`
        // builds and returns the whole document - a clone of every row, per frame - just
        // to read lengths this function can read from the cache directly. That clone
        // measured as a real cost: it made a real-terminal row flaky under load. The
        // caller has already rendered, so the cache is warm; a cold cache simply reports
        // no image rows, which is the correct answer for a transcript nobody has painted.
        let cache = self.cache.borrow();
        let mut line = 0usize;
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                line += 1;
            }
            let Some((_, _, lines, _)) = cache.get(i).and_then(|slot| slot.as_ref()) else {
                continue;
            };
            if let Entry::Image { info, .. } = entry
                && let Some(height) = info.height
                && height > 0
            {
                // The header is the entry's first row, which is where the kitty
                // sequence is painted (gap 1).
                heights.insert(line, height);
            }
            line += lines.len();
        }
        heights
    }
    /// The render-cache epoch (gh #324 gap 2).
    ///
    /// Exposed to tests only: the cell-size probe's whole claim is that it drops the
    /// frames that were laid out against the estimate, and "the cache was
    /// invalidated" is not observable from the rendered rows when the new grid
    /// happens to produce the same text. Reading the epoch is the direct question.
    #[cfg(test)]
    #[must_use]
    pub fn cache_epoch_for_test(&self) -> u64 {
        self.cache_epoch
    }
}
