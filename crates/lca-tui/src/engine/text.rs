//! Text measurement and surgery, ported from pi's
//! `packages/tui/src/utils.ts`
//! (`pi-tui-re/src_re/tui-engine/utils.md`).
//!
//! This layer decides whether the whole renderer looks right: width, wrap,
//! truncate, slice, and ANSI-state tracking.
//!
//! Port note (RE doc §10): `unicode-width` covers the common East Asian /
//! combining / emoji cases; the terminal-bug corrections pi layers on top
//! (tab = 3, regional indicators pinned at 2, Thai/Lao AM +1, the terminal
//! spacing-mark set) are kept explicitly.
//! `ponytail:` full Unicode `Mark`/`Spacing_Mark` property tables are not
//! ported; `unicode-width` plus the explicit mark sets cover the common
//! cases. Upgrade with a `unicode-properties`-class crate if drift shows.
//!
//! Attribution: `graphemeWidth` is "based on code from the string-width
//! library" (MIT), as pi's own header states.
//!
//! This file is the **barrel over the module's own seams**, and each one is a
//! file a reader can open instead of scrolling:
//!
//! - `scan.rs` - the closed-vocabulary escape reader, `char_at`, and the ASCII
//!   fast path;
//! - `width.rs` - `grapheme_width`, the measured-width cache, and the two
//!   width entries;
//! - `wrap.rs` - word wrap with ANSI carried across breaks;
//! - `truncate.rs` - truncation to a cell budget;
//! - `slice.rs` - column slicing, the overlay split, and the column hit-tests;
//! - `ansi.rs`, `osc8.rs`, `stream_safety.rs` - the SGR tracker, OSC 8 links and
//!   the incomplete-escape sanitizer, each already its own file.
//!
//! Every public path is re-exported below under the name it had
//! (`lca_tui::engine::text::visible_width`, `…::truncate_to_width`, …), so the
//! split moved code and never a name.

mod ansi;
mod osc8;
// gh #405 gap 4: the incomplete-escape sanitizer, split out for the ceiling.
mod stream_safety;
pub use stream_safety::sanitize_incomplete_escapes;
// gh #405 gap 4: the streaming-safety rows (split chunks, incomplete tails).
#[cfg(test)]
#[path = "text/stream_safety_tests.rs"]
mod stream_safety_tests;

mod scan;
mod slice;
mod truncate;
mod width;
mod wrap;

pub use ansi::{AnsiCodeTracker, get_active_background_ansi};
pub use osc8::{ActiveHyperlink, Osc8Terminator, linkify_urls, parse_osc8_hyperlink};
pub(crate) use scan::char_at;
pub use scan::{ascii_visible_width, extract_ansi_code};
pub use slice::{
    extract_segments, get_grapheme_cell_range, get_osc8_link_at_column, slice_by_column,
    slice_with_width,
};
pub use truncate::truncate_to_width;
pub use width::{
    grapheme_width, normalize_terminal_output, strip_terminal_sequences, visible_width,
};

// Private helpers the rows in `tests.rs` ask for directly. They were private
// functions of this file before the split, so `use super::*` reached them; this
// keeps exactly that reach. `cfg(test)` because `fast_path_hits` only exists in
// test builds ("a faster function nobody calls is not a fix" has a counter to
// prove the call) and the slow width path is otherwise reached through
// `visible_width`, which is public.
#[cfg(test)]
use scan::fast_path_hits;
#[cfg(test)]
use width::visible_width_slow;
pub use wrap::{apply_background_to_line, wrap_text_with_ansi};

/// The punctuation set used by word navigation.
pub use super::word::{
    WordClass, atomic_span_at, atomic_start_ending_at, word_class, word_end_after,
    word_start_before,
};

/// The punctuation set used by word navigation (gh #383): one character of this set is
/// a punctuation run member, which makes `...` and `()->` move as single units.
pub const PUNCTUATION: &str = "(){}[]<>.,;:'\"!?+-=*/\\|&%^$#@~`";

/// Whether a character is whitespace.
pub fn is_whitespace_char(c: char) -> bool {
    c.is_whitespace()
}

/// Whether a character is punctuation.
pub fn is_punctuation_char(c: char) -> bool {
    PUNCTUATION.contains(c)
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
