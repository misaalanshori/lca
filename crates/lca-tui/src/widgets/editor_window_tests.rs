//! gh #382: the composer's window, the autocomplete that survives a horizontal
//! move, and a click that lands where it was aimed.
//!
//! Three defects, and the design call for each was "converge, do not add":
//!
//! * **gap 1** (a 25-line prompt eating the transcript) was already fixed by #388 gap
//!   1 - `lca-ui`'s `editor_view` caps the composer at `max(5, rows*30%)`, follows the
//!   caret, and puts `↑ N more` / `↓ N more` **inside** the existing rule. So this
//!   module pins that behaviour from `lca-tui`'s side rather than adding a second cap
//!   (`editor_view` is in `lca-ui`, which is out of this cycle's boundary).
//! * **gap 2** is *refresh-or-close*: a horizontal move re-queries the provider, and
//!   the picker closes only because the new position yields nothing. pi states it that
//!   way (`moveCursor`'s tail, `editor.ts:1946-1956`) and names the `Tab`-concat bug it
//!   also fixes.
//! * **gap 3** is a clamp: on a **non-last** visual row a click at the row's end must
//!   land on the row's last grapheme, not on the next row's first one - pi's
//!   `if (!isLastSegment && targetIndex === chunk.length) targetIndex = lastGraphemeIndex`
//!   (`editor.ts:688`).

use super::*;
use crate::widgets::autocomplete::{AutocompleteItem, Origin, Suggestions};
use std::sync::Arc;

/// A provider that offers `value` whenever the text before the cursor ends with
/// `trigger` - enough to stand in for `/mod` and `@src/ma` without a filesystem.
struct TriggerProvider {
    trigger: &'static str,
    value: &'static str,
}

impl AutocompleteProvider for TriggerProvider {
    fn get_suggestions(&self, text_before_cursor: &str, _force: bool) -> Option<Suggestions> {
        // The token under the cursor: everything after the last space.
        let token = text_before_cursor.rsplit(char::is_whitespace).next()?;
        if token.starts_with(self.trigger) {
            Some(Suggestions {
                items: vec![AutocompleteItem {
                    value: self.value.to_string(),
                    label: self.value.to_string(),
                    description: None,
                    origin: Origin::Builtin,
                    dim: false,
                }],
                prefix: token.to_string(),
            })
        } else {
            None
        }
    }
}

/// An editor with a provider that fires on `/`-tokens, holding `before`.
fn with_slash_provider(before: &str) -> Editor {
    let mut e = Editor::new();
    e.set_autocomplete(Arc::new(TriggerProvider {
        trigger: "/",
        value: "/model",
    }));
    e.insert_str(before);
    e.refresh_suggestions(true);
    e
}

// ---------------------------------------------------------------------------
// gap 2: a horizontal move refreshes the popup, it does not clear it
// ---------------------------------------------------------------------------

// Verifies: gh #382 gap 2 - **`Left` back into a completed token re-filters instead of
// closing the popup.**
//
// The defect: every horizontal move called `clear_suggestions()`, so arrowing left from
// `/model` back into `/mod` closed the completion instead of narrowing it. The fix is
// pi's *refresh-or-close*: re-query at the new position and let "no suggestions" be the
// only thing that closes it.
#[test]
fn left_inside_a_token_refilters_the_popup() {
    let mut e = with_slash_provider("/mod");
    assert!(
        e.suggestions().is_some(),
        "the provider offers a suggestion for `/mod`"
    );
    e.cursor_left();
    assert_eq!(e.cursor_col, 3, "the caret moved into the token");
    assert!(
        e.suggestions().is_some(),
        "the popup is still open (re-filtered), not cleared"
    );
}

// Verifies: gh #382 gap 2 - **`Right` out of the token closes it.**
//
// The other half, and the one that stops the fix being "never close". My first version
// inserted a trailing space with `insert_str`, which clears the popup on its own - so it
// passed against the "never close" mutation and proved nothing. This one moves the
// caret with `cursor_right` only, so the *move* is what has to decide.
#[test]
fn right_out_of_the_token_closes_the_popup() {
    // The buffer already holds the separator, so no edit happens between the two
    // observations - only a move.
    let mut e = with_slash_provider("/mod");
    e.insert_str(" x");
    // `insert_str` clears the popup, so open it explicitly at the token's end - after
    // the edit, so the moves below are the only thing deciding.
    e.cursor_col = 4;
    e.refresh_suggestions(true);
    assert!(
        e.suggestions().is_some(),
        "open while the caret is in `/mod`"
    );
    e.cursor_right(); // onto the space, which ends the token
    e.cursor_right(); // into the `x`, a token the provider does not answer
    assert!(
        e.suggestions().is_none(),
        "once the caret leaves the token the popup closes, driven by the move alone"
    );
}

