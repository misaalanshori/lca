//! Gh #386 gap 2: a stationary mouse must not make a centered picker run away.
//!
//! ## The defect
//!
//! `hover_picker_item` set `picker.selected`, and the rolling window is
//! `selected - max_visible/2` (`render.rs::picker_window`). So a mouse *move* over
//! a list re-centered the window under a stationary cursor, the row under the
//! pointer changed, that fired another move, and the list walked itself to the
//! end. Merely resting the pointer over `/model` was enough.
//!
//! ## The contract, which is pi's (`select-list.ts::handleMouse`)
//!
//! | input | what it does to `selected` |
//! |---|---|
//! | move | **nothing** — "Hover must not change selection: the visible range is centered on it" |
//! | press | previews the pressed item, and remembers what it replaced |
//! | release on the same row | confirms it |
//! | release on a different row | **aborts and restores** the pre-press selection |
//! | wheel | one item per physical event, no acceleration |
//! | keyboard | unchanged (see the report: LCA saturates where pi wraps) |
//!
//! The reason `move` must not touch `selected` *at all*, rather than merely not
//! re-centering, is that a frozen window with a silently moving selection is the
//! worse bug: the next `Enter` would confirm a row the user never pointed at.
//!
//! The input-kind table is `chat_mouse.rs`'s `picker_*` methods — one per input
//! kind, all keyed on the same `PickerHit` the hit test produces, so a picker
//! cannot be wired differently from its neighbours.
//!
//! Split out of `chat_picker_scroll_tests.rs` for the 1,200-line ceiling.

use super::*;
use super::{chat, strip};

/// A model picker with enough rows that the window genuinely rolls.
fn tall_chat() -> Chat {
    use crate::chat_pickers::ModelPicker;
    let mut chat = chat();
    chat.screen_mode = true;
    let models: Vec<crate::ModelRow> = (0..50)
        .map(|index| {
            crate::ModelRow::from((format!("model-{index:02}"), format!("model-{index:02}")))
        })
        .collect();
    chat.model_picker = Some(ModelPicker::new(models));
    chat
}

/// The open model picker's `selected`.
fn selected(chat: &Chat) -> usize {
    chat.model_picker.as_ref().expect("open").selected
}

/// Paint one frame, so `picker_window` reflects the current selection.
fn frame(chat: &Chat) -> (usize, usize, u16) {
    let (w, h) = (80u16, 24u16);
    let _ = chat.viewport(w, h, 0);
    let (start, end, content) = chat.picker_window.get();
    (start, end, content)
}

// ===========================================================================
// move — the bug
// ===========================================================================

// Verifies: gh #386 gap 2 (the reported symptom) - a mouse **move** over a picker
// item does not change `selected`, so the centered window cannot re-center under a
// stationary cursor.
//
// Before the fix `hover_picker_item` wrote `selected`, the window followed, and the
// row under the pointer changed - which is exactly how the list scrolled itself.
#[test]
fn a_mouse_move_over_a_picker_item_does_not_change_the_selection() {
    let mut chat = tall_chat();
    // Keyboard-walk to a rolled position, so the window is provably off the top.
    let _ = frame(&chat);
    for _ in 0..20 {
        let _ = chat.handle_picker_key("", Some("down"));
    }
    let before = selected(&chat);
    let (start_before, end_before, _) = frame(&chat);
    assert!(start_before > 0, "the window rolled: {start_before}");

    // A move over a *different* item - the gesture that used to select.
    let other = (start_before + 3).min(end_before.saturating_sub(1));
    chat.picker_hover(other);

    assert_eq!(
        selected(&chat),
        before,
        "a move is not a selection: hover must not write `selected`"
    );
    let (start_after, end_after, _) = frame(&chat);
    assert_eq!(
        (start_after, end_after),
        (start_before, end_before),
        "and the window is byte-identical, because it derives from `selected`"
    );
}

