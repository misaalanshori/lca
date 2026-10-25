//! The autocomplete popup's two-column layout (gh #386 gap 1a).
//!
//! Split out of `editor.rs` for the workspace's 1,200-line file ceiling
//! (gate 11). The rule is pi's `select-list.ts`
//! (`pi-tui-re/src_re/tui-widgets/pickers-and-fields.md` §`select-list.ts`):
//!
//! - **one** primary column for the whole list, not per row - which is the
//!   zigzag this fixes;
//! - its width is `clamp(widest visible label + 2, 12, 32)`;
//! - the description column appears only when the pane is wider than 40 cells
//!   **and** the space left after the primary column exceeds
//!   [`MIN_DESCRIPTION_WIDTH`], because on a narrow terminal a description
//!   that steals the label's space is worse than no description at all.
//!
//! Everything is measured with [`visible_width`], never `str::len()`: a CJK or
//! emoji label occupies cells, not bytes, and this repo has fixed that class of
//! bug often enough.

use crate::engine::text::{truncate_to_width, visible_width};
use crate::widgets::autocomplete::AutocompleteItem;

/// The narrowest the primary (label) column may be, in cells.
pub const MIN_PRIMARY_COLUMN_WIDTH: usize = 12;
/// The widest the primary (label) column may be, in cells.
pub const MAX_PRIMARY_COLUMN_WIDTH: usize = 32;
/// The gap between a label and its description, in cells (pi's
/// `PRIMARY_COLUMN_GAP`).
pub const PRIMARY_COLUMN_GAP: usize = 2;
/// The pane must be **wider** than this for a description column to exist.
pub const MIN_TWO_COLUMN_WIDTH: usize = 40;
/// The space left for a description must **exceed** this many cells.
pub const MIN_DESCRIPTION_WIDTH: usize = 10;
/// The selected-row marker, and the same width when it is absent.
pub const MARKER: &str = "▸ ";
/// The marker's width while a row is not selected.
pub const MARKER_BLANK: &str = "  ";

/// The width of one row's label column, in cells.
///
/// `clamp(widest visible label + PRIMARY_COLUMN_GAP, 12, 32)` - pi's
/// `getPrimaryColumnWidth`. Measured with [`visible_width`] so the answer is in
/// cells on every script.
#[must_use]
pub fn primary_column_width(items: &[AutocompleteItem]) -> usize {
    let widest = items
        .iter()
        .map(|item| visible_width(&item.label) + PRIMARY_COLUMN_GAP)
        .max()
        .unwrap_or(0);
    widest.clamp(MIN_PRIMARY_COLUMN_WIDTH, MAX_PRIMARY_COLUMN_WIDTH)
}

/// One rendered popup row.
///
/// `selected` puts the marker in front. The row is plain text - no escapes -
/// because the caller styles a *whole* selected row (pi's `selectedText`), which
/// is what keeps a selected row's description from being dim beside a styled
/// label.
#[must_use]
pub fn row(item: &AutocompleteItem, selected: bool, width: usize, column: usize) -> String {
    let marker = if selected { MARKER } else { MARKER_BLANK };
    let marker_width = visible_width(marker);

    // The description is the item's own, with its provenance tag appended in
    // the same column under the same truncation. A row that already has a
    // description keeps it; the tag rides after it (gh #386 gap 1b).
    let description = compose_description(item);

    // pi's threshold, in its order: the pane must be wider than 40, *then* the
    // remainder must exceed 10. Both are measured against the column this list
    // actually solves to, so a wide label set can remove the column on a pane
    // that a narrow one would keep.
    let effective_column = column.min(width.saturating_sub(marker_width + 4)).max(1);
    let description_start = marker_width + effective_column;
    let remaining = width.saturating_sub(description_start + 2);

    let Some(description) = description else {
        // No description at all: no label padding either, so a mixed list does
        // not leave trailing spaces.
        return truncate_to_width(&format!("{marker}{}", item.label), width, "…", false);
    };

    if width > MIN_TWO_COLUMN_WIDTH && remaining > MIN_DESCRIPTION_WIDTH {
        // The label is truncated *into its column*, never to make room for the
        // description at its own expense - the label is what the user is
        // choosing between.
        let label_width = effective_column.saturating_sub(PRIMARY_COLUMN_GAP).max(1);
        let label = truncate_to_width(&item.label, label_width, "…", false);
        let padding = effective_column
            .saturating_sub(visible_width(&label))
            .max(1);
        let description = truncate_to_width(&description, remaining, "…", false);
        return format!("{marker}{label}{}{description}", " ".repeat(padding));
    }

    // Narrow pane: one column, and the description is dropped rather than
    // squeezing the label. A pending popup is a keystroke away from being
    // re-rendered; an unreadable label is not recoverable.
    truncate_to_width(&format!("{marker}{}", item.label), width, "…", false)
}

/// The description text for a row: its own, flattened to one line, with the
/// provenance tag after it.
fn compose_description(item: &AutocompleteItem) -> Option<String> {
    // pi's `normalizeToSingleLine`: a multi-line description would otherwise
    // break the column, so its newlines become spaces.
    let own = item
        .description
        .as_deref()
        .map(|text| text.replace(['\r', '\n'], " ").trim().to_string())
        .filter(|text| !text.is_empty());
    let tag = item.origin.tag();
    match (own, tag) {
        (Some(own), Some(tag)) => Some(format!("{own} [{tag}]")),
        (Some(own), None) => Some(own),
        (None, Some(tag)) => Some(format!("[{tag}]")),
        (None, None) => None,
    }
}
