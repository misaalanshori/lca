//! Rows for the overlay/splice layer (gh #238, #373).
//!
//! Split out of `render.rs` when that file crossed the 1,200-line ceiling.

use super::*;
use lca_tui::engine::text::strip_terminal_sequences;

#[test]
fn overlay_box_centers_over_the_viewport() {
    let mut base = vec![".".repeat(80); 24];
    overlay_box(
        &mut base,
        80,
        24,
        "login",
        &["hello".to_string()],
        &Theme::plain(),
    );
    let joined: String = base
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("hello"));
    assert!(joined.contains("login"));
}

// Verifies: gh #238 - the seam isolates SGR state both ways:
// base styles never enter the segment, segment styles never leave
// it, and the right slice re-arms the base state.
#[test]
fn splice_segment_isolates_sgr_state_both_directions() {
    use lca_tui::engine::core::SEGMENT_RESET;
    // Base row opens italic and never closes it; the segment opens
    // bold and never closes it.
    let base = "aa\x1b[3mbbccdd".to_string();
    let row = splice_segment(&base, 2, 2, "\x1b[1mXX");
    let reset = SEGMENT_RESET;
    assert_eq!(
        row,
        format!("aa{reset}\x1b[1mXX{reset}\x1b[3mccdd"),
        "byte-exact splice: {row:?}"
    );
    // The segment's bold cannot reach the right slice...
    let after_bold = row.find("\x1b[1m").expect("segment style");
    let exit = row.find(reset).expect("entry reset");
    let second = row[exit + reset.len()..].find(reset).expect("exit reset") + exit + reset.len();
    assert!(
        after_bold > exit && after_bold < second,
        "bold inside the segment"
    );
    // ...and the base row's italic re-arms after the exit reset
    // (pending_ansi carries it into the right slice).
    assert!(
        row[second + reset.len()..].contains("\x1b[3m"),
        "base state restored"
    );
}

// Verifies: gh #238 - short segments pad to full width (no
// transparency gaps) and long ones clip without an ellipsis.
#[test]
fn splice_segment_pads_and_clips_to_exact_width() {
    use lca_tui::engine::text::visible_width;
    let row = splice_segment("0123456789", 3, 4, "AB");
    assert_eq!(visible_width(&row), 10);
    let stripped: String = row.chars().filter(|c| *c != '\x1b').collect();
    assert!(stripped.contains("AB  "), "padded: {stripped:?}");
    let row = splice_segment("0123456789", 2, 3, "ABCDEFG");
    assert_eq!(visible_width(&row), 10, "clipped, width kept");
}

// Verifies: gh #238 - the side panel splices at the seam: a
// transcript background band cannot tint panel cells, the panel
// always closes, and rows stay exactly `width` columns wide.
#[test]
fn side_panel_isolates_sgr_state_and_keeps_width() {
    use lca_tui::engine::core::SEGMENT_RESET;
    use lca_tui::engine::text::visible_width;
    // Base row opens a background band and never closes it.
    let mut base = vec![format!("xx\x1b[48;2;1;2;3m{}", "y".repeat(78)); 3];
    side_panel(&mut base, 80, &["panel".to_string()], &Theme::plain());
    for line in &base {
        assert_eq!(visible_width(line), 80, "exact width: {line:?}");
        assert_eq!(
            line.matches(SEGMENT_RESET).count(),
            2,
            "open and close: {line:?}"
        );
    }
    let reset = SEGMENT_RESET;
    let first = &base[0];
    // The band code appears only before the entry reset...
    let entry = first.find(reset).expect("entry");
    assert!(
        first[..entry].contains("48;2;1;2;3"),
        "base state left of the seam"
    );
    // ...never inside the panel cells.
    let exit = first[entry + reset.len()..].find(reset).expect("exit") + entry + reset.len();
    assert!(
        !first[entry + reset.len()..exit].contains("48;2;1;2;3"),
        "no band in the panel: {first:?}"
    );
    assert!(first.contains("panel"), "content survives");
}

// Verifies: gh #238 - the tooltip splices at the seam: the row
// beneath cannot tint it and its background cannot leak right.
#[test]
fn tooltip_isolates_sgr_state_both_directions() {
    use lca_tui::engine::core::SEGMENT_RESET;
    let theme = Theme::plain();
    let mut base = vec!["aa\x1b[3mbbccdd".to_string()];
    paint_tooltip(&mut base, 2, 0, &["\x1b[1mXX".to_string()], &theme);
    let reset = SEGMENT_RESET;
    let row = &base[0];
    // Entry reset, bold segment, exit reset, then the base row's
    // own italic re-armed over the remaining cells.
    assert!(
        row.contains(&format!("aa{reset}")),
        "entry reset after the left slice: {row:?}"
    );
    assert!(row.contains("\x1b[1mXX"), "segment content survives");
    let exit = row.find(&format!("XX{reset}")).expect("exit reset");
    assert!(
        row[exit..].contains("\x1b[3m"),
        "base state restored right of the seam: {row:?}"
    );
}

#[test]
fn side_panel_takes_the_right_column() {
    let mut base = vec!["x".repeat(80); 3];
    side_panel(&mut base, 80, &["panel".to_string()], &Theme::plain());
    let line = strip_terminal_sequences(&base[0]);
    // Rows close with a reset now (gh #238): the content still
    // lands at the right column, terminated, not dangling.
    assert!(line.contains("panel"), "{line:?}");
    assert!(line.trim_end().ends_with("panel"), "{line:?}");
}