// Verifies: gh #382 gap 2 - **an empty next token is no token**, so the close is the
// provider's answer rather than a rule about spaces.
#[test]
fn a_space_ends_the_token_for_the_provider_too() {
    let mut e = Editor::new();
    e.set_autocomplete(Arc::new(TriggerProvider {
        trigger: "/",
        value: "/model",
    }));
    e.insert_str("/mod ");
    e.refresh_suggestions(true);
    assert!(
        e.suggestions().is_none(),
        "a trailing space yields an empty token, which the provider does not answer"
    );
}

// Verifies: gh #382 gap 2 - **`Left` out of a token closes it**, so the horizontal move
// is what re-decides rather than a rule keyed to direction.
#[test]
fn left_out_of_the_token_closes_the_popup() {
    let mut e = with_slash_provider("/mod");
    // Put the caret at column 0 - before the token - and step further left.
    e.cursor_col = 0;
    e.cursor_left();
    assert!(
        e.suggestions().is_none(),
        "with the caret before the token there is nothing to complete"
    );
}

// Verifies: gh #382 gap 2 - **the popup's list actually narrows** as the caret moves,
// rather than merely staying open.
//
// "Still open" is the weak assertion; this one asks the provider again at the new
// position and compares. A fix that kept the *stale* suggestion set would pass
// `left_inside_a_token_refilters_the_popup` and still show the wrong rows.
#[test]
fn the_refreshed_popup_matches_the_new_caret_position() {
    let mut e = with_slash_provider("/mod");
    let at_four = e.suggestions().expect("open at `/mod`").items[0]
        .value
        .clone();
    e.cursor_left();
    let at_three = e.suggestions().expect("open at `/mo`").items[0]
        .value
        .clone();
    assert_eq!(at_four, at_three, "the same candidate, re-derived");
    // The prefix is what tells them apart: it must describe the *new* position.
    let prefix = e.suggestions().expect("open").prefix.clone();
    assert_eq!(
        prefix, "/mo",
        "the suggestion set was recomputed for the caret's new token, not carried over"
    );
}

// Verifies: gh #382 gap 2 - **vertical movement re-derives too, on the same rule.**
//
// pi's `moveCursor` tail covers both axes with one statement, so Up/Down must not keep
// a *different* policy from Left/Right. The assertion is therefore the rule, not a
// direction: a vertical move that leaves the token closes the popup, and one that stays
// in it re-filters. My first version of this row asserted "still open" after moving
// away to a line with no `/`-token - which is the *correct* close, so the row was
// testing the wrong thing.
#[test]
fn vertical_movement_uses_the_same_refresh_or_close_rule() {
    let mut e = Editor::new();
    e.set_autocomplete(Arc::new(TriggerProvider {
        trigger: "/",
        value: "/model",
    }));
    // Two lines, both carrying a `/`-token, so a vertical move stays inside one.
    e.insert_str("/mod\n/modx");
    let _ = e.render(40);
    e.cursor_col = 0;
    e.cursor_up(); // line 0, column 0
    e.cursor_col = 4;
    e.refresh_suggestions(true);
    assert!(e.suggestions().is_some(), "open at `/mod`");

    // Down to the second line. Its first visual position is column 0, where there is
    // no token yet - so the rule closes, which is right. Moving along that line to its
    // own `/`-token then re-opens, because the popup is derived from where the caret is
    // rather than from the direction it came.
    e.cursor_down();
    assert!(
        e.suggestions().is_none(),
        "column 0 of `/modx` has no token yet"
    );
    e.cursor_col = 0;
    e.cursor_right();
    e.cursor_right();
    e.refresh_suggestions(true);
    assert!(
        e.suggestions().is_some(),
        "and the popup re-opens on this line's own token: before={:?}",
        e.text_before_cursor()
    );

    // And leaving both tokens closes it, which is the same rule as Left/Right.
    e.cursor_col = 0;
    e.cursor_up();
    e.cursor_col = 1;
    e.refresh_suggestions(false);
    e.cursor_left();
    assert!(
        e.suggestions().is_none(),
        "with the caret before any token there is nothing to complete"
    );
}

// ---------------------------------------------------------------------------
// gap 3: a click on a wrapped continuation row lands on that row
// ---------------------------------------------------------------------------

