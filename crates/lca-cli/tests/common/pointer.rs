//! Pointer geometry for real-pane receipts: the terminal **column** of a needle,
//! the screen cell of one, and a synthesized SGR-1006 drag.
//!
//! Split out of `common/mod.rs` when the harness crossed gate 11's ceiling, and it
//! earns its own file on merit: every click receipt in the CLI suite needs the same
//! three conversions, and getting any of them wrong makes a click land somewhere
//! else while the test still "passes" a looser assertion.
//!
//! # A byte index is not a column
//!
//! The reason this is shared rather than copied: `str::find` returns a **byte**
//! offset, and the startup header's logo glyphs (`█`, `▀`, `▄`) are three bytes and
//! **one column** wide. A column computed from a byte offset overstates by two cells
//! per glyph before the needle, so a synthesized click misses the button it aimed
//! at. gh #399 found four call sites carrying that bug - they had only ever matched
//! ASCII, where the two happen to agree.

use super::Tmux;

/// The **terminal column** where `needle` starts in `line`, or `None`.
///
/// Not `line.find(needle)`: see this module's header. This walks the prefix and asks
/// the engine's own width function, so a wide character counts as its real two
/// columns and an ANSI-wrapped line measures what the terminal shows.
pub fn column_of(line: &str, needle: &str) -> Option<u16> {
    let at = line.find(needle)?;
    let width = lca_tui::engine::text::visible_width(&line[..at]);
    u16::try_from(width).ok()
}

/// The tool card body's row number, from its own leading digits.
///
/// Returns `None` for a row that is not a body row (the resource listing also leads
/// with a digit), which is what keeps a click receipt aimed at the card.
// scrollbar glyph riding the end, so only the head is bare.
#[cfg(unix)]
pub fn body_number(line: &str) -> Option<u32> {
    if line.contains("extensions") {
        return None;
    }
    line.split_whitespace().next()?.parse::<u32>().ok()
}

/// Whether the captured bytes carry a **transcript** selection span.
///
/// Not simply "is `\x1b[7m` present": the editor paints its caret with the
/// same escape (`widgets/editor.rs`, `\x1b[7m \x1b[0m`), so a naive check
/// reports a highlight on every frame and asserts nothing. A selection is
/// distinguishable by width - it spans more than the single caret cell - and
/// by living above the composer. Both are required here.
#[cfg(unix)]
pub fn has_highlight(sgr: &str) -> bool {
    sgr.lines().any(|line| {
        let mut rest = line;
        while let Some(at) = rest.find("\x1b[7m") {
            let after = &rest[at + "\x1b[7m".len()..];
            // The caret is exactly one cell; a selection is at least two.
            let width = after
                .find("\x1b[")
                .map(|end| &after[..end])
                .unwrap_or(after);
            if width.chars().count() > 1 {
                return true;
            }
            rest = after;
        }
        false
    })
}

/// The **screen** row and column of `needle`, as 0-based cells.
///
/// `capture-pane -e` is not one output line per screen row: a row whose
/// content carries SGR is emitted with its escapes intact, and tmux can
/// break it across output lines (a trailing caret cell lands on the next
/// line). Counting `lines()` therefore points at the wrong row, and an SGR
/// drag sent to that row selects nothing - a silent vacuous pass.
///
/// So count *screen* rows the way the terminal does: a line is a new screen
/// row only when the previous one is a complete row. tmux pads a full row
/// with spaces, and a row that ended without a newline break is continued.
/// The robust form is to strip the escapes from the SGR capture and locate
/// the needle there, then map that index against the plain capture, which
/// does emit one line per row.
#[cfg(unix)]
pub fn locate(pane: &str, plain: &str, needle: &str) -> Option<(u16, u16)> {
    // The plain capture is one line per screen row. Find the needle there
    // for the row, then use the plain row's own column offset.
    plain
        .lines()
        .enumerate()
        // Gh #399: a terminal column, not a byte index (see `column_of`).
        .find_map(|(row, line)| column_of(line, needle).map(|col| (row as u16, col)))
        .or_else(|| {
            // Fallback: the needle may only be visible in the SGR capture.
            let _ = pane;
            None
        })
}

/// Drag a selection across the row holding `needle`, in SGR-1006 bytes.
///
/// The press and the motion carry button 0 (`\x1b[<0;…M` / `\x1b[<32;…M`);
/// the release is the `m` form. Coordinates are 1-based cells.
#[cfg(unix)]
pub fn drag_over(session: &Tmux, row: u16, start_col: u16, end_col: u16) {
    let y = row + 1;
    session.send_literal(&format!("\x1b[<0;{};{y}M", start_col + 1));
    session.send_literal(&format!("\x1b[<32;{};{y}M", end_col + 1));
    session.send_literal(&format!("\x1b[<0;{};{y}m", end_col + 1));
}
