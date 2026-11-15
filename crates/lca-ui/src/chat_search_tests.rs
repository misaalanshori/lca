//! Gh #376's rows: the wrap-insensitive corpus, the anchored selection, the reveal,
//! the follow lock, the two styles, and the previous direction.
//!
//! Every row asserts an **observable** - a reported span, a scroll value, or the
//! painted SGR bytes - never "a match exists", because "a match exists" is exactly
//! what the pre-fix code already answered while the phrase went unfound.

use super::Chat;
use super::chat_search_corpus::{self, SearchMatch};
use super::tests::{chat, strip};

/// A chat whose transcript fills more than a 24-row pane, one prompt per entry.
fn chat_with(prompts: &[&str]) -> Chat {
    let mut chat = chat();
    chat.world.resize(80, 24);
    for (i, prompt) in prompts.iter().enumerate() {
        chat.transcript.push_user(*prompt);
        // The answer deliberately does **not** repeat the prompt: a row that asks
        // "how many matches" is worthless if the fixture planted the word twice.
        chat.transcript.append_text(&format!("answer number {i}"));
        chat.transcript.finish_assistant();
    }
    chat
}

/// The rows of the rendered document, plain text, for a documented comment receipt.
fn document(chat: &Chat, width: u16) -> Vec<String> {
    strip(&chat.render(width))
}

/// Open the search and type `query` one key at a time (the production order).
fn search(chat: &mut Chat, query: &str) {
    chat.handle_key("\x12");
    for c in query.chars() {
        chat.handle_key(&c.to_string());
    }
}

/// The segments of the selected match, as `(row, start, end)`.
fn selected_segments(chat: &Chat) -> Vec<(usize, usize, usize)> {
    let key = chat.search_selected.expect("a selection");
    chat.search_matches
        .iter()
        .find(|m| m.key() == key)
        .map(|m| {
            m.segments
                .iter()
                .map(|s| (s.row, s.start_col, s.end_col))
                .collect()
        })
        .unwrap_or_default()
}

// ===========================================================================
// Gap 1: the corpus - a phrase split by a wrap is still one match.
// ===========================================================================

// Verifies: gh #376 gap 1 - **a phrase the window wrapped is one match on two rows.**
//
// The measurement that made this the requirement rather than an edge case: at 20
// columns `"error handling"` cannot fit on one row, so the per-row `contains` this
// replaces answered zero for a phrase that is plainly in the text. The row asserts the
// **spans**, because "a match exists" is what the defect already claimed for
// single-row queries.
#[test]
fn a_phrase_split_by_a_wrap_is_one_match_across_two_rows() {
    // **Narrow on purpose**: at 80 columns (and even at 24, the bubble's indent leaves
    // room) the phrase fits one row, which is the case the old code already answered.
    // 16 columns forces the wrap the issue is about; measured, the two halves land on
    // adjacent rows of the same bubble.
    let mut chat = chat_with(&["the error handling path is here"]);
    chat.world.resize(16, 24);
    search(&mut chat, "error handling");
    assert_eq!(chat.search_matches.len(), 1, "one phrase, one match");

    // The same document, natively wrapped by the search's own width.
    let width = chat.world.size.0;
    let rows = document(&chat, width);
    let segments = selected_segments(&chat);
    assert_eq!(segments.len(), 2, "one segment per row: {segments:?}");
    // The notice row is **one wrapped bubble**: the phrase is split by the wrap and the
    // halves land on consecutive rows of that bubble (rows 19 and 20 in a 16-column
    // pane). Asserting `adjacent rows` rather than a hard-coded pair keeps the row about
    // the wrap and not about the fixture's chrome.
    assert!(
        segments[0].0 + 1 == segments[1].0,
        "the halves are on adjacent rows: {segments:?}"
    );
    assert!(
        segments[0].2 > segments[0].1 && segments[1].2 > segments[1].1,
        "neither half is empty: {segments:?}"
    );
    // Both halves are the parts of the phrase: `error` ends row 0, `handling` starts
    // the next. Quoting the rows is the receipt the brief asks for.
    let first = &rows[segments[0].0];
    let text = lca_tui::engine::text::slice_by_column(
        first,
        segments[0].1,
        segments[0].2 - segments[0].1,
        false,
    );
    assert_eq!(text, "error", "the first row's half is `error`: {first:?}");
    let text = lca_tui::engine::text::slice_by_column(
        &rows[segments[1].0],
        segments[1].1,
        segments[1].2 - segments[1].1,
        false,
    );
    assert_eq!(
        text, "handling",
        "the second row's half is `handling`: {:?}",
        rows[segments[1].0]
    );
}