// ==== gh #373 bug 1: a short base row must still splice at its column ====

/// One row with its ANSI sequences removed.
fn plain(row: &str) -> String {
    strip_terminal_sequences(row)
}

/// The column a substring starts at, in cells (never bytes).
fn column_of(row: &str, needle: &str) -> Option<usize> {
    let at = row.find(needle)?;
    Some(visible_width(&strip_terminal_sequences(&row[..at])))
}

// Verifies: gh #373 bug 1 - a segment spliced into a base row **shorter than
// `x`** lands at its column, not at the end of the text.
//
// This is the defect: `splice_segment` never padded `before` to `x`, so on any
// row shorter than the column the overlay slid left to wherever the text
// stopped. The caller had already placed it; the seam moved it.
#[test]
fn a_segment_lands_at_its_column_on_a_short_row() {
    let row = splice_segment("ab", 10, 2, "XX");
    assert_eq!(
        column_of(&plain(&row), "XX"),
        Some(10),
        "the segment must sit at column 10, not after `ab`: {:?}",
        plain(&row)
    );
}

// Verifies: gh #373 bug 1 - an empty base row still gets its overlay at `x`.
// The degenerate short row: with no text at all, the unpadded seam put the
// segment at column 0.
#[test]
fn a_segment_lands_at_its_column_on_an_empty_row() {
    let row = splice_segment("", 7, 3, "YYY");
    assert_eq!(
        column_of(&plain(&row), "YYY"),
        Some(7),
        "an empty row still anchors the overlay at its column"
    );
}

// Verifies: gh #373 bug 1 design call 2 - a 2-cell glyph straddling the left
// seam does not shift the segment.
//
// `█` at column 9 spans 9-10; the segment starts at 10, so the glyph cannot be
// drawn (it would bleed into the segment's first cell) and the row pads
// instead. The segment must still begin at exactly column 10.
#[test]
fn a_wide_glyph_at_the_left_seam_does_not_shift_the_segment() {
    // Nine ASCII cells, then `日` occupying columns 9 and 10.
    let base = format!("{}日tail", ".".repeat(9));
    let row = plain(&splice_segment(&base, 10, 2, "XX"));
    assert_eq!(
        column_of(&row, "XX"),
        Some(10),
        "a straddling wide glyph must not push the segment right: {row:?}"
    );
    assert_eq!(
        visible_width(&splice_segment(&base, 10, 2, "XX")),
        visible_width(&base),
        "and the row keeps its width"
    );
}

// Verifies: gh #373 bug 1 design call 2 - a 2-cell glyph straddling the right
// seam does not lose a tail column.
//
// The segment covers columns 10-11; a glyph spanning 11-12 straddles its right
// edge. Taking it would make the row one cell wider than the base row.
#[test]
fn a_wide_glyph_at_the_right_seam_never_widens_the_row() {
    let base = format!("{}日tail", ".".repeat(11));
    let spliced = splice_segment(&base, 10, 2, "XX");
    assert!(
        visible_width(&spliced) <= visible_width(&base),
        "splicing may drop a straddled glyph but must never add a cell: {:?}",
        plain(&spliced)
    );
}

// Verifies: gh #373 bug 1 design call 2 - the left seam keeps the straddling
// glyph and pads to the column, rather than shifting the segment.
//
// A 2-cell glyph at columns 9-10 and a segment starting at 10 cannot both be
// drawn without overlap, so the glyph survives (it is the base row's own
// content) and the segment starts exactly at 10. The pre-fix code put the
// segment at 11 because `before` measured 11 wide.
#[test]
fn the_left_seam_pads_to_the_column_when_the_glyph_is_dropped() {
    // `日` is 2 cells. Nine dots occupy 0-8, so `日` would span 9-10 and a
    // segment at 10 has no free cell for it: the slice keeps it (strict =
    // false) only when it ends by `x`, so it survives here and the pad adds
    // nothing, leaving the segment at 10.
    let base = format!("{}日tail", ".".repeat(9));
    let row = splice_segment(&base, 10, 2, "XX");
    assert_eq!(
        visible_width(&row),
        15,
        "9 dots + 2-cell glyph + 2-cell segment + `il`: {:?}",
        plain(&row)
    );
    assert_eq!(
        column_of(&plain(&row), "XX"),
        Some(10),
        "the segment still begins at 10: {:?}",
        plain(&row)
    );
    assert_eq!(
        visible_width(&row),
        visible_width(&base),
        "and the row is no wider than the base row"
    );
}

