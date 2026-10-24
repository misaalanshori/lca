//! The fullscreen viewport's own keys (gh #375 gaps 4 and 5): plain `Home`/`End`
//! belong to the editor, `Ctrl+Home`/`Ctrl+End` to the viewport, and
//! `PageUp`/`PageDown` page by pi's fraction.
//!
//! Split out of `run_tests.rs` when that file crossed gate 11's 1,200-line
//! ceiling with these rows in it. Behaviour unchanged.
//!
//! The collision these rows pin: `home` was bound both to
//! `tui.editor.cursorLineStart` and to `tui.altScreen.top`, while `run` returned
//! early on the viewport's matcher **before** `chat.handle_key`, so `End` always
//! jumped to the live bottom and `Home` could teleport the view instead of moving
//! the caret. The split is pi's, adopted rather than re-invented.

use super::tests::{chat, options};
use super::*;
use lca_tui::engine::terminal::FakeTerminal;

/// A fullscreen chat with a long transcript, a non-empty prompt, and a screen the
/// caller has already scrolled away from the live bottom.
pub(crate) fn scrolled_fullscreen_with_draft() -> (crate::chat::Chat, Screen, FakeTerminal) {
    let mut chat = super::tests::chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    chat.editor.set_text("half-typed prompt");
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    screen.set_scroll(12);
    (chat, screen, term)
}

pub(crate) fn press(
    key: &str,
    chat: &mut crate::chat::Chat,
    screen: &mut Screen,
    term: &mut FakeTerminal,
) -> InputResult {
    let (input_tx, _input_rx) = std::sync::mpsc::channel();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    handle_input(
        key, chat, screen, term, &input_tx, &resize_tx, None, &None, &mut false,
    )
}

/// Where the caret sits in the editor, as a `(line, column-in-cells)` pair read
/// from the painted `CURSOR_MARKER` - the same surface the user sees.
pub(crate) fn caret_cells(chat: &crate::chat::Chat) -> Option<(usize, usize)> {
    let marker = lca_tui::engine::core::CURSOR_MARKER;
    let rows = chat.editor.render(80);
    for (row_index, row) in rows.iter().enumerate() {
        if let Some(at) = row.find(marker) {
            let before: String = row[..at].to_string();
            let visible = lca_tui::engine::text::strip_terminal_sequences(&before);
            return Some((row_index, lca_tui::engine::text::visible_width(&visible)));
        }
    }
    None
}

// ============================================================================
// gh #375 gap 4 - plain Home/End belong to the editor, Ctrl+Home/End to the view
// ============================================================================

// Verifies: gh #375 gap 4 - plain `Home` in a fullscreen session with a
// non-empty prompt moves the caret to the start of the line and does **not**
// scroll the transcript. This is the row that bites: `tui.altScreen.top` used
// to claim `home` too, and the viewport's matcher ran before the editor's.
#[test]
fn plain_home_moves_the_caret_and_leaves_the_viewport_alone() {
    let (mut chat, mut screen, mut term) = scrolled_fullscreen_with_draft();
    let before_scroll = screen.scroll();
    let before_caret = caret_cells(&chat).expect("the caret is painted");
    assert!(
        before_caret.1 > 0,
        "the fixture must put the caret away from the line start"
    );

    press("\x1b[H", &mut chat, &mut screen, &mut term);

    assert_eq!(
        caret_cells(&chat).map(|(_, col)| col),
        Some(0),
        "Home moves the caret to the start of the line"
    );
    assert_eq!(
        screen.scroll(),
        before_scroll,
        "Home must not scroll the transcript"
    );
    assert_eq!(
        chat.editor.text(),
        "half-typed prompt",
        "and it does not edit the buffer"
    );
}