// Verifies: gh #376 gap 1 - **a match through a wide glyph reports the cell it
// occupies**, which is the `visible_width` rule rather than byte length.
//
// A CJK row is two cells per character, so a byte-indexed column would report half the
// width; the row asserts the span covers exactly the two cells of `界`.
#[test]
fn a_match_through_a_wide_glyph_reports_cell_columns() {
    let mut chat = chat_with(&["边界 error 处理"]);
    search(&mut chat, "error");
    let segments = selected_segments(&chat);
    assert_eq!(segments.len(), 1, "{segments:?}");
    let (row, start, end) = segments[0];
    assert_eq!(
        end - start,
        5,
        "`error` is five cells, no matter what is behind it: {segments:?}"
    );
    // And the columns really are cells: slicing the row by them yields `error`, which
    // would be off by one per preceding wide glyph under a byte-indexed column.
    let rows = document(&chat, chat.world.size.0);
    let text = lca_tui::engine::text::slice_by_column(&rows[row], start, end - start, false);
    assert_eq!(text, "error", "row {:?}", rows[row]);
}

// Verifies: gh #376 gap 1 - **case folding keeps the span mapping exact.**
//
// `İ` folds to two characters, so a whole-document fold shifts every offset after it -
// which is why the old highlighter bailed out on such a line. Folding per character
// with an index map keeps the columns right, and the row proves it by finding a match
// *after* the expanding character.
#[test]
fn a_fold_that_expands_a_character_does_not_shift_the_span() {
    let mut chat = chat_with(&["İstanbul notes: NEEDLE here"]);
    search(&mut chat, "needle");
    let segments = selected_segments(&chat);
    assert_eq!(segments.len(), 1, "{segments:?}");
    let (row, start, end) = segments[0];
    let rows = document(&chat, chat.world.size.0);
    let text = lca_tui::engine::text::slice_by_column(&rows[row], start, end - start, false);
    assert_eq!(
        text, "NEEDLE",
        "the span still lands on the match after the fold: row {:?}",
        rows[row]
    );
}

// Verifies: gh #376 gap 1 - **a query's own whitespace runs collapse**, the same rule
// the corpus uses to join runs. Typing two spaces must find what one finds, or the
// corpus join would be a rule the query path does not share.
#[test]
fn a_query_with_extra_spaces_finds_what_one_space_finds() {
    let mut chat = chat_with(&["error handling"]);
    search(&mut chat, "error  handling");
    assert_eq!(
        chat.search_matches.len(),
        1,
        "the collapse makes it the same query: {:?}",
        chat.search_matches
    );
}

// ===========================================================================
// Gap 2: anchoring - the first pick is the nearest at-or-after, not row 0.
// ===========================================================================

// Verifies: gh #376 gap 2 - **typing does not yank the reader to the top of the
// document.** With the viewport scrolled into the transcript, the first match picked
// is the nearest at or after the top row, so `f` then `fu` keeps the reader where they
// were reading instead of teleporting once per keystroke.
#[test]
fn typing_anchors_the_selection_at_the_current_viewport() {
    let mut chat = chat_with(&[
        "function one",
        "function two",
        "unrelated",
        "function three",
        "unrelated",
    ]);
    let width = chat.world.size.0;
    let height = chat.world.size.1;
    // Scroll away from the live bottom, into the middle of the document.
    let window = chat.window_height(width, height);
    let content = chat.transcript_len(width);
    let middle = u16::try_from(content.saturating_sub(window) / 2).unwrap_or(0);
    chat.last_scroll = middle;

    search(&mut chat, "funct");
    let top = chat.search_top_row();
    let first = chat
        .search_matches
        .get(chat.search_index)
        .map(SearchMatch::first_row)
        .expect("a selection");
    assert!(
        first >= top,
        "the selection is at or after the viewport top {top}, not row 0: {first}"
    );
    let earlier = chat
        .search_matches
        .iter()
        .filter(|m| m.first_row() < top)
        .count();
    assert!(
        earlier > 0 || chat.search_index > 0,
        "the fixture has matches above and below the anchor: {:?}",
        chat.search_matches
            .iter()
            .map(SearchMatch::first_row)
            .collect::<Vec<_>>()
    );
    // Typing another character of the same word keeps the same anchor, so the pick does
    // not move once per keystroke. (`i` narrows `funct` to `functi`, which still matches
    // the same entries, so the comparison is about the anchor and not the match count.)
    chat.handle_key("i");
    assert!(
        !chat.search_matches.is_empty(),
        "the narrowed query still matches"
    );
    let again = chat
        .search_matches
        .get(chat.search_index)
        .map(SearchMatch::first_row)
        .expect("a selection");
    assert_eq!(
        again, first,
        "the second keystroke keeps the reader's place: {again} vs {first}"
    );
}