// Verifies: gh #373 bug 1 design call 2 - the right seam drops the straddling
// glyph instead of overwriting half of it or widening the row.
#[test]
fn the_right_seam_drops_the_glyph_and_keeps_the_row() {
    // Eleven dots (0-10), so `日` would span 11-12 - straddling the segment's
    // right edge at 12. Half a glyph cannot be drawn, so it goes, and the row
    // loses exactly its two cells: nothing is *added*.
    let base = format!("{}日tail", ".".repeat(11));
    let base_width = visible_width(&base);
    let row = splice_segment(&base, 10, 2, "XX");
    // The base is 11 dots + a 2-cell glyph + `tail` = 17. The segment covers
    // columns 10-11; the glyph's first cell (11) is inside it and its second
    // (12) is not, so the glyph goes and the segment's own 2 cells take its
    // place: 16. One cell less than the base, and never one more - the tail is
    // untouched, which is the property that matters.
    assert_eq!(
        visible_width(&row),
        base_width - 1,
        "the tail is preserved and the row is a cell narrower, not wider: {:?}",
        plain(&row)
    );
    assert!(
        !plain(&row).contains('日'),
        "a glyph straddling the segment's right edge cannot be half-drawn: {:?}",
        plain(&row)
    );
    assert!(plain(&row).ends_with("tail"), "and the tail survives");
}

// Verifies: gh #373 bug 1 design call 4 - **the invariant**: after splicing, a
// row is exactly `width` cells, for every (base length, x, width) combination.
//
// A table rather than a sample: the bug appears only where `base` is shorter
// than `x`, and a sample would have to guess which those are.
#[test]
fn splicing_never_changes_the_rows_width() {
    for width in [0usize, 1, 2, 3, 5, 8, 13, 21, 34, 60, 80] {
        for x in [0usize, 1, 2, 3, 5, 8, 13, 21, 34, 60, 80] {
            if x + width > 160 {
                continue;
            }
            for base_len in [0usize, 1, 2, 3, 5, 8, 13, 21, 34, 60, 80] {
                let base: String = "abcdefghij".chars().cycle().take(base_len).collect();
                let base_width = visible_width(&base);
                let spliced = splice_segment(&base, x, width, "SEG");
                // Where the segment sits beyond the base's own extent, the row
                // *must* grow to reach it - that is the pad. Where it fits
                // inside, the width is the base's.
                // The base is ASCII here, so no wide glyph can straddle; the
                // only legitimate width is the pad's (`x + width` when the
                // base is shorter). This is the invariant bug 1 violated.
                let expected = base_width.max(x + width);
                assert_eq!(
                    visible_width(&spliced),
                    expected,
                    "width={width} x={x} base_len={base_len}: a splice must not \
                     invent or drop columns: {:?}",
                    plain(&spliced)
                );
            }
        }
    }
}

// Verifies: gh #373 bug 1 design call 4, the exact-width form: a base row at
// least as wide as the segment's own span keeps its width exactly. That is the
// case a scrolling frame actually has, and the one a widening bug would destroy.
//
// The span, not the row, is the bound: a segment placed at `x` genuinely needs
// `x + seg` columns, so a row shorter than that *must* grow - that is the pad
// doing its job, and asserting otherwise would forbid the fix itself. Mutation
// found this: `row_width=1 x=0 seg=2` is a 1-cell row asked to hold a 2-cell
// segment, which cannot be honoured without widening.
#[test]
fn a_row_at_least_as_wide_as_the_span_keeps_its_width() {
    for row_width in [1usize, 2, 5, 20, 60, 80] {
        let base = "x".repeat(row_width);
        for x in 0..row_width {
            for seg in [1usize, 2, 5] {
                if x + seg > row_width {
                    continue;
                }
                let spliced = splice_segment(&base, x, seg, "S");
                assert_eq!(
                    visible_width(&spliced),
                    row_width,
                    "row_width={row_width} x={x} seg={seg}: a splice inside the row \
                     must not change its width"
                );
            }
        }
    }
}

// Verifies: gh #373 bug 1 - a row shorter than the segment's span grows to
// exactly the span, and no further. This is the other half of the invariant: the
// pad may widen a short row to reach the overlay, and must then stop.
#[test]
fn a_row_shorter_than_the_span_grows_to_exactly_the_span() {
    for row_width in [0usize, 1, 2, 3] {
        let base = "x".repeat(row_width);
        for x in 0..8usize {
            for seg in [1usize, 2, 5] {
                let spliced = splice_segment(&base, x, seg, "S");
                assert_eq!(
                    visible_width(&spliced),
                    row_width.max(x + seg),
                    "row_width={row_width} x={x} seg={seg}"
                );
            }
        }
    }
}

// Verifies: gh #373 bug 2 (free with bug 1's fix, with its own row) - the tail
// after the splice does not resurrect a style the base row had **closed**.
//
// `slice_by_column` collected every code before the seam into `pending_ansi`
// *and* pushed any code sitting exactly at the seam, so `\x1b[1mab\x1b[0mcd`
// answered `\x1b[1mcd` - bold coming back from the dead on the tail. The
// `AnsiCodeTracker` behind `extract_segments` emits the net style instead.
#[test]
fn the_tail_does_not_resurrect_a_closed_style() {
    let base = "\x1b[1mab\x1b[0mcd";
    let row = splice_segment(base, 0, 2, "XX");
    let tail = row.rsplit(SEGMENT_RESET).next().unwrap_or("");
    assert!(
        !tail.contains("\x1b[1m"),
        "the closed bold must not return after the splice: {row:?}"
    );
    assert!(tail.contains("cd"), "and the tail text survives: {row:?}");
}