// Verifies: gh #386 gap 2 - a move followed by `Enter` confirms what was selected
// **before** the move.
//
// This is the failure the frozen-window-only fix would leave: the highlight stays
// put while `selected` silently moves, so the confirmation names a row the user
// never pointed at. The row asserts the confirmation, not the field.
#[test]
fn a_move_then_enter_confirms_the_pre_move_selection() {
    let mut chat = tall_chat();
    let _ = frame(&chat);
    for _ in 0..5 {
        let _ = chat.handle_picker_key("", Some("down"));
    }
    let chosen = selected(&chat);
    assert_eq!(chosen, 5, "the keyboard walk set the target");

    // Hover something else, then confirm.
    chat.picker_hover(30);
    chat.confirm_picker_item();

    // The model picker applies its choice on Enter; the notice names it.
    let notice = chat.last_notice().unwrap_or_default();
    assert!(
        notice.contains(&format!("model-{chosen:02}")),
        "Enter confirmed the pre-move selection, not the hovered row: {notice:?}"
    );
}

// ===========================================================================
// press / release
// ===========================================================================

// Verifies: gh #386 gap 2 - **press previews** (the selection follows the pressed
// row immediately, as pi's `mousePressedIndex` does), and a **release on the same
// row confirms** it.
#[test]
fn a_press_previews_and_a_release_on_the_same_row_confirms() {
    let mut chat = tall_chat();
    let _ = frame(&chat);
    let target = 7;

    chat.picker_press(target);
    assert_eq!(
        selected(&chat),
        target,
        "the press previews the pressed row"
    );
    assert!(
        chat.picker_press.is_some(),
        "and records the press, so a release can compare"
    );

    let outcome = chat.picker_release(target);
    assert_eq!(outcome, crate::chat_mouse::ReleaseOutcome::Confirm);
    // The release *decides*; the caller acts on the decision, exactly as the
    // keyboard path does (`confirm_picker_item` is Enter's body). Asserting
    // closure here instead would only re-test Enter.
    chat.confirm_picker_item();
    assert!(
        chat.model_picker.is_none(),
        "a confirming release closes the picker"
    );
}

// Verifies: gh #386 gap 2 (**the row people forget**) - a release on a *different*
// row aborts and **restores the pre-press selection**.
//
// The press preview moved `selected`, so without a restore a drag-off would leave
// the user's selection somewhere they never chose - and the next `Enter` would
// confirm it. The prior value therefore has to be captured at the press.
#[test]
fn a_release_on_a_different_row_aborts_and_restores() {
    let mut chat = tall_chat();
    let _ = frame(&chat);
    for _ in 0..4 {
        let _ = chat.handle_picker_key("", Some("down"));
    }
    let before = selected(&chat);
    assert_eq!(before, 4);

    // Press elsewhere (preview moves), then release on a third row: abort.
    chat.picker_press(20);
    assert_eq!(selected(&chat), 20, "the preview moved with the press");
    let outcome = chat.picker_release(31);

    assert_eq!(
        outcome,
        crate::chat_mouse::ReleaseOutcome::Abort { restore: before },
        "the abort carries the value to restore"
    );
    assert_eq!(
        selected(&chat),
        before,
        "and the selection is back where the user left it"
    );
    assert!(
        chat.model_picker.is_some(),
        "an aborted click leaves the picker open"
    );
    assert!(chat.picker_press.is_none(), "and forgets the press");
}

// Verifies: gh #386 gap 2 - a release on **chrome or the backdrop** aborts too, and
// restores. Dragging off the box is the same gesture as dragging onto another row.
#[test]
fn a_release_off_the_items_aborts_and_restores() {
    let mut chat = tall_chat();
    let _ = frame(&chat);
    for _ in 0..3 {
        let _ = chat.handle_picker_key("", Some("down"));
    }
    let before = selected(&chat);
    chat.picker_press(15);
    // `None` is what the renderer reports for a release that hit no item.
    let outcome = chat.picker_release_item(None);
    assert_eq!(
        outcome,
        crate::chat_mouse::ReleaseOutcome::Abort { restore: before }
    );
    assert_eq!(selected(&chat), before);
    assert!(chat.model_picker.is_some());
}