// Verifies: gh #376 gap 2 - **wrapping is for the case with nothing below.** A query
// whose only match is above the viewport selects it rather than finding nothing.
#[test]
fn a_query_whose_only_match_is_above_wraps_to_it() {
    let mut chat = chat_with(&["early needle", "unrelated", "unrelated", "unrelated"]);
    let width = chat.world.size.0;
    let height = chat.world.size.1;
    let window = chat.window_height(width, height);
    let content = chat.transcript_len(width);
    let bottom = u16::try_from(content.saturating_sub(window)).unwrap_or(0);
    chat.last_scroll = 0; // live bottom
    assert!(bottom > 0, "the document overflows the window");

    search(&mut chat, "needle");
    let rows = chat
        .search_matches
        .iter()
        .map(SearchMatch::first_row)
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "one match: {rows:?}");
    assert!(
        rows[0] < chat.search_top_row(),
        "it is above the viewport (top {}), so this is the wrap: {rows:?}",
        chat.search_top_row()
    );
    assert_eq!(
        chat.search_index, 0,
        "and it is selected rather than skipped"
    );
}

// ===========================================================================
// Gap 3: the reveal leaves context above, and holds against streaming.
// ===========================================================================

// Verifies: gh #376 gap 3 - **a third of the window sits above the match.** The
// pre-fix reveal top-pinned (`scroll_pinning`), so the match was the first row and the
// reader had no context above it; a search result is easier to judge with the lines
// that lead into it.
#[test]
fn a_reveal_leaves_a_third_of_the_window_above_the_match() {
    // Room **below** the match as well as above: a reveal that only left context
    // above would be satisfiable by a fixture that ends at the match.
    let mut chat = chat_with(&[
        "filler",
        "filler",
        "filler",
        "filler",
        "needle here",
        "filler",
        "filler",
        "filler",
        "filler",
        "filler",
        "filler",
    ]);
    let width = chat.world.size.0;
    let height = chat.world.size.1;
    search(&mut chat, "needle");
    let row = chat
        .search_matches
        .get(chat.search_index)
        .map(SearchMatch::first_row)
        .expect("a match");

    let window = chat.window_height(width, height);
    let scroll = chat.search_scroll.expect("the reveal asked for a scroll");
    let content = chat.transcript_len(width);
    let top = content
        .saturating_sub(window)
        .saturating_sub(usize::from(scroll));
    let lead = row.saturating_sub(top);
    let third = window / 3;
    assert!(
        lead >= third,
        "the match sits at least a third of {window} rows down (lead {lead}, third {third})"
    );
    assert!(
        content.saturating_sub(window) >= row + (window - third),
        "and there is room below it in the fixture"
    );
}

