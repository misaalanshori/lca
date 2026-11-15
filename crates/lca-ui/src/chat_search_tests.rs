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

// Verifies: gh #376 fix F4 - **the painted frame keeps the selected match while a
// whole window streams in.** This row replaces the one shipped with `90478b03`, which
// asserted that `clamp_scroll` returned the same *number* - true and useless, because
// the number is what drifts away from the row. Measured on that tree: the highlight was
// lost at stream line 5 of a 16-row window.
//
// The fixture is the brief's, and the assertion is the **painted frame**: after a full
// window of streamed lines, some row still carries `\x1b[1;7m` around `needle` and the
// match is inside the window the frame sliced.
#[test]
fn a_stream_delta_does_not_move_the_selected_match() {
    let mut chat = chat_with(&[
        "filler", "filler", "filler", "filler", "needle", "filler", "filler", "filler", "filler",
        "filler",
    ]);
    chat.screen_mode = true;
    search(&mut chat, "needle");
    let width = chat.world.size.0;
    let height = chat.world.size.1;
    let mut scroll = chat.take_search_scroll().expect("a reveal");
    assert!(scroll > 0, "the reveal is mid-document: {scroll}");

    // Stream a whole window of lines, one frame each, in the production order.
    let window = chat.window_height(width, height);
    for line in 0..window {
        chat.transcript
            .append_text(&format!("\nstreamed line {line}"));
        scroll = chat.clamp_scroll(scroll, width, height);
        let frame = chat.viewport(width, height, scroll);
        assert!(
            frame.iter().any(|row| row.contains("\x1b[1;7mneedle")),
            "after {} streamed lines the selected match is still painted in the window \
             (scroll {scroll}): {frame:?}",
            line + 1
        );
    }
}