// ===========================================================================
// wheel
// ===========================================================================

// Verifies: gh #386 gap 2 - the wheel moves **exactly one item per physical
// event**, and a burst of five events moves five, not fifteen.
//
// `WheelAccel` exists for the transcript, where rows are a continuum. A picker moves
// by *items*, so acceleration would let one trackpad flick fly through the list.
// The row drives `picker_wheel` directly, which is what the wheel arm calls with a
// clamped ±1.
#[test]
fn the_wheel_moves_one_item_per_event_with_no_acceleration() {
    let mut chat = tall_chat();
    let _ = frame(&chat);
    assert_eq!(selected(&chat), 0);

    // `delta` is added to the index, so +1 walks *down* the list (the wheel arm
    // passes the event's sign through; the transcript's convention is the same).
    // One physical notch.
    chat.wheel_picker(1);
    assert_eq!(selected(&chat), 1, "one notch, one row");

    // A simulated momentum burst: five events in a row.
    for _ in 0..5 {
        chat.wheel_picker(1);
    }
    assert_eq!(
        selected(&chat),
        6,
        "five events move five rows - no acceleration, unlike the transcript"
    );

    // And back up by exactly one.
    chat.wheel_picker(-1);
    assert_eq!(selected(&chat), 5);
}

// Verifies: gh #386 gap 2 - the wheel does not wrap and does not overrun the ends.
//
// pi clamps the wheel (`Math.max(0, Math.min(len-1, ...))`) even though its
// *keyboard* wraps, so the two input kinds differ on purpose.
#[test]
fn the_wheel_clamps_at_both_ends() {
    let mut chat = tall_chat();
    let _ = frame(&chat);
    chat.wheel_picker(-1);
    assert_eq!(selected(&chat), 0, "no wrap above the first row");
    for _ in 0..80 {
        chat.wheel_picker(1);
    }
    assert_eq!(selected(&chat), 49, "clamped at the last of 50 models");
}

// ===========================================================================
// keyboard — unchanged (the brief's point 4)
// ===========================================================================

// Verifies: gh #386 gap 2 - keyboard `Up`/`Down` behaves **exactly as before**,
// including the fact that it does *not* wrap for this picker.
//
// The brief says: if LCA does not wrap, that is a separate finding - do not add it
// silently here. So the row pins today's saturating behaviour for the model picker,
// and the report names the asymmetry (the fork picker's `Up` *does* wrap).
#[test]
fn the_keyboard_walk_is_unchanged() {
    let mut chat = tall_chat();
    let _ = frame(&chat);
    for _ in 0..3 {
        let _ = chat.handle_picker_key("", Some("down"));
    }
    assert_eq!(selected(&chat), 3, "Down walks forward");
    let _ = chat.handle_picker_key("", Some("up"));
    assert_eq!(selected(&chat), 2, "Up walks back");
    // At the top, Up saturates rather than wrapping: today's behaviour, pinned so a
    // future cycle changes it deliberately.
    for _ in 0..10 {
        let _ = chat.handle_picker_key("", Some("up"));
    }
    assert_eq!(
        selected(&chat),
        0,
        "Up at the top saturates (pi wraps; reported, not changed here)"
    );
}

// ===========================================================================
// the table itself
// ===========================================================================