// Verifies: gh #373 bug 2 - a style the base row leaves **open** still carries
// into the tail. The tracker is not "emit nothing"; it emits what is active.
#[test]
fn the_tail_keeps_a_style_that_is_still_open() {
    let base = "\x1b[3mabcd";
    let row = splice_segment(base, 0, 2, "XX");
    let tail = row.rsplit(SEGMENT_RESET).next().unwrap_or("");
    assert!(
        tail.contains("\x1b[3m") && tail.contains("cd"),
        "an open italic re-arms on the tail: {row:?}"
    );
}

// Verifies: gh #373 bug 1 - the drawer tab, through the real caller shape: a
// 1-cell glyph spliced at a column on a row much shorter than it.
#[test]
fn a_drawer_tab_lands_at_its_column_on_a_short_row() {
    // `paint_drawer_tab` splices exactly one cell at `col`.
    let row = plain(&splice_segment("prompt", 60, 1, "◀"));
    assert_eq!(
        column_of(&row, "◀"),
        Some(60),
        "the drawer tab belongs at column 60, not after `prompt`: {row:?}"
    );
}

// Verifies: gh #373 bug 1 - the jump indicator, likewise: a multi-cell label
// centered on a column, over a short row.
#[test]
fn a_jump_indicator_lands_at_its_column_on_a_short_row() {
    let label = " ↓ Jump to latest message ";
    let width = visible_width(label);
    let row = plain(&splice_segment("short", 12, width, label));
    assert_eq!(
        column_of(&row, "Jump"),
        Some(12 + visible_width(" ↓ ")),
        "the jump label starts at its column: {row:?}"
    );
}

// Verifies: R11 - the overlay box is a full frame, not a half one.
#[test]
fn the_overlay_box_draws_a_full_frame() {
    let mut base = vec![".".repeat(80); 24];
    overlay_box(
        &mut base,
        80,
        24,
        "login",
        &["hello".to_string()],
        &Theme::plain(),
    );
    let joined: Vec<String> = base.iter().map(|l| strip_terminal_sequences(l)).collect();
    assert!(
        joined.iter().any(|l| l.contains('╭') && l.contains('╮')),
        "a top frame with both corners"
    );
    assert!(
        joined.iter().any(|l| l.contains('╰') && l.contains('╯')),
        "a bottom frame with both corners"
    );
    let content = joined
        .iter()
        .find(|l| l.contains("hello"))
        .expect("a content line");
    let after = content.split("hello").nth(1).unwrap_or("");
    assert!(
        after.contains('│'),
        "a right border after the content: {content:?}"
    );
}

// Verifies: FR-UI-23 (R4) - every dialog row carries explicit styles:
// a reset, the frame in the border role, a reset - so a transcript
// color behind the dialog cannot bleed in, and the border cannot bleed
// into the transcript beside it.
#[test]
fn dialog_rows_override_the_styles_around_them() {
    let theme = Theme::colored();
    let border = theme.role(Role::Border);
    // A base line painted with a strong color, as the transcript's
    // markdown and tool cards are.
    let painted = format!("\x1b[41m{}\x1b[0m", ".".repeat(80));
    let mut base = vec![painted.clone(); 24];
    overlay_box(&mut base, 80, 24, "login", &["hello".to_string()], &theme);
    let row = base
        .iter()
        .find(|line| line.contains("hello"))
        .expect("the dialog row rendered");

    let reset_at = row.find(SEGMENT_RESET).expect("a reset before the frame");
    let after_reset = &row[reset_at + SEGMENT_RESET.len()..];
    assert!(
        strip_terminal_sequences(after_reset).starts_with("│ hello"),
        "the frame starts right after the reset: {row:?}"
    );
    let border_sgr = border("x");
    let prefix = border_sgr.split('x').next().unwrap_or("");
    assert!(
        !prefix.is_empty() && row.contains(prefix),
        "the frame carries the border role's SGR: {row:?}"
    );
    assert!(
        row.matches(SEGMENT_RESET).count() >= 2,
        "a reset on both sides of the dialog text: {row:?}"
    );
}

// Verifies: FR-UI-23 (R4) - the base fragments keep their own styling:
// the transcript to the left of a dialog keeps the color it had.
#[test]
fn the_fragments_around_a_dialog_keep_their_own_bytes() {
    let theme = Theme::colored();
    let left = format!("\x1b[42m{}\x1b[0m", "L".repeat(6));
    let line = format!("{left}{}", ".".repeat(74));
    let mut base = vec![line; 24];
    overlay_box(&mut base, 80, 24, "login", &["hello".to_string()], &theme);
    let row = base
        .iter()
        .find(|line| line.contains("hello"))
        .expect("the dialog row rendered");
    assert!(
        row.contains(&left),
        "the left fragment is untouched: {row:?}"
    );
}

