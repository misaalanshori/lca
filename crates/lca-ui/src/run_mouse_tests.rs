//! Gh #386 gap 2 through the **real pointer dispatch**: `handle_picker_mouse`, the
//! function the loop calls for every normalized mouse event.
//!
//! The `Chat`-level rows (`chat_picker_hover_tests.rs`) prove the input-kind table.
//! These prove the dispatch *uses* it: `run_mouse.rs` had no test at all before this
//! cycle, so a move that reached `select_picker_item` again — or a release that
//! never consulted `picker_release_item` — would pass every other row in the suite.
//! That is the wiring half of the bug, and it is where the self-scroll lived.

use super::*;
use crate::run::switch_screen;
use crate::run_mouse::handle_picker_mouse;

/// A fullscreen chat with a 50-model picker open, plus the terminal the dispatch
/// needs for the selection-clear calls.
fn picker_chat() -> (Chat, Screen, lca_tui::engine::terminal::FakeTerminal) {
    use crate::chat_pickers::ModelPicker;
    let mut term = lca_tui::engine::terminal::FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(lca_tui::engine::main_screen::MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    let mut chat = crate::run::tests::chat();
    chat.screen_mode = true;
    let models: Vec<crate::ModelRow> = (0..50)
        .map(|index| {
            crate::ModelRow::from((format!("model-{index:02}"), format!("model-{index:02}")))
        })
        .collect();
    chat.model_picker = Some(ModelPicker::new(models));
    let (w, h) = (80u16, 24u16);
    let _ = chat.viewport(w, h, 0);
    (chat, screen, term)
}

/// The 0-based column and row of an item, read from the painted box.
fn item_cell(chat: &Chat, item: usize) -> (u16, u16) {
    let (w, h) = (80u16, 24u16);
    let (rect, map) = chat.picker_layout(w, h).expect("the box maps");
    let offset = map
        .iter()
        .position(|entry| *entry == Some(item))
        .unwrap_or_else(|| panic!("item {item} is painted"));
    (rect.col + 4, rect.row + 1 + offset as u16)
}

/// The cell of a row that is chrome (a header, not an item).
fn chrome_cell(chat: &Chat) -> (u16, u16) {
    let (w, h) = (80u16, 24u16);
    let (rect, _) = chat.picker_layout(w, h).expect("the box maps");
    (rect.col + 4, rect.row + 1)
}

fn move_to(chat: &mut Chat, screen: &mut Screen, col: u16, row: u16) {
    handle_picker_mouse(
        lca_tui::engine::core::MouseEvent::Move {
            col,
            row,
            button: None,
            modifiers: lca_tui::engine::core::Modifiers {
                shift: false,
                alt: false,
                ctrl: false,
            },
        },
        chat,
        screen,
    );
}

fn down_at(chat: &mut Chat, screen: &mut Screen, col: u16, row: u16) {
    handle_picker_mouse(
        lca_tui::engine::core::MouseEvent::Down {
            col,
            row,
            button: lca_tui::engine::core::MouseButton::Left,
            modifiers: lca_tui::engine::core::Modifiers {
                shift: false,
                alt: false,
                ctrl: false,
            },
        },
        chat,
        screen,
    );
}

fn up_at(chat: &mut Chat, screen: &mut Screen, col: u16, row: u16) {
    handle_picker_mouse(
        lca_tui::engine::core::MouseEvent::Up {
            col,
            row,
            button: lca_tui::engine::core::MouseButton::Left,
            modifiers: lca_tui::engine::core::Modifiers {
                shift: false,
                alt: false,
                ctrl: false,
            },
        },
        chat,
        screen,
    );
}

/// The open picker's `selected`.
fn selected(chat: &Chat) -> usize {
    chat.model_picker.as_ref().expect("open").selected
}

// Verifies: gh #386 gap 2 (the reported symptom, through the real dispatch) - a
// mouse **move** over a picker row does not change the selection.
#[test]
fn a_move_through_the_dispatch_does_not_change_the_selection() {
    let (mut chat, mut screen, _term) = picker_chat();
    for _ in 0..20 {
        let _ = chat.handle_picker_key("", Some("down"));
    }
    let before = selected(&chat);
    let (col, row) = item_cell(&chat, 2);
    move_to(&mut chat, &mut screen, col, row);
    assert_eq!(
        selected(&chat),
        before,
        "the dispatch's move arm must not select"
    );
}

// Verifies: gh #386 gap 2 - a **press** through the dispatch previews, and a
// **release on the same row** confirms (the picker closes).
#[test]
fn a_press_and_same_row_release_through_the_dispatch_confirms() {
    let (mut chat, mut screen, _term) = picker_chat();
    let target = 6;
    let (col, row) = item_cell(&chat, target);
    down_at(&mut chat, &mut screen, col, row);
    assert_eq!(selected(&chat), target, "the press previews");
    assert!(chat.picker_press.is_some(), "and owns the gesture");
    up_at(&mut chat, &mut screen, col, row);
    assert!(
        chat.model_picker.is_none(),
        "a same-row release confirms and closes"
    );
}

// Verifies: gh #386 gap 2 (**the row people forget**, through the dispatch) - a
// release on a different row aborts and restores the pre-press selection.
#[test]
fn a_release_on_another_row_through_the_dispatch_restores() {
    let (mut chat, mut screen, _term) = picker_chat();
    for _ in 0..3 {
        let _ = chat.handle_picker_key("", Some("down"));
    }
    let before = selected(&chat);
    let pressed = before + 2;
    let other = before + 4;
    let (col, down_row) = item_cell(&chat, pressed);
    down_at(&mut chat, &mut screen, col, down_row);
    assert_eq!(selected(&chat), pressed, "the press previewed");

    let (col2, up_row) = item_cell(&chat, other);
    up_at(&mut chat, &mut screen, col2, up_row);
    assert_eq!(
        selected(&chat),
        before,
        "a drag-off restores the selection the press replaced"
    );
    assert!(
        chat.model_picker.is_some(),
        "and leaves the picker open, because nothing was confirmed"
    );
}

// Verifies: gh #386 gap 2 - a press on **chrome** previews nothing, and a release
// there aborts. The gesture is still consumed: chrome eats it.
#[test]
fn a_chrome_press_through_the_dispatch_selects_nothing() {
    let (mut chat, mut screen, _term) = picker_chat();
    let before = selected(&chat);
    let (col, row) = chrome_cell(&chat);
    down_at(&mut chat, &mut screen, col, row);
    assert_eq!(selected(&chat), before, "chrome moves nothing");
    up_at(&mut chat, &mut screen, col, row);
    assert_eq!(selected(&chat), before);
    assert!(chat.model_picker.is_some(), "still open");
}

// Verifies: gh #386 gap 2 - the **popup** (autocomplete) is the other centered list,
// and a move over it must not select either.
//
// `select_suggestion` writes the index `popup_window` centers on, so hovering a
// popup row had the same self-scroll shape. A *click* still accepts (that is
// `chat_render.rs`'s path, a release not a hover), which the row below pins.
#[test]
fn a_move_over_the_popup_does_not_select_an_offer() {
    use lca_tui::widgets::autocomplete::{AutocompleteItem, Origin, Suggestions};
    let (mut chat, mut screen, _term) = picker_chat();
    chat.model_picker = None;
    let items: Vec<AutocompleteItem> = (0..30)
        .map(|i| AutocompleteItem {
            value: format!("value-{i}"),
            label: format!("value-{i}"),
            description: None,
            origin: Origin::Builtin,
            dim: false,
        })
        .collect();
    chat.editor.set_suggestions(Suggestions {
        items,
        prefix: "v".to_string(),
    });
    let (w, h) = (80u16, 24u16);
    let _ = chat.viewport(w, h, 0);
    let before = chat.editor.suggestion_index();
    let (top, len) = chat.popup_rect(w, h).expect("the popup paints");
    assert!(len > 2, "the popup has rows to hover");
    // Hover a row two below the top.
    move_to(&mut chat, &mut screen, 10, top + 2);
    assert_eq!(
        chat.editor.suggestion_index(),
        before,
        "a move over the popup does not move the offer selection"
    );
}