// Verifies: gh #375 gap 4 - plain `End` moves the caret to the end of the line
// and does not jump the viewport to the live bottom.
#[test]
fn plain_end_moves_the_caret_and_leaves_the_viewport_alone() {
    let (mut chat, mut screen, mut term) = scrolled_fullscreen_with_draft();
    // Put the caret at the start so "moved to the end" is observable.
    chat.editor.cursor_line_start();
    let before_scroll = screen.scroll();
    assert!(before_scroll > 0, "the fixture must be scrolled away");

    press("\x1b[F", &mut chat, &mut screen, &mut term);

    let (_, col) = caret_cells(&chat).expect("the caret is painted");
    assert_eq!(
        col,
        "half-typed prompt".len(),
        "End moves the caret to the end of the line"
    );
    assert_eq!(
        screen.scroll(),
        before_scroll,
        "End must not jump the viewport to the live bottom"
    );
}

// Verifies: gh #375 gap 4 - `Ctrl+Home` scrolls the viewport to the transcript
// top and leaves the caret where it was. pi's split, adopted rather than
// re-invented, which is what removes the ambiguity by construction.
#[test]
fn ctrl_home_scrolls_to_the_top_and_leaves_the_caret_alone() {
    let (mut chat, mut screen, mut term) = scrolled_fullscreen_with_draft();
    let before_caret = caret_cells(&chat).expect("the caret is painted");

    press("\x1b[1;5H", &mut chat, &mut screen, &mut term);

    assert_eq!(
        caret_cells(&chat),
        Some(before_caret),
        "Ctrl+Home must not move the caret"
    );
    let extent = chat.scroll_extent(80, 24);
    let max = extent.0.saturating_sub(extent.1);
    assert_eq!(
        screen.scroll() as usize,
        max,
        "Ctrl+Home scrolls the viewport to the top of the transcript"
    );
}

// Verifies: gh #375 gap 4 - `Ctrl+End` returns the viewport to the live bottom
// and leaves the caret where it was.
#[test]
fn ctrl_end_scrolls_to_the_bottom_and_leaves_the_caret_alone() {
    let (mut chat, mut screen, mut term) = scrolled_fullscreen_with_draft();
    let before_caret = caret_cells(&chat).expect("the caret is painted");

    press("\x1b[1;5F", &mut chat, &mut screen, &mut term);

    assert_eq!(
        caret_cells(&chat),
        Some(before_caret),
        "Ctrl+End must not move the caret"
    );
    assert_eq!(screen.scroll(), 0, "Ctrl+End returns to the live bottom");
}

// Verifies: gh #375 gap 4 - an empty prompt's `Home`/`End` are still sensible:
// they leave the caret at column 0 (there is nothing to move across) and still
// do not scroll. Pinned because "the editor owns the key" has to be true when
// the editor is empty too, or the key becomes a dead press.
#[test]
fn home_and_end_on_an_empty_prompt_do_not_scroll() {
    let (mut chat, mut screen, mut term) = scrolled_fullscreen_with_draft();
    chat.editor.set_text("");
    let before_scroll = screen.scroll();

    press("\x1b[H", &mut chat, &mut screen, &mut term);
    assert_eq!(screen.scroll(), before_scroll, "empty Home does not scroll");
    assert_eq!(
        caret_cells(&chat).map(|(_, col)| col),
        Some(0),
        "empty Home leaves the caret at column 0"
    );

    press("\x1b[F", &mut chat, &mut screen, &mut term);
    assert_eq!(screen.scroll(), before_scroll, "empty End does not scroll");
    assert_eq!(
        caret_cells(&chat).map(|(_, col)| col),
        Some(0),
        "empty End leaves the caret at column 0"
    );
}