// Verifies: R1 - a picker's selected row is pi's `selectedBg`
// highlight: the cells in accent on the selected background, the
// frame still in the border role, and the rows around it untouched.
#[test]
fn the_selected_picker_row_carries_the_selected_background() {
    let theme = Theme::colored();
    let mut base = vec![".".repeat(80); 24];
    overlay_box_selected(
        &mut base,
        80,
        24,
        "model",
        &["first".to_string(), "second".to_string()],
        &theme,
        Some(1),
    );
    let selected = base
        .iter()
        .find(|l| l.contains("second"))
        .expect("the selected row");
    let other = base
        .iter()
        .find(|l| l.contains("first"))
        .expect("the other row");
    assert!(
        selected.contains("\x1b[48;2;58;58;74m"),
        "selectedBg #3a3a4a: {selected:?}"
    );
    // Derived from the palette rather than pinned as a literal: the accent is a
    // calibrated value (gh #387 gap 3), and a row hardcoding it would need
    // editing every time the calibration moved.
    let painted = theme.role(Role::Accent)("x");
    let accent_sgr = painted.split('x').next().unwrap_or_default();
    assert!(
        !accent_sgr.is_empty() && selected.contains(accent_sgr),
        "the selected cells are the accent role ({accent_sgr:?}): {selected:?}"
    );
    assert!(
        selected.contains(&theme.role(Role::Border)("│ ")),
        "the frame keeps the border role: {selected:?}"
    );
    assert!(
        !other.contains("48;2;58;58;74"),
        "only the selected row is highlighted: {other:?}"
    );
}

// ---------------------------------------------------------------------------
// gh #373 gap 3: a modal's height comes from the rows it paints
// ---------------------------------------------------------------------------
//
// `overlay_box_placed` sized the box from `body.len()` - the *unwrapped* line count -
// while painting `wrap_text_with_ansi` output. So a modal with one long line was laid
// out three rows tall and then painted seven, and the paint ran past its own rect.
//
// The windowed variant (`overlay_box_placed_windowed`'s caller) already did this
// correctly: it wraps first, counts the wrapped rows, and only then resolves the rect.
// **This is a convergence, not a second implementation** - the single-pass path now
// computes its rows first and hands the same rows to the same painter.

/// The rows a paint actually touched, and the box's own vertical extent.
fn painted_box(base: &[String]) -> (Vec<usize>, Option<(usize, usize)>) {
    let rows: Vec<usize> = base
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            let plain = strip_terminal_sequences(r);
            !plain.trim_matches(['.', ' ']).trim().is_empty()
        })
        .map(|(i, _)| i)
        .collect();
    let top = base
        .iter()
        .position(|r| strip_terminal_sequences(r).contains('╭'));
    let bottom = base
        .iter()
        .rposition(|r| strip_terminal_sequences(r).contains('╰'));
    (rows, top.zip(bottom))
}

// Verifies: gh #373 gap 3 - **a modal with one long line is tall enough for its wrapped
// content**, and its bottom border sits below every painted row.
//
// This is the row the gap exists for. The assertion is on the *painted frame*, not a
// computed height: a fix that raised the number without moving the border would pass a
// height check and still cut through its own text.
#[test]
fn a_modal_with_one_long_line_is_as_tall_as_its_wrapped_content() {
    let mut base = vec![".".repeat(60); 24];
    let body = vec!["x".repeat(200)];
    overlay_box_selected(&mut base, 60, 24, "T", &body, &Theme::plain(), None);

    let (rows, frame) = painted_box(&base);
    let (top, bottom) = frame.expect("the box has both borders");
    let last_painted = *rows.last().expect("something was painted");
    assert_eq!(
        bottom, last_painted,
        "the bottom border is the last painted row - nothing sits below it: painted \
         {rows:?}, frame {top}..={bottom}"
    );
    // And the box really did grow for the wrap: one line, five wrapped rows.
    assert!(
        bottom > top + 2,
        "the box is taller than a single unwrapped line would need ({top}..={bottom})"
    );
    // Nothing was lost: every wrapped row is drawn.
    let inner = (60usize * 80 / 100).saturating_sub(4);
    let need = wrap_text_with_ansi(&body[0], inner).len();
    assert_eq!(
        bottom - top - 1,
        need,
        "one content row per wrapped row, no more and none dropped"
    );
}

// Verifies: gh #373 gap 3 - **the box's height comes from the wrapped rows, not from
// `body.len()`.**
//
// This is the assertion that fails against the defect, and it took two attempts to
// write: comparing the *painted* extent to the wrapped count passes either way, because
// the overflowing paint happens to span the same rows. What actually differs is the
// **rect the layout resolves** - the one `click_modal` uses - so that is what the row
// reads, via the same `resolve_overlay_layout` call with the wrapped count.
#[test]
fn the_box_height_is_derived_from_the_wrapped_rows() {
    let inner = 60usize * 80 / 100 - 4;
    let body = vec!["x".repeat(200)];
    let need = wrap_text_with_ansi(&body[0], inner).len();
    assert!(need > 1, "the fixture wraps ({need} rows)");

    let options = lca_tui::engine::core::OverlayOptions {
        width: Some(lca_tui::engine::core::SizeValue::Percent(80)),
        min_width: Some(24),
        max_height: Some(lca_tui::engine::core::SizeValue::Abs(24)),
        margin: 2,
        ..Default::default()
    };
    let from_wrapped = lca_tui::engine::core::resolve_overlay_layout(
        &options,
        60,
        24,
        u16::try_from(need).unwrap_or(u16::MAX).saturating_add(2),
    );
    let from_unwrapped = lca_tui::engine::core::resolve_overlay_layout(
        &options,
        60,
        24,
        u16::try_from(body.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2),
    );
    assert_ne!(
        from_wrapped.height,
        from_unwrapped.height,
        "the two ways of counting really do differ here (wrapped {need} vs body {}), so          the row distinguishes them",
        body.len()
    );

    // And the paint occupies the wrapped rect. Note this second half passes either way
    // for a *short* long-line body: the overflowing paint happens to span the same rows,
    // which is why the defect was invisible on screen until the body was tall enough to
    // be clipped. The `assert_ne!` above is the half that fails against the defect; the
    // clipped case is `a_body_taller_than_the_terminal_...`.
    let mut base = vec![".".repeat(60); 24];
    overlay_box_selected(&mut base, 60, 24, "T", &body, &Theme::plain(), None);
    let (_, frame) = painted_box(&base);
    let (top, bottom) = frame.expect("both borders");
    assert_eq!(
        bottom - top + 1,
        from_wrapped.height as usize,
        "the painted frame is exactly the wrapped rect's height ({} rows), not the          unwrapped one ({} rows)",
        from_wrapped.height,
        from_unwrapped.height
    );
}

