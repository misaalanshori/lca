//! The changed-range side of inline images (gh #324 gap 4).
//!
//! Split out of `main_screen.rs` for gate 11's 1,200-line ceiling. The design note is on
//! `expand_range_for_images`: the differential renderer picks an interval of document rows
//! to repaint, and an interval that cuts through a multi-row image cannot be honoured -
//! the terminal has no way to redraw part of a texture - so the interval is widened to a
//! union of whole blocks before anything consumes it.

/// The ids a kitty image line carries, and its row count (gh #324 gap 4).
///
/// A **transmission** (`a=T`) and a **placement** (`a=p`) both carry `i=`; the renderer
/// has to read them from either, because the crop (gap 3) rewrites a block into a
/// placement and a placement still names the texture it belongs to.
pub(super) fn kitty_ids_on(line: &str) -> Vec<u32> {
    let Some(start) = line.find("\x1b_G") else {
        return Vec::new();
    };
    let rest = &line[start + 3..];
    let end = match rest.find(';') {
        Some(at) => at,
        None => rest.find("\x1b\\").unwrap_or(rest.len()),
    };
    rest[..end]
        .split(',')
        .filter_map(|param| param.strip_prefix("i="))
        .filter_map(|value| value.parse::<u32>().ok())
        .collect()
}

/// Widen a changed range so it never starts or ends inside a kitty image block
/// (gh #324 gap 4; pi's `expandChangedRangeForKittyImages`).
///
/// The differential renderer picks an interval of document rows to repaint, and an
/// interval that cuts through multi-row image is meaningless: the terminal cannot
/// repaint half a texture, so the untouched half keeps the old drawing (a ghost) and the
/// repainted half shows the new frame. The interval therefore has to be a union of whole
/// blocks.
///
/// # Why both frames are scanned
///
/// A block may exist only in the previous frame (it was replaced or removed) or only in
/// the new one (it just arrived). Scanning just one leaves the other half-cased: the
/// first gives a ghost where an image used to be, the second slices a fresh placement.
///
/// # The condition, and why it is pi's
///
/// A block at `[start, end]` is intersected when `start >= first` (it begins inside or
/// after the range's start) **or** `start <= last && end >= first` (it begins at or
/// before the range's end and reaches into it). The second disjunct is the one a naive
/// `start <= last` gets wrong: a block that ends *before* the range begins is not
/// intersected at all.
///
/// # The frame-budget consequence
///
/// An image spanning most of the viewport pulls almost any range up to its own extent,
/// so a frame that changes a row beside it becomes a near-full repaint. That is the
/// deliberately accepted trade: a full repaint is *correct*, a sliced texture is not,
/// and the frames this costs are the ones already paying for a large image's decode.
/// The expansion is a single pass over each frame with no loop, so it cannot recurse or
/// double-emit - `a_full_viewport_image_neither_loops_nor_double_emits` pins that.
pub(super) fn expand_range_for_images(
    previous: &[String],
    next: &[String],
    first: usize,
    last: usize,
) -> (usize, usize) {
    let mut out_first = first;
    let mut out_last = last;
    for lines in [previous, next] {
        for (start, rows) in blocks_in(lines) {
            let end = start.saturating_add(rows as usize).saturating_sub(1);
            let intersects = start >= first || (start <= last && end >= first);
            if !intersects {
                continue;
            }
            out_first = out_first.min(start);
            out_last = out_last.max(end);
        }
    }
    (out_first, out_last)
}

/// Every kitty image block in a frame: its first row and how many rows it spans.
///
/// The span comes from the line's own `r=`, and is capped by the frame, exactly as pi's
/// `getKittyImageReservedRows` does - a block cannot reserve rows that do not exist.
pub(super) fn blocks_in(lines: &[String]) -> Vec<(usize, u32)> {
    let mut blocks = Vec::new();
    for (row, _line) in lines.iter().enumerate() {
        let Some(rows) = crate::widgets::image::kitty_block_rows(lines, row) else {
            continue;
        };
        if rows <= 1 {
            continue;
        }
        let remaining = lines.len().saturating_sub(row) as u32;
        blocks.push((row, rows.min(remaining).max(1)));
    }
    blocks
}

/// The delete command for a texture whose image is gone (gh #324 gap 4).
///
/// pi's `deleteKittyImage`: `d=I` (uppercase) frees the image data as well as removing
/// it, which is what keeps a long session from accumulating a texture per replaced image.
pub(super) fn delete_kitty_image(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// The image ids the previous frame held and this one does not (gh #324 gap 4).
///
/// **An id diff, deliberately, rather than "delete what is in the changed range".** The
/// two operations look interchangeable and are not: a *cropped* image (gap 3) keeps its
/// id and changes only its placement, so a range-based delete would remove the texture
/// it is about to re-place. Diffing ids answers the narrower and correct question - which
/// textures is nothing going to point at any more.
///
/// Sorted and deduplicated so the emitted order is deterministic; a frame that replaces
/// two images deletes both in one synchronized block.
pub(super) fn orphaned_image_ids(previous: &[String], next: &[String]) -> Vec<u32> {
    let live: std::collections::BTreeSet<u32> =
        next.iter().flat_map(|line| kitty_ids_on(line)).collect();
    let mut orphaned: Vec<u32> = previous
        .iter()
        .flat_map(|line| kitty_ids_on(line))
        .filter(|id| !live.contains(id))
        .collect();
    orphaned.sort_unstable();
    orphaned.dedup();
    orphaned
}