// Verifies: gh #375 gap 4 - main-screen mode is unchanged: `Home`/`End` are the
// editor's there too, and there is no viewport to scroll.
#[test]
fn home_and_end_in_main_screen_mode_stay_with_the_editor() {
    let mut chat = chat();
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    chat.editor.set_text("draft");
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    let before = caret_cells(&chat).expect("the caret is painted");

    press("\x1b[H", &mut chat, &mut screen, &mut term);
    assert_eq!(
        caret_cells(&chat).map(|(_, col)| col),
        Some(0),
        "Home moves the caret in main-screen mode"
    );
    assert_eq!(screen.scroll(), 0, "and there is no viewport to scroll");
    assert!(before.1 > 0, "the fixture started with the caret away");
}

// Verifies: gh #375 gap 4 - a user who already remapped these keys keeps their
// mapping. Only the **defaults** changed, so a binding in `keybindings.toml`
// still wins: here a user puts the viewport's top on plain `Home` back, and the
// editor no longer claims it.
#[test]
fn a_user_remap_of_home_still_wins_over_the_new_defaults() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    use std::collections::BTreeMap;

    // The user restores the pre-#375 arrangement: viewport top on plain Home,
    // editor line-start on ctrl+a only.
    let mut binds: BTreeMap<String, Vec<String>> = BTreeMap::new();
    binds.insert("tui.altScreen.top".to_string(), vec!["home".to_string()]);
    binds.insert(
        "tui.editor.cursorLineStart".to_string(),
        vec!["ctrl+a".to_string()],
    );
    let manager = std::sync::Arc::new(KeybindingsManager::with_user_bindings(binds));

    let mut chat = Chat::new(options(), manager.clone());
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    chat.editor.set_text("draft");

    assert_eq!(
        chat.viewport_scroll("\x1b[H"),
        Some(crate::chat_keys::ViewportScroll::Top),
        "the user's Home still scrolls the viewport"
    );

    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    screen.set_scroll(12);
    let before_caret = caret_cells(&chat).expect("the caret is painted");
    press("\x1b[H", &mut chat, &mut screen, &mut term);
    assert_eq!(
        caret_cells(&chat),
        Some(before_caret),
        "and the editor does not also take it"
    );
    assert_ne!(
        screen.scroll(),
        12,
        "the viewport moved, per the user's map"
    );
}

// Verifies: gh #375 gap 4 - the startup double-claim warning stays silent for
// these actions with the shipped defaults. The loader only reports user-vs-user
// conflicts (a default may legitimately be contextual), so the check is that the
// *default table* leaves no key claimed twice between the editor and the
// viewport - the property the brief asked for, tested where it is decidable.
#[test]
fn no_default_key_is_claimed_by_both_the_editor_and_the_viewport() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let manager = KeybindingsManager::new();
    for action in [
        "tui.editor.cursorLineStart",
        "tui.editor.cursorLineEnd",
        "tui.altScreen.top",
        "tui.altScreen.bottom",
    ] {
        assert!(
            !manager.keys(action).is_empty(),
            "{action} must keep at least one default key"
        );
    }
    let editor: Vec<String> = manager
        .keys("tui.editor.cursorLineStart")
        .iter()
        .chain(manager.keys("tui.editor.cursorLineEnd").iter())
        .cloned()
        .collect();
    let viewport: Vec<String> = manager
        .keys("tui.altScreen.top")
        .iter()
        .chain(manager.keys("tui.altScreen.bottom").iter())
        .cloned()
        .collect();
    for key in &editor {
        assert!(
            !viewport.contains(key),
            "{key:?} is claimed by both the editor ({editor:?}) and the \
             viewport ({viewport:?})"
        );
    }
    assert!(
        manager.conflicts().is_empty(),
        "the shipped defaults must raise no double-claim warning: {:?}",
        manager.conflicts()
    );
}