// Verifies: gh #373 gap 3 (control) - **a body that already fits is unchanged.**
//
// The fix must not reshape short modals: a three-line body is three rows whatever the
// wrapping does, and a `body.len()+2` that happened to be right stays right.

#[test]
fn a_short_body_is_unchanged_by_the_two_pass() {
    let body = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
    let mut base = vec![".".repeat(60); 24];
    overlay_box_selected(&mut base, 60, 24, "T", &body, &Theme::plain(), None);
    let (_, frame) = painted_box(&base);
    let (top, bottom) = frame.expect("both borders");
    assert_eq!(bottom - top + 1, 3 + 2, "three body lines plus two borders");
    let text: String = base
        .iter()
        .map(|r| strip_terminal_sequences(r))
        .collect::<Vec<_>>()
        .join("\n");
    for word in ["alpha", "beta", "gamma"] {
        assert!(text.contains(word), "{word} is painted: {text}");
    }
}

// Verifies: gh #373 gap 3 - **a body taller than the terminal keeps a closed frame and
// says so.**
//
// Once wrapping counts, a long body can exceed the screen. The clamp caps the box's
// *height*; the content is then truncated to fit and the bottom border carries a
// `… n more` note, so the box is never left open and never silently drops lines. That
// decision has to be pinned: an open box is the same defect one step out.
#[test]
fn a_body_taller_than_the_terminal_keeps_a_closed_frame_and_says_what_is_missing() {
    // 24 rows is the case that actually distinguishes the fix: with the unwrapped
    // height the paint runs past the frame's end and the **bottom border is never
    // drawn** (`bottom=false`), while a 12-row terminal happened to clip in a place
    // that kept it. A row that only used 12 would have passed against the defect - which
    // is exactly what my first version did.
    let term_h = 24u16;
    let body = vec!["y".repeat(2000)];
    let mut base = vec![".".repeat(60); term_h as usize];
    overlay_box_selected(&mut base, 60, term_h, "T", &body, &Theme::plain(), None);

    let (_, frame) = painted_box(&base);
    let (top, bottom) = frame.expect("the box still has a top and a bottom border");
    assert!(
        bottom < term_h as usize,
        "the box fits the terminal ({top}..={bottom} of {term_h})"
    );
    let bottom_row = strip_terminal_sequences(&base[bottom]);
    assert!(
        bottom_row.contains("more"),
        "and the bottom border says how many rows are not shown, rather than leaving the \
         frame open: {bottom_row:?}"
    );
}

// Verifies: gh #373 gap 3 - **an unbreakable token wider than the inner width hard-wraps,
// and the box is sized for the rows that produces.**
//
// The adversarial case for any wrapper: a long path or URL has no break opportunity, so
// the wrapper force-breaks it. Whatever it does, the height must come from the same call,
// or the box and its content disagree again.
#[test]
fn an_unbreakable_token_wider_than_the_box_wraps_and_the_box_accounts_for_it() {
    let mut base = vec![".".repeat(60); 24];
    let token = "https://example.com/".to_string() + &"a".repeat(300);
    let body = vec![token.clone()];
    overlay_box_selected(&mut base, 60, 24, "T", &body, &Theme::plain(), None);

    let (_, frame) = painted_box(&base);
    let (top, bottom) = frame.expect("both borders");
    let inner = 60usize * 80 / 100 - 4;
    // The link is wrapped *after* linkification, which is what the painter does.
    let need = wrap_text_with_ansi(&linkify_urls(&token), inner).len();
    assert!(
        need > 1,
        "the token really is unbreakable-and-wider than {inner}"
    );
    assert_eq!(
        bottom - top - 1,
        need,
        "one row per wrapped segment of the unbreakable token"
    );
    // Every painted row is exactly the box width - a hard wrap must not overflow.
    for row in &base[top..=bottom] {
        assert_eq!(
            visible_width(&strip_terminal_sequences(row)),
            60,
            "each row is the full box width: {:?}",
            strip_terminal_sequences(row)
        );
    }
}