// Verifies: gh #376 gap 3 - **a stream delta cannot take the selected match away.**
//
// The defect: a match at the live bottom re-engaged sticky follow, so the next token
// pushed the match up and out of the window. The lock is pi's `followSuppressedAtEnd`
// in this tree's bottom coordinates; the row drives the production order
// (`search_scroll`, then `clamp_scroll`) with a token arriving in between.
#[test]
fn a_stream_delta_does_not_move_the_selected_match() {
    // The match sits **mid-document** so the reveal's scroll is not 0: at the live
    // bottom the follow rule already returns 0 and the lock would be untestable.
    let mut chat = chat_with(&[
        "filler", "filler", "filler", "filler", "needle", "filler", "filler", "filler", "filler",
        "filler",
    ]);
    search(&mut chat, "needle");
    let scroll = chat.search_scroll.expect("a reveal");
    let width = chat.world.size.0;
    let height = chat.world.size.1;
    // `clamp_scroll` runs on the reveal's scroll; the lock is armed.
    let clamped = chat.clamp_scroll(scroll, width, height);
    assert_eq!(
        clamped, scroll,
        "the reveal's scroll survives its own clamp"
    );

    // Now a token arrives: the document grows.
    chat.transcript
        .append_text("\nmore answer text\nand a third line");
    let after = chat.clamp_scroll(clamped, width, height);
    assert_eq!(
        after, clamped,
        "the lock holds the position while output streams ({} vs {})",
        after, clamped
    );

    // A reader's scroll is theirs: any other value drops the lock, and the delta applies
    // again, so the lock cannot become a sticky trap.
    chat.search_follow_lock.set(None);
    chat.transcript.append_text("\nand more");
    let next = chat.clamp_scroll(clamped, width, height);
    assert_ne!(next, clamped, "without the lock the delta applies: {next}");
}

// ===========================================================================
// Gap 4: two styles, per-channel closes, and the previous direction.
// ===========================================================================

// Verifies: gh #376 gap 4 - **both renderers paint the two styles.** The scrollback
// screen returns from `compose_overlays_bottom_anchored` early when nothing is overlaid
// (the search was never an overlay), so the highlight must be owned by the frame - a
// row that only drove the fullscreen path would pass while the default renderer
// painted nothing (measured: `has_bold=false has_under=false` before the fix).
#[test]
fn both_renderers_paint_the_search_styles() {
    for screen_mode in [false, true] {
        let mut chat = chat_with(&["needle one", "unrelated", "needle two"]);
        chat.screen_mode = screen_mode;
        search(&mut chat, "needle");
        let frame = chat.viewport(80, 24, 0).join("\n");
        assert!(
            frame.contains("\x1b[1;7m") && frame.contains("\x1b[4m"),
            "both styles paint with screen_mode={screen_mode}: {frame:?}"
        );
    }
}

// Verifies: gh #376 gap 4 - **the current match's SGR differs from the others', and
// each span closes only the channels it opened.**
//
// pi's defaults are `\x1b[4m…\x1b[24m` (underline) for the others and
// `\x1b[1;7m…\x1b[22;27m` (bold+inverse) for the current one. `\x1b[0m` would also drop
// the row's background and any hyperlink, which is why this tree forbids it in styling
// paths; the row asserts its absence in the frame as well as the two pairs.
#[test]
fn the_current_match_is_bold_and_the_others_are_underlined() {
    let mut chat = chat_with(&["needle one", "unrelated", "needle two"]);
    search(&mut chat, "needle");
    assert_eq!(chat.search_matches.len(), 2, "two matches");

    let raw = chat.viewport(80, 24, 0);
    let frame = raw.join("\n");
    assert!(
        frame.contains("\x1b[1;7m") && frame.contains("\x1b[22;27m"),
        "the current match is bold+inverse, closed on its own channels: {frame:?}"
    );
    assert!(
        frame.contains("\x1b[4m") && frame.contains("\x1b[24m"),
        "the other match is underlined, closed on its own channel: {frame:?}"
    );
    // **Per-channel closes, and nothing else.** The span is `1;7m` opened and `22;27m`
    // closed: no `\x1b[0m` (which would also drop the row's background, its italic and
    // its hyperlink), no re-wrapped row, and the text between the wrappers is exactly
    // the match. Measured: `" › \x1b[1;7mneedle\x1b[22;27m one…"`.
    for (at, _) in frame.match_indices("\x1b[1;7m") {
        let rest = &frame[at + "\x1b[1;7m".len()..];
        let close = rest.find("\x1b[22;27m");
        let text = close.map_or(rest, |index| &rest[..index]);
        assert!(
            close.is_some(),
            "the bold+inverse span closes on its own channels: {text:?}"
        );
        assert_eq!(text, "needle", "and only the match is styled: {text:?}");
    }
    assert!(
        !frame.contains("\x1b[1;7m\x1b[0m") && !frame.contains("\x1b[4m\x1b[0m"),
        "no span is a reset sandwich: {frame:?}"
    );

    // Selecting the other match moves the styles with it: exactly one span is current.
    chat.handle_key("\r");
    let raw = chat.viewport(80, 24, 0);
    let frame = raw.join("\n");
    assert_eq!(
        frame.matches("\x1b[1;7m").count(),
        1,
        "one current match after the step: {frame:?}"
    );
}