/// A line long enough to wrap several times at width 20, and the width it was laid
/// out at.
fn wrapped_editor() -> Editor {
    let mut e = Editor::new();
    let long: String = (0..40).map(|i| format!("word{i} ")).collect();
    e.insert_str(long.trim_end());
    let _ = e.render(20);
    e
}

/// Which rendered row carries the caret marker.
fn painted_row(e: &Editor) -> usize {
    e.render(20)
        .iter()
        .position(|r| r.contains(CURSOR_MARKER))
        .unwrap_or(usize::MAX)
}

// Verifies: gh #382 gap 3 - **a click at a non-last row's end paints the caret on that
// row, not the next one.**
//
// The defect: `row.end` on a non-last segment is the *next* segment's start, so a click
// on the row's last cell placed the caret at a column belonging to the following row -
// visibly one row low. pi's rule is `last ? len : len - 1` (`editor.ts:688`).
#[test]
fn a_click_at_a_continuation_rows_end_stays_on_that_row() {
    let e = wrapped_editor();
    let first_len = e.visual_rows()[0].len();
    // The row's last cell: one less than the length, and its end boundary too.
    for col in [first_len - 1, first_len, first_len + 1] {
        let mut c = wrapped_editor();
        c.handle_click(col, 0);
        assert_eq!(
            painted_row(&c),
            0,
            "a click at column {col} of row 0 paints the caret on row 0, not the \
             continuation (row 0 is {first_len} cells)"
        );
    }
}

// Verifies: gh #382 gap 3 - **a click in the middle of a continuation row lands there**,
// which is the case that already worked - pinned so the clamp cannot break it.
#[test]
fn a_click_inside_a_continuation_row_lands_on_it() {
    let mut e = wrapped_editor();
    let row = 2;
    e.handle_click(8, row);
    assert_eq!(
        painted_row(&e),
        row,
        "the caret paints on the row that was clicked"
    );
}

// Verifies: gh #382 gap 3 - **a click past the end of the *last* row clamps to the
// line's end**, the one case the brief says is already right.
//
// It is pinned because the non-last clamp is written beside it: a `len - 1` applied
// unconditionally would stop the caret reaching the end of the buffer, which is the
// regression a careless fix ships.
#[test]
fn a_click_past_the_last_row_clamps_to_the_line_end() {
    let mut e = wrapped_editor();
    let last = e.visual_rows().len() - 1;
    let len = e.visual_rows()[last].len();
    e.handle_click(len + 50, last);
    assert_eq!(
        e.cursor_col,
        e.text().chars().count(),
        "past the end of the last row is the end of the buffer"
    );
    assert_eq!(painted_row(&e), last, "and it paints on the last row");
}

// Verifies: gh #382 gap 3 - **a single-row line still accepts a click at its end.**
//
// The degenerate case: one visual row *is* the last row, so the clamp must not apply.
#[test]
fn a_click_at_the_end_of_a_one_row_line_reaches_its_end() {
    let mut e = Editor::new();
    e.insert_str("hello");
    let _ = e.render(20);
    e.handle_click(5, 0);
    assert_eq!(e.cursor_col, 5, "the caret sits after the last character");
}

// ---------------------------------------------------------------------------
// gap 1: the composer's window (#388 gap 1's cap, pinned from this side)
// ---------------------------------------------------------------------------

// Verifies: gh #382 gap 1 - **the editor still renders every row**, and the cap is the
// renderer's job, not the widget's.
//
// This is the convergence row. Two caps that disagree is the class this repo keeps
// fixing, so the arrangement is stated: `Editor::render` is total (it is also what
// `visual_rows` and the click hit-test walk), and the *window* is `lca-ui`'s
// `editor_view`, which caps at `max(5, rows*30%)`, follows the caret and reports the
// hidden counts the badges need. A cap added here instead would break `handle_click`'s
// row addressing and the transcript's separator arithmetic in one move.
#[test]
fn the_editor_renders_every_row_and_the_window_is_the_renderers() {
    let mut e = Editor::new();
    let big = (0..40)
        .map(|n| format!("LINE-{n:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_str(&big);
    let rows = e.render(80);
    assert_eq!(
        rows.len(),
        40,
        "the widget is total: capping here would make `handle_click`'s row indices \
         and the visual-row map disagree with what is on screen"
    );
    // And the caret is present exactly once, so a buyer of this render can find it.
    let markers = rows.iter().filter(|r| r.contains(CURSOR_MARKER)).count();
    assert_eq!(markers, 1, "exactly one caret marker in the rendered set");
}