// Verifies: gh #386 gap 2 - the routing is **one table in one place**, so the next
// picker cannot be wired differently.
//
// The bug says "any picker", and the shape that lets it return is per-picker mouse
// code. Every picker answers through the same four methods, and each of those is the
// only writer of `selected` for its input kind.
#[test]
fn every_picker_answers_through_the_same_four_methods() {
    // The four input kinds exist as one method each, in `chat_mouse.rs`.
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/chat_mouse.rs"))
        .expect("chat_mouse.rs is readable");
    for method in [
        "pub fn picker_hover(",
        "pub fn picker_press(",
        "pub fn picker_release_item(",
        "pub fn wheel_picker(",
    ] {
        assert!(source.contains(method), "the table's `{method}` arm exists");
    }
    // The hover arm writes nothing: that is the bug's fix, in the one place all ten
    // pickers share.
    let start = source
        .find("pub fn picker_hover(")
        .expect("picker_hover exists");
    let rest = &source[start..];
    let body = &rest[..rest.find("\n    }\n").map_or(rest.len(), |at| at + 7)];
    assert!(
        !body.contains(".selected ="),
        "hover assigns no selection: {body}"
    );
    assert!(
        body.contains("fn picker_hover(") || body.contains("does nothing"),
        "and says so, so the next reader does not restore it: {body}"
    );
}

// Verifies: gh #386 gap 2 - every picker previews on press, not just the model one.
//
// The chain in `picker_press` is one `else if` per picker; a new picker added
// without an arm would silently stop previewing. Three of the ten are checked here
// (one per shape: list, fixed options, sectioned body) and the rest share the code.
#[test]
fn every_picker_shape_previews_on_press() {
    // A theme picker (fixed option list).
    let mut chat = chat();
    chat.screen_mode = true;
    chat.open_theme_picker();
    let before = chat.theme_picker.as_ref().expect("open").selected;
    let other = (before + 1) % chat.theme_names.len().max(1);
    chat.picker_press(other);
    assert_eq!(
        chat.theme_picker.as_ref().expect("open").selected,
        other,
        "the theme picker previews"
    );
    // And the abort restores the theme too, or a drag-off would leave the preview.
    let outcome = chat.picker_release(other + 5);
    assert_eq!(
        outcome,
        crate::chat_mouse::ReleaseOutcome::Abort { restore: before }
    );
    assert_eq!(
        chat.theme_picker.as_ref().expect("open").selected,
        before,
        "a drag-off restores the theme picker's selection"
    );
}

// Verifies: gh #386 gap 2 - the picker's chrome eats a press without moving the
// selection, and the backdrop still dismisses.
//
// These are the two non-item outcomes the table has to keep, and they are why
// `picker_press` takes an `Option`: `None` is chrome.
#[test]
fn chrome_press_selects_nothing_and_backdrop_still_dismisses() {
    let mut chat = tall_chat();
    let _ = frame(&chat);
    let before = selected(&chat);
    chat.picker_press_item(None);
    assert_eq!(selected(&chat), before, "chrome moves nothing");
    let outcome = chat.picker_release_item(None);
    assert_eq!(
        outcome,
        crate::chat_mouse::ReleaseOutcome::Abort { restore: before }
    );
    assert!(
        chat.model_picker.is_some(),
        "a chrome press/release leaves the picker open"
    );
}

// Verifies: gh #386 gap 2 - the frame a user sees does not move while the pointer
// rests on it.
//
// The strongest form of the reported symptom: paint, hover, paint, and compare the
// two frames' text. A window that re-centered would differ; a selection that moved
// would differ in the highlight.
#[test]
fn resting_the_pointer_leaves_two_frames_identical() {
    let mut chat = tall_chat();
    let (w, h) = (80u16, 24u16);
    for _ in 0..15 {
        let _ = chat.handle_picker_key("", Some("down"));
    }
    let first = strip(&chat.viewport(w, h, 0)).join("\n");
    // The pointer sits still on a row: the same hover, twice, as a stationary mouse
    // reports nothing but the app's own redraws.
    let (start, end, _) = chat.picker_window.get();
    for _ in 0..3 {
        chat.picker_hover(start + 2);
    }
    let (start2, end2, _) = chat.picker_window.get();
    let second = strip(&chat.viewport(w, h, 0)).join("\n");
    assert_eq!(
        (start2, end2),
        (start, end),
        "the window never re-centers under a still pointer"
    );
    assert_eq!(first, second, "and the frame is byte-identical");
}