// Verifies: gh #376 gap 4 - **the previous key moves backwards and wraps.** The
// binding (`tui.altScreen.searchPrevious`) has been in the table since it was written
// with no handler; the row asserts the observable (which match is selected), and that
// stepping back from the first wraps to the last.
//
// **The key is a user rebind, on purpose.** The shipped defaults are `shift+enter` and
// `ctrl+shift+g`; a legacy terminal sends `\r` for Enter *and* Shift+Enter, so there is
// no byte to tell them apart (pi has the same constraint). Rebinding through the
// override map a user actually has proves two things at once - the handler asks the
// manager rather than hard-coding a key, and the row runs on every terminal.
#[test]
fn search_previous_steps_backwards_and_wraps() {
    let mut chat = chat_with(&["needle one", "unrelated", "needle two"]);
    let mut bindings = std::collections::BTreeMap::new();
    bindings.insert(
        "tui.altScreen.searchPrevious".to_string(),
        vec!["ctrl+g".to_string()],
    );
    chat.keybindings = std::sync::Arc::new(
        lca_tui::engine::keybindings::KeybindingsManager::with_user_bindings(bindings),
    );
    search(&mut chat, "needle");
    assert_eq!(chat.search_index, 0, "the first match is selected");

    chat.handle_key("\x07"); // Ctrl+G, the rebound previous key
    assert_eq!(
        chat.search_index, 1,
        "from the first, backwards wraps to the last: {}",
        chat.search_index
    );
    chat.handle_key("\x07");
    assert_eq!(chat.search_index, 0, "and back to the first");
}

// ===========================================================================
// The corpus itself, measured rather than assumed.
// ===========================================================================

// Verifies: gh #376's fast-path decision - **the ASCII path exists because the
// measurement demanded it**, and it is the only second path the module carries.
//
// pi has a `charCodeAt` fast path because `Intl.Segmenter` is slow in JS. The first
// version of this corpus measured every character with `visible_width(&ch.to_string())`
// and cost **8.7 s** for a 2,000-row document - 100 k allocations and cache probes,
// several hundred frames' worth. The cheap discriminator turned out to be **ASCII**,
// where a byte offset inside a run *is* a cell offset, so no measurement is needed at
// all; non-ASCII lines still measure (CJK is two cells). Measured after the change:
//
// | rows | build + match |
// |------|---------------|
// | 200  | 4.7 ms |
// | 400  | 8.1 ms |
// | 800  | 14.4 ms |
// | 2000 | 42 ms |
//
// The row pins the **regression**, not a frame budget. Idle it measured 14.4 ms for 800
// rows, but under `nextest`'s parallel load the same work takes ~90 ms - so a
// wall-clock budget tight enough to be meaningful is exactly how this tree has shipped
// flaky rows. The ceiling is therefore two orders of magnitude above the fix and still
// **an order of magnitude below** the pre-fix cost (8.7 s for 2,000 rows), which is the
// claim that matters: the per-character `visible_width` walk is gone.
#[test]
fn the_corpus_indexes_a_realistic_document_quickly() {
    let render = |rows: usize| -> std::time::Duration {
        let lines: Vec<String> = (0..rows)
            .map(|i| format!("row {i}: some text with a needle in the middle of it"))
            .collect();
        let started = std::time::Instant::now();
        let corpus = chat_search_corpus::build(&lines, 0);
        let matches = chat_search_corpus::find(&corpus, "needle");
        assert_eq!(matches.len(), rows, "one match per row");
        started.elapsed()
    };
    let small = render(800);
    assert!(
        small < std::time::Duration::from_secs(1),
        "800 rows build+match (measured 14.4 ms idle, ~90 ms loaded; pre-fix shape took \
         seconds): {small:?}"
    );
    let large = render(2000);
    assert!(
        large < std::time::Duration::from_secs(2),
        "2000 rows build+match (measured 42 ms idle, 8.7 s before the ASCII path): {large:?}"
    );
}