// Verifies: gh #373 gap 3 - **the picker's own two-pass path is the one that survives.**
//
// `overlay_box_picker` already wrapped for its window before this cycle (it counts
// `wrap_row` output to decide what fits), so it is the **reference** the single-pass
// modal path converged on rather than a second thing to fix. This row pins that its
// bottom border is its last painted row - the property that was broken on the modal
// side - so a future edit cannot make the picker regress into the same shape.
//
// It is anchored above the composer, so it cannot grow downward the way a centred modal
// can; that is why the assertion is "the frame is closed at its last painted row"
// rather than "the box grew".
#[test]
fn the_picker_keeps_a_closed_frame_at_its_last_painted_row() {
    // A realistic picker body: several items, one of which wraps. (One item alone hits
    // the `.max(1)` item floor, which is a different code path.)
    let body = vec![
        "short".to_string(),
        "a very long picker row that must wrap more than once across the box".to_string(),
        "tail".to_string(),
        String::new(),
        String::new(),
    ];
    let mut base = vec![".".repeat(60); 24];
    let _ = overlay_box_picker(
        &mut base,
        60,
        24,
        "pick",
        &body,
        &Theme::plain(),
        Some(0),
        0,
        None,
        &[],
    );
    let (rows, frame) = painted_box(&base);
    let (_, bottom) = frame.expect("both borders");
    assert_eq!(
        *rows.last().expect("painted"),
        bottom,
        "the picker's bottom border is its last painted row: {rows:?}"
    );
    // And the wrapping row really did become more than one painted row.
    let inner = 60usize * 80 / 100 - 4;
    assert!(
        wrap_text_with_ansi(&body[1], inner).len() > 1,
        "the fixture's long row wraps ({:?} at inner {inner}), so the row exercises the          two-pass path: {:?}",
        wrap_text_with_ansi(&body[1], inner).len(),
        body[1]
    );
    let content_rows = rows.len().saturating_sub(2);
    assert!(
        content_rows > 1,
        "more than one content row was painted for the wrapped line: {rows:?}"
    );
}

// Verifies: gh #373 gap 3 - **a click on a wrapped modal row lands on that row's line.**
//
// The agreement row the brief calls for, and the one that fails today: `click_modal` sized
// its hit box from `body.len()` while the paint used wrapped rows, so a click on a
// wrapped row was rejected as "outside the box" (or mapped to the wrong body line). Both
// now count the same wrapped rows through `wrap_count`, which is
// `wrap_text_with_ansi(...).len()` - the painter's own wrapper.
//
// The hit-test needs a live host, so this row drives its *arithmetic* directly: the
// wrapped row index of a click, computed the way `click_modal` computes it, must name the
// body line that the frame paints on that row.
#[test]
fn a_click_on_a_wrapped_row_maps_to_the_line_painted_there() {
    let body = vec![
        "short".to_string(),
        "a body line long enough that it must wrap more than once across the box".to_string(),
        "tail".to_string(),
    ];
    let inner = 60usize * 80 / 100 - 4;

    // What `click_modal` builds: each body line's first content row.
    let mut wrapped: Vec<usize> = Vec::with_capacity(body.len() + 1);
    let mut rows = 0usize;
    for line in &body {
        wrapped.push(rows);
        rows += wrap_text_with_ansi(line, inner).len().max(1);
    }
    wrapped.push(rows);
    assert!(rows > body.len(), "the fixture really wraps ({rows} rows)");

    // The rect the hit-test now resolves: the wrapped count, not `body.len()`.
    let options = lca_tui::engine::core::OverlayOptions {
        width: Some(lca_tui::engine::core::SizeValue::Percent(80)),
        min_width: Some(24),
        max_height: Some(lca_tui::engine::core::SizeValue::Abs(24)),
        margin: 2,
        ..Default::default()
    };
    let rect_wrapped = lca_tui::engine::core::resolve_overlay_layout(
        &options,
        60,
        24,
        u16::try_from(rows).unwrap_or(u16::MAX).saturating_add(2),
    );
    let rect_unwrapped = lca_tui::engine::core::resolve_overlay_layout(
        &options,
        60,
        24,
        u16::try_from(body.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2),
    );
    assert_ne!(
        rect_wrapped.height,
        rect_unwrapped.height,
        "the two counts give different hit boxes here, so the row distinguishes them \
         (wrapped {rows} vs body {})",
        body.len()
    );

    // Every content row of the wrapped rect maps into a body line that exists, and the
    // rows of line 1 map to line 1 - not to line 2, which is what an unwrapped count
    // reports for them.
    // Content row `r` (1-based within the frame) is body row `r - 1`.
    for content_row in 1..(rect_wrapped.height as usize - 1) {
        let content = content_row - 1;
        let line_index = wrapped
            .iter()
            .rposition(|start| *start <= content)
            .unwrap_or(0);
        assert!(
            line_index < body.len(),
            "row {content} maps to a real body line ({line_index})"
        );
        if content >= wrapped[1] && content < wrapped[2] {
            assert_eq!(
                line_index, 1,
                "body row {content} is one of the wrapped line's rows, so it names line 1"
            );
        }
    }
    // And the last content row is inside the rect, so a click there is not rejected.
    let last_content = rect_wrapped.height as usize - 2;
    assert!(
        last_content >= wrapped[1],
        "a click on the last content row of the wrapped rect reaches line 1's rows \
         (last content {last_content}, line 1 starts {})",
        wrapped[1]
    );
}