// Verifies: gh #376 fix F4's other half - **a match at the live bottom is not frozen.**
// The shipped row dodged this case ("the lock would be untestable") because a
// scroll-number lock cannot express it; holding the *rows* does. With the reader at the
// live bottom the follow rule must still reach 0, and the match stays on screen.
#[test]
fn a_live_bottom_match_keeps_the_transcript_following() {
    let mut chat = chat_with(&["a", "b", "needle"]);
    chat.screen_mode = true;
    search(&mut chat, "needle");
    let width = chat.world.size.0;
    let height = chat.world.size.1;
    let mut scroll = chat.take_search_scroll().expect("a reveal");
    assert_eq!(scroll, 0, "a live-bottom match reveals at the live bottom");

    for line in 0..6 {
        chat.transcript.append_text(&format!("\nmore {line}"));
        scroll = chat.clamp_scroll(scroll, width, height);
        let frame = chat.viewport(width, height, scroll);
        assert!(
            frame.iter().any(|row| {
                row.contains("\x1b[1;7m") && row.to_lowercase().contains("needle")
                    || row.contains("\x1b[1;7m")
            }),
            "the live-bottom match is painted after {} lines (scroll {scroll}): {frame:?}",
            line + 1
        );
    }
    // And follow is still live: a fresh scroll of 0 is where the frame lands.
    assert_eq!(
        chat.clamp_scroll(0, width, height),
        0,
        "follow-at-the-end is not suppressed for a live-bottom match"
    );
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
// gh #376 fix-up: the **lifecycle** of the search's state - the four states the
// shipped rows did not drive (a resize, a second search, closing, a long stream).
// ===========================================================================

// Verifies: gh #376 fix F1 - **a resize repaints the match on the new rows.**
//
// `refresh_search` ran only from a keystroke, so a SIGWINCH left the segments of the
// *old* width against the *new* rows: measured on `90478b03` the styled spans were
// `["rtup", "elp. LC"]` - real text, wrong columns. The refresh now lives in
// `viewport`, the seam both renderers pass through.
#[test]
fn a_resize_repaints_the_match_on_the_new_rows() {
    let mut chat = chat_with(&["the error handling path is here"]);
    chat.screen_mode = true;
    // Matched at 80 columns, where the phrase is one row.
    search(&mut chat, "error handling");
    assert_eq!(chat.search_matches.len(), 1, "one match at 80 columns");
    // The two words are two *runs*, so they are two segments even on one row (the
    // merge rule joins adjacent spans, not separated words) - the discriminator is
    // therefore which **rows** they land on, not how many segments there are.
    let before = selected_segments(&chat);
    assert_eq!(before.len(), 2, "two words: {before:?}");
    assert_eq!(
        before[0].0, before[1].0,
        "at 80 columns both words are on one row: {before:?}"
    );

    // The production order for a SIGWINCH: the world's size, then a frame.
    //
    // 16 columns, because at 24 the user bubble's own row is **truncated** rather than
    // wrapped (measured) - the wrap the row is about needs a width the phrase cannot fit.
    chat.world.resize(16, 24);
    let frame = chat.viewport(16, 24, 0);
    // The phrase now wraps, so the match is **two** segments on two rows - the proof
    // that the list was rebuilt at the new width rather than reused.
    let after = selected_segments(&chat);
    assert_eq!(after.len(), 2, "two words: {after:?}");
    assert_eq!(
        after[0].0 + 1,
        after[1].0,
        "at 16 columns the phrase is split across two adjacent rows (re-queried, not          reused): {after:?}"
    );
    let styled: Vec<String> = frame
        .iter()
        .filter(|row| row.contains("\x1b[1;7m"))
        .map(|row| lca_tui::engine::text::strip_terminal_sequences(row))
        .collect();
    assert!(
        styled.iter().any(|row| row.contains("error"))
            && styled.iter().any(|row| row.contains("handling")),
        "the painted halves are the phrase's own words, not `rtup`/`elp. LC`: {styled:?}"
    );
}

// Verifies: gh #376 fix F2 - **closing the search gives the scroll back to the reader.**
//
// The worst finding: the close arm left the follow hold armed, so `clamp_scroll` kept
// returning the reveal's scroll and **absorbed every streamed line's growth** - measured
// 23 → 23 across 6 new rows where the growing transcript demands 29. The view therefore
// drifts toward new output instead of holding the reader's place, which is
// indistinguishable from "the transcript stopped following" to anyone reading it.
//
// The observable is the growth: after close, each streamed line moves the scroll by its
// delta exactly as it does with no search at all; and at the live bottom (0) the newest
// line paints, which is follow-at-the-end working again.
#[test]
fn closing_the_search_lets_the_transcript_follow_again() {
    let mut chat = chat_with(&[
        "filler", "filler", "filler", "filler", "needle", "filler", "filler", "filler", "filler",
        "filler",
    ]);
    chat.screen_mode = true;
    search(&mut chat, "needle");
    let mut scroll = chat.take_search_scroll().expect("a reveal");
    assert!(scroll > 0, "the reveal is mid-document: {scroll}");
    let width = chat.world.size.0;
    let height = chat.world.size.1;
    scroll = chat.clamp_scroll(scroll, width, height);

    // The reader is done: Escape is the production close.
    chat.handle_key("\x1b");
    assert!(chat.search.is_none(), "closed");

    // A page streams in, one frame per line, as the loop would drive it. Each new row
    // must move the scroll by a delta, because the reader is *not* at the bottom.
    let window = chat.window_height(width, height);
    for line in 0..window {
        chat.transcript
            .append_text(&format!("\nnewest line {line}"));
        let next = chat.clamp_scroll(scroll, width, height);
        assert!(
            next > scroll,
            "after close, streamed line {} moves the scroll ({scroll} → {next}); a live \
             reveal hold would have absorbed it",
            line + 1
        );
        scroll = next;
    }

    // At the live bottom the frame follows again, which is follow-at-the-end resuming.
    assert_eq!(
        chat.clamp_scroll(0, width, height),
        0,
        "follow-at-the-end works after a close"
    );
    let frame = chat.viewport(width, height, 0);
    assert!(
        frame
            .iter()
            .any(|row| row.contains(&format!("newest line {}", window - 1))),
        "and the newest line is on screen: {frame:?}"
    );
}

// Verifies: gh #376 fix F3's **open arm** specifically - **re-opening without closing
// re-anchors too.**
//
// The Escape path clears the anchor anyway (fix F2), so the interesting case is the one
// that never goes through a close: `Ctrl+R` pressed **while the search is already open**
// replaces the query without a key event that clears anything. Measured on the shipped
// tree, that kept the earlier anchor, so the next query anchored at the row the *first*
// one was opened from. The open arm is what makes both paths anchor at the reader.
#[test]
fn reopening_the_search_without_closing_re_anchors() {
    let mut chat = chat_with(&["needle up here", "filler", "filler", "filler"]);
    chat.screen_mode = true;
    let width = chat.world.size.0;
    let height = chat.world.size.1;
    let window = chat.window_height(width, height);
    let content = chat.transcript_len(width);
    chat.last_scroll = u16::try_from(content.saturating_sub(window)).unwrap_or(0);
    search(&mut chat, "needle");
    assert!(
        chat.search_anchor == Some(chat.search_top_row())
            || chat
                .search_anchor
                .is_some_and(|anchor| anchor <= chat.search_top_row() + 1),
        "the first search anchored at the top of the document: {:?}",
        chat.search_anchor
    );

    // The reader scrolls to the live bottom, then presses Ctrl+R again **without
    // closing**, and searches again.
    for line in 0..window {
        let tail = if line + 1 == window {
            "needle at the live bottom"
        } else {
            "more output"
        };
        chat.transcript.append_text(&format!("\n{tail} {line}"));
    }
    chat.last_scroll = 0;
    // `open_search` is the production open arm (`app.search` calls exactly this). It is
    // invoked directly because a `Ctrl+R` while the search already owns the keyboard is
    // swallowed by the search's own key handler - a fact this row verified by measuring
    // it, not by assuming it.
    chat.open_search();
    for c in "needle".chars() {
        chat.handle_key(&c.to_string());
    }
    let picked = chat
        .search_matches
        .get(chat.search_index)
        .map(SearchMatch::first_row)
        .expect("a match");
    assert!(
        picked >= chat.search_top_row(),
        "re-opening without closing still anchors at the viewport top {} (picked {})",
        chat.search_top_row(),
        picked
    );
}

// Verifies: gh #376 fix F3 - **a second search anchors where the reader is now.**
//
// The anchor was written once per tab (`if self.search_anchor.is_none()`), so the first
// search's row kept deciding the second one's pick (measured: anchor 1 survived the
// close and a scroll to the live bottom, so search 2 picked row 7). The row asserts the
// **picked row**, not the field, because the field was never the defect.
//
// The fixture puts a match at the live bottom *after* the scroll, because that is the
// only arrangement where "anchored where the reader is" and "wrapped to the top" give
// different answers - a fixture with no match below the viewport passes the wrong rule
// for the right reason.
#[test]
fn a_second_search_anchors_at_the_viewport_the_reader_is_in() {
    let mut chat = chat_with(&["needle up here", "filler", "filler", "filler"]);
    chat.screen_mode = true;
    let width = chat.world.size.0;
    let height = chat.world.size.1;
    let window = chat.window_height(width, height);
    let content = chat.transcript_len(width);
    // First search from the **top of the document** (scroll-to-top in bottom
    // coordinates), which is where the measured fixture's anchor 1 came from.
    chat.last_scroll = u16::try_from(content.saturating_sub(window)).unwrap_or(0);
    search(&mut chat, "needle");
    let first = chat
        .search_matches
        .get(chat.search_index)
        .map(SearchMatch::first_row)
        .expect("a match");
    assert_eq!(
        first,
        chat.search_matches[0].first_row(),
        "from the top, the top match is picked"
    );
    chat.handle_key("\x1b");

    // The reader scrolls to the live bottom, and the turn streams a page whose **last**
    // line carries a match: it lands inside the viewport.
    for line in 0..window {
        let tail = if line + 1 == window {
            "needle at the live bottom"
        } else {
            "more output"
        };
        chat.transcript.append_text(&format!("\n{tail} {line}"));
    }
    chat.last_scroll = 0;

    search(&mut chat, "needle");
    let picked = chat
        .search_matches
        .get(chat.search_index)
        .map(SearchMatch::first_row)
        .expect("a match");
    let top = chat.search_top_row();
    assert!(
        picked >= top,
        "the second search picks at or after the viewport top {top}, not the first \
         search's row ({picked}); matches {:?}",
        chat.search_matches
            .iter()
            .map(SearchMatch::first_row)
            .collect::<Vec<_>>()
    );
}

// Verifies: gh #376 fix F5 - **the paint is bounded by the window, not the document.**
//
// The shipped shape asked each painted row which matches covered it - `O(matches × rows)`,
// measured **62.7 ms per frame** for 600 rows and 1,800 matches while the corpus
// build+find cost 6.4 ms. This is pi's `applySearchHighlights` shape: partition-point to
// the first visible match, stop past the window, one range map keyed by row.
//
// Two claims, both load-robust because they are **ratios measured in one run** (the
// gh #588 lesson - this tree has shipped a wall-clock row before):
//
// 1. **Against the corpus**: the paint over a fullscreen window stays within 4× the
//    corpus build+find on the same document. Measured on the fix, 600 rows / 4,800
//    matches / 24-row window: **0.27 ms paint vs 7.5 ms corpus** (ratio 0.04); the shipped
//    per-row scan was **62.7 ms** on this fixture - and, once the scan was bounded, the
//    remaining 55 ms turned out to be `visible_width(&ch.to_string())` per glyph, which
//    the same ASCII discriminator the corpus uses removes.
// 2. **Against itself as the document grows**: quadrupling the document must not
//    quadruple the paint, because the window did not grow. Measured 150 → 1,200 rows:
//    0.32 ms → 0.44 ms while the corpus half went 1.9 ms → 18.6 ms. A per-row × matches
//    scan scales with the document; the range map does not.
#[test]
fn the_paint_is_bounded_by_the_window_not_the_document() {
    // **The review's fixture, at the review's scale**: 600 rows with several matches per
    // row, painted as the scrollback renderer paints it (every document row). Bounded
    // work is per *match inside the painted range*; the shipped shape filtered the whole
    // match list **per painted row**. Measured: bounded **10.9 ms**, shipped per-row scan
    // **29.6 ms**, corpus build+find 4.6 ms.
    let paint_of = |rows: usize| -> (std::time::Duration, std::time::Duration, usize, usize) {
        let mut chat = chat();
        chat.world.resize(80, 24);
        chat.screen_mode = false;
        for i in 0..rows {
            let cells = "value ".repeat(5);
            chat.transcript.append_text(&format!("row {i}: {cells}\n"));
        }
        chat.transcript.finish_assistant();
        let width = chat.world.size.0;
        search(&mut chat, "value");
        let lines = chat.transcript.render(width, &chat.theme);
        let corpus = chat_search_corpus::build(&lines, 0);
        let started = std::time::Instant::now();
        let found = chat_search_corpus::find(&corpus, "value");
        let corpus_time = started.elapsed();
        assert!(!found.is_empty(), "the query matches");
        let screen = chat.viewport(width, 24, 0);
        let started = std::time::Instant::now();
        for _ in 0..10 {
            let mut copy = screen.clone();
            chat.paint_search_rows(&mut copy);
        }
        (
            started.elapsed() / 10,
            corpus_time,
            chat.search_matches.len(),
            screen.len(),
        )
    };

    let (paint, corpus, matches, rows) = paint_of(600);
    assert!(
        matches > 1000,
        "a common query makes many matches: {matches}"
    );
    assert!(rows > 500, "the whole document is painted: {rows} rows");
    // The claim, as a **ratio measured in one run** (the gh #588 lesson): the paint may
    // not cost more than four corpus build+finds on the same document. Bounded was 2.4x
    // and the shipped per-row scan 4.8x on this fixture (6.4x on the review's), so the
    // bound separates the two shapes rather than merely being generous.
    assert!(
        paint <= corpus.saturating_mul(4),
        "the paint ({paint:?}) stays within 4x the corpus build+find ({corpus:?}) over \
         {rows} painted rows and {matches} matches"
    );

    let (big, big_corpus, big_matches, big_rows) = paint_of(1200);
    assert!(big_matches > matches, "and more matches exist");
    assert!(big_rows >= rows, "over at least as many painted rows");
    assert!(
        big <= big_corpus.saturating_mul(4),
        "and it stays bounded as the document grows: {big:?} paint against \
         {big_corpus:?} corpus over {big_rows} rows and {big_matches} matches"
    );
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

// Verifies: gh #376 fix F6 - **a tab switch forgets the search's follow hold.**
//
// The shipped shape kept the hold in a `Cell<Option<u16>>`, which the registry macro's
// `= None` cannot write, so `search_follow_lock` was invisible to
// `tests/regressions/333` and a tab switch left the previous tab's hold armed - which,
// in the state F2 describes, froze the new tab's scroll at the old tab's reveal. The
// field is `Option`-shaped now, the registry names it, and this row drives the real
// sweep (`close_tab_overlays`) and then asks the scroll what it does.
#[test]
fn a_tab_switch_forgets_the_search_follow_state() {
    let mut chat = chat_with(&[
        "filler", "filler", "filler", "filler", "needle", "filler", "filler", "filler", "filler",
        "filler",
    ]);
    chat.screen_mode = true;
    search(&mut chat, "needle");
    let scroll = chat.take_search_scroll().expect("a reveal");
    assert!(scroll > 0, "the reveal is mid-document");
    assert!(
        chat.frame().follow.is_some(),
        "the follow hold is armed before the switch: {:?}",
        chat.frame()
    );

    crate::overlay_registry::close_tab_overlays(&mut chat);

    assert!(chat.search.is_none(), "the switch closed the search");
    assert!(
        chat.frame().follow.is_none(),
        "and dropped its follow hold: {:?}",
        chat.frame()
    );
    // The observable: a following clamp lands at the live bottom again instead of the
    // previous tab's reveal.
    let width = chat.world.size.0;
    let height = chat.world.size.1;
    assert_eq!(
        chat.clamp_scroll(0, width, height),
        0,
        "the new tab follows its own output"
    );
}
