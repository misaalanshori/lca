//! Gh #382: the composer's window, the autocomplete that survives a horizontal move,
//! and a click that lands where it was aimed. NFR-24's guard.
//!
//! The behavioural rows live in `crates/lca-tui/src/widgets/editor_window_tests.rs`.
//! This file pins the three seams that would let each defect come back silently:
//!
//! 1. **one cap, in the renderer** - `Editor::render` is total, and the *window* is
//!    `lca-ui`'s `editor_view` at `max(5, rows*30%)`. Two caps that disagree is the
//!    class this repo keeps fixing, so the split is asserted rather than described.
//! 2. **one refresh-or-close helper** - every caret move goes through it, so no arm can
//!    quietly go back to clearing.
//! 3. **one clamp predicate** - the non-last row's end belongs to the next row, and
//!    `is_last_row_of_line` is what decides which case a click is in.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

/// Source with `//` comment lines removed, so a row about *code* is neither satisfied
/// nor defeated by prose.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of one function, found by brace matching from its signature.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        match byte {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    return &source[start..start + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("{signature} is not closed");
}

// Verifies: gh #382 gap 1 - **the cap lives in the renderer, and the widget stays
// total.**
//
// This is the convergence row the brief asks for. #388 gap 1 already capped the composer
// at pi's `max(5, rows*30%)` in `lca-ui`'s `editor_view`, following the caret and putting
// `↑ N more` / `↓ N more` inside the existing rule. A second cap in `Editor::render`
// would break `handle_click`'s row addressing, the visual-row map the movements walk, and
// the separator's hidden-count arithmetic in one move - so the split is pinned.
#[test]
fn the_window_is_the_renderers_and_the_widget_stays_total() {
    let editor = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    let render = function_body(&editor, "pub fn render(&self");
    assert!(
        !render.contains("skip(") && !render.contains("take("),
        "the widget's render is total - no windowing here: {render}"
    );
    assert!(
        !render.contains("3 / 10") && !render.contains("* 3"),
        "and it holds no cap arithmetic of its own: {render}"
    );

    // The window and its cap are one function, in the renderer.
    let chat = code_only(&read("crates/lca-ui/src/chat_render.rs"));
    let view = function_body(&chat, "pub(crate) fn editor_view(");
    assert!(
        view.contains("rows * 3 / 10") && view.contains(".max(5)"),
        "the one cap is pi's 30%, never fewer than five: {view}"
    );
    assert!(
        view.contains("CURSOR_MARKER"),
        "and the offset follows the caret, which is the invariant a cap can break: \
         {view}"
    );
    // Zero-extra-row indicators: the counts ride the separator, they are not rows.
    assert!(
        chat.contains("render_top_with(width, &self.theme, &border, hidden_above)"),
        "the `↑ N more` count goes into the existing top rule, not a new row"
    );
    assert!(
        chat.contains("render_bottom_with(width, &border, hidden_below)"),
        "and `↓ N more` into the bottom one"
    );
}

// Verifies: gh #382 gap 2 - **every caret move goes through the one refresh-or-close
// helper**, so no arm can drift back to `clear_suggestions()`.
//
// The defect was four separate `clear_suggestions()` calls. A fix applied to the
// horizontal arms only would leave Up/Down clearing, which is why the guard is about
// *all four* moves rather than the two the issue names.
#[test]
fn every_caret_move_refreshes_or_closes() {
    // Both files: the helper lives in `editor_suggestions.rs` (split out at the ceiling)
    // while the moves stay in `editor.rs`. A guard that read only one file would pass
    // while half the seam moved house - the failure the #381 guards hit twice.
    let editor = format!(
        "{}\n{}",
        code_only(&read("crates/lca-tui/src/widgets/editor.rs")),
        code_only(&read("crates/lca-tui/src/widgets/editor_suggestions.rs"))
    );
    let editor = editor.as_str();
    for signature in [
        "pub fn cursor_left(&mut self",
        "pub fn cursor_right(&mut self",
        "pub fn cursor_up(&mut self",
        "pub fn cursor_down(&mut self",
    ] {
        let body = function_body(editor, signature);
        assert!(
            body.contains("refresh_or_close_suggestions("),
            "{signature} asks the shared helper: {body}"
        );
        assert!(
            !body.contains("clear_suggestions("),
            "{signature} does not clear on a move - the close is the provider's empty \
             answer, not the fact that the caret moved: {body}"
        );
    }

    // The helper itself really is refresh-then-close, not a rename of `clear`.
    let helper = function_body(editor, "fn refresh_or_close_suggestions(&mut self");
    assert!(
        helper.contains("refresh_suggestions(false)"),
        "it re-queries at the new position: {helper}"
    );
    assert!(
        helper.contains("clear_suggestions()"),
        "and closes only when the answer is empty: {helper}"
    );
    // `force = false`: a move is not an explicit completion request.
    assert!(
        !helper.contains("refresh_suggestions(true)"),
        "a move must not force the provider to fire: {helper}"
    );
}

// Verifies: gh #382 gap 3 - **the clamp is decided by the one "last row" predicate.**
//
// `row.end` on a non-last segment is the next segment's start, so a click on this row's
// last cell used to place the caret one row low. The fix is `last ? len : len - 1`, and
// the "last" question must be asked of `is_last_row_of_line` - the same predicate the
// wrapper and the caret use - or the two notions of "last" drift.
#[test]
fn the_click_clamp_asks_the_one_last_row_predicate() {
    let editor = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    let click = function_body(&editor, "pub fn handle_click(");
    assert!(
        click.contains("is_last_row_of_line(&rows, row)"),
        "the last-row question is asked of the shared predicate: {click}"
    );
    // Both arms present: the clamp must not be applied unconditionally (that pins the
    // caret one short of the buffer's end).
    assert!(
        click.contains("visual.end.saturating_sub(1)"),
        "a non-last row clamps to its last grapheme: {click}"
    );
    let last_arm = click
        .find("visual.end\n")
        .or_else(|| click.find("            visual.end,"))
        .unwrap_or_else(|| panic!("the last-row arm keeps the full end: {click}"));
    let clamp_at = click
        .find("visual.end.saturating_sub(1)")
        .expect("the clamp exists");
    assert!(
        last_arm < clamp_at,
        "the last-row arm and the clamp are separate arms, in that order: {click}"
    );
}
