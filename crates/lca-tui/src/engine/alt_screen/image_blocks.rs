//! Widen a set of touched rows so every kitty image block is painted whole
//! (gh #324 gap 1).
//!
//! Split out of `alt_screen.rs` for gate 11's 1,200-line ceiling; the renderer
//! calls it from its diff loop and the rows beside it pin the behaviour.

/// Widen a set of touched rows so every kitty image block is painted whole
/// (gh #324 gap 1).
///
/// pi's `expandChangedRangeForKittyImages`, narrowed to a row set: if any
/// reserved row of a block is being repainted, the block's header row has to be
/// among them, because the header is the only row carrying the sequence - a
/// repaint of the blanks alone paints nothing, and a repaint of the header alone
/// erases the block's own reserved rows. Returns `None` when no block is involved,
/// so a text-only frame keeps its exact row set and its pinned byte shapes.
pub(super) fn expand_for_image_blocks(lines: &[String], touched: &[usize]) -> Option<Vec<usize>> {
    let mut expanded = touched.to_vec();
    let mut changed = false;
    for (row, line) in lines.iter().enumerate() {
        let Some(reserved) = crate::widgets::image::kitty_reserved_rows(line) else {
            continue;
        };
        let reserved = (reserved as usize).max(1);
        let end = (row + reserved - 1).min(lines.len().saturating_sub(1));
        if !(row..=end).any(|r| touched.contains(&r)) {
            continue;
        }
        // The reserve's own rows are blank, so they never appear in `touched`
        // from a content diff; add them all, header first.
        for r in row..=end {
            if !expanded.contains(&r) {
                expanded.push(r);
                changed = true;
            }
        }
    }
    if !changed {
        return None;
    }
    expanded.sort_unstable();
    expanded.dedup();
    Some(expanded)
}