// Verifies: gh #375 gap 5 - `tui.altScreen.pageUp`/`pageDown` were declared in the
// binding table and never acted on: no code matched them, so the keys were dead.
// A page now moves the viewport by the window less pi's 4-row overlap, and the
// caret is untouched.
#[test]
fn page_up_and_down_scroll_the_viewport_by_a_page() {
    let (mut chat, mut screen, mut term) = scrolled_fullscreen_with_draft();
    chat.editor.set_text("");
    // Start at the live bottom so both directions have room.
    screen.set_scroll(0);
    let before_caret = caret_cells(&chat).expect("the caret is painted");
    let (_, window) = chat.scroll_extent(80, 24);

    press("\x1b[5~", &mut chat, &mut screen, &mut term); // PageUp
    assert_eq!(
        screen.scroll(),
        crate::chat_keys::page_scroll_rows(window),
        "PageUp moves one page towards the top"
    );
    assert_eq!(
        caret_cells(&chat),
        Some(before_caret),
        "paging must not move the caret"
    );

    press("\x1b[6~", &mut chat, &mut screen, &mut term); // PageDown
    assert_eq!(
        screen.scroll(),
        0,
        "PageDown returns towards the live bottom by the same page"
    );
    assert_eq!(
        caret_cells(&chat),
        Some(before_caret),
        "...and still leaves the caret alone"
    );
}

// Verifies: gh #375 gap 5 - a page scroll stops at the transcript top rather than
// running past it, so repeated PageUp presses leave a valid frame.
#[test]
fn page_up_clamps_at_the_transcript_top() {
    let (mut chat, mut screen, mut term) = scrolled_fullscreen_with_draft();
    chat.editor.set_text("");
    for _ in 0..30 {
        press("\x1b[5~", &mut chat, &mut screen, &mut term);
    }
    let (content, window) = chat.scroll_extent(80, 24);
    assert_eq!(
        screen.scroll() as usize,
        content.saturating_sub(window),
        "paging up stops at the top instead of overshooting"
    );
}

// Verifies: gh #375 gap 5 + pi's arithmetic - a page is the whole window less the
// overlap, never less than one row. Pinned directly so the constant cannot drift
// from pi's `Math.max(1, viewportHeight - PAGE_SCROLL_OVERLAP)`.
#[test]
fn a_page_is_the_window_less_the_overlap_floored_at_one() {
    assert_eq!(
        crate::chat_keys::page_scroll_rows(24),
        (24 - crate::chat_keys::PAGE_SCROLL_OVERLAP) as u16
    );
    assert_eq!(
        crate::chat_keys::page_scroll_rows(4),
        1,
        "floored at one row"
    );
    assert_eq!(crate::chat_keys::page_scroll_rows(0), 1, "and never zero");
}

// Verifies: gh #375 gap 4 - `/hotkeys` shows the new truth: plain `Home`/`End`
// describe the editor, and the viewport's top/bottom are `Ctrl+Home`/`Ctrl+End`.
// The table is generated from the live manager, so this row pins that the split
// actually reaches the reader rather than only the matcher.
#[test]
fn hotkeys_lists_the_split_bindings() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let text = crate::chat_notices::hotkeys_notice(&KeybindingsManager::new());

    let line_for = |needle: &str| {
        text.lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("/hotkeys must list {needle:?}:\n{text}"))
            .to_string()
    };

    // The table prints the manager's own key ids (lower case), so compare in
    // those terms rather than in the display casing.
    let line_start = line_for("Move to line start");
    assert!(
        line_start.contains("home") && !line_start.contains("ctrl+home"),
        "plain home belongs to the editor and ctrl+home must not appear on its row: {line_start:?}"
    );

    let line_end = line_for("Move to line end");
    assert!(
        line_end.contains("end") && !line_end.contains("ctrl+end"),
        "plain end belongs to the editor: {line_end:?}"
    );

    let top = line_for("Scroll viewport to top");
    assert!(
        top.contains("ctrl+home"),
        "the viewport's top is ctrl+home now: {top:?}"
    );

    let bottom = line_for("Scroll viewport to bottom");
    assert!(
        bottom.contains("ctrl+end"),
        "the viewport's bottom is ctrl+end now: {bottom:?}"
    );
}