// ===========================================================================
// gh #373 gap 4: a percentage positions the overlay, not its corner.
//
// These rows drive the real painter at the rect the layout resolves for a
// percentage position, so what they read is the frame a user sees. The gaps are
// the property: a corner-anchored formula leaves the right gap half the box
// wider, and a centring fix silently breaks both endpoints.
// ===========================================================================

/// The first and last columns of a painted frame row, in cells.
fn frame_edges(row: &str) -> Option<(usize, usize)> {
    let plain = strip_terminal_sequences(row);
    let left = plain
        .chars()
        .position(|c| c == '╭' || c == '│' || c == '╰')?;
    let right = plain
        .chars()
        .collect::<Vec<char>>()
        .iter()
        .rposition(|c| *c == '╮' || *c == '│' || *c == '╯')?;
    Some((left, right))
}

/// The opts a percentage-positioned box resolves, and the box's rect.
fn percent_box(
    term_w: u16,
    term_h: u16,
    pct: u16,
    body: &[String],
) -> (
    lca_tui::engine::core::OverlayOptions<'static>,
    lca_tui::engine::core::Rect,
) {
    let options = lca_tui::engine::core::OverlayOptions {
        width: Some(lca_tui::engine::core::SizeValue::Percent(60)),
        min_width: Some(24),
        max_height: Some(lca_tui::engine::core::SizeValue::Abs(10)),
        col: Some(lca_tui::engine::core::SizeValue::Percent(pct)),
        row: Some(lca_tui::engine::core::SizeValue::Percent(pct)),
        margin: 2,
        ..Default::default()
    };
    let rect = lca_tui::engine::core::resolve_overlay_layout(
        &options,
        term_w,
        term_h,
        u16::try_from(body.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2),
    );
    (options, rect)
}

// Verifies: gh #373 gap 4 - `Percent(50)` leaves equal gaps on both sides, and
// the painted frame is the width the layout resolved.
#[test]
fn a_percentage_placed_overlay_has_equal_gaps_on_both_sides() {
    let body = vec!["alpha".to_string(), "beta".to_string()];
    for (term_w, term_h) in [(80u16, 24u16), (100, 30), (61, 21)] {
        let (options, rect) = percent_box(term_w, term_h, 50, &body);
        let mut base = vec![".".repeat(term_w as usize); term_h as usize];
        overlay_box_placed(
            &mut base,
            term_w,
            term_h,
            "t",
            &body,
            &Theme::plain(),
            None,
            &options,
        );
        let plain: Vec<String> = base
            .iter()
            .map(|line| strip_terminal_sequences(line))
            .collect();
        let top = plain
            .iter()
            .position(|r| r.contains('╭'))
            .expect("a top border");
        let bottom = plain
            .iter()
            .rposition(|r| r.contains('╰'))
            .expect("a bottom border");
        let (left, right) = frame_edges(&plain[top]).unwrap_or_else(|| panic!("{:?}", plain[top]));
        assert_eq!(
            right - left + 1,
            rect.width as usize,
            "the painted frame is the resolved rect's width at {term_w}x{term_h}"
        );
        let left_gap = left;
        let right_gap = (term_w as usize).saturating_sub(right + 1);
        assert!(
            left_gap.abs_diff(right_gap) <= 1,
            "at {term_w}x{term_h} the horizontal gaps differ: left {left_gap}, \
             right {right_gap} (frame {left}..={right}, rect {rect:?})"
        );
        let above = top;
        let below = term_h as usize - (bottom + 1);
        assert!(
            above.abs_diff(below) <= 1,
            "at {term_w}x{term_h} the vertical gaps differ: above {above}, below {below}"
        );
        assert_eq!(top, rect.row as usize, "the frame starts at the rect's row");
        assert_eq!(
            bottom - top + 1,
            rect.height as usize,
            "and is the rect's height"
        );
    }
}

// Verifies: gh #373 gap 4 - the endpoints, which are the values in use.
// `Percent(0)` puts the frame's near edge on the margin; `Percent(100)` puts its
// far edge on the opposite margin. A pure centring fix breaks the second.
#[test]
fn a_percentage_overlays_endpoints_sit_on_the_margins() {
    let body = vec!["alpha".to_string()];
    for pct in [0u16, 100] {
        let (options, rect) = percent_box(80, 24, pct, &body);
        let mut base = vec![".".repeat(80); 24];
        overlay_box_placed(
            &mut base,
            80,
            24,
            "t",
            &body,
            &Theme::plain(),
            None,
            &options,
        );
        let plain: Vec<String> = base
            .iter()
            .map(|line| strip_terminal_sequences(line))
            .collect();
        let top = plain
            .iter()
            .position(|r| r.contains('╭'))
            .expect("a top border");
        let bottom = plain
            .iter()
            .rposition(|r| r.contains('╰'))
            .expect("a bottom border");
        let (left, right) = frame_edges(&plain[top]).unwrap_or_else(|| panic!("{:?}", plain[top]));
        if pct == 0 {
            assert_eq!(left, 2, "Percent(0) sits on the left margin");
            assert_eq!(top, 2, "and on the top margin");
        } else {
            assert_eq!(right + 1, 78, "Percent(100) ends on the right margin");
            assert_eq!(bottom + 1, 22, "and on the bottom margin");
        }
        assert_eq!(top, rect.row as usize);
        assert_eq!(bottom - top + 1, rect.height as usize);
    }
}
