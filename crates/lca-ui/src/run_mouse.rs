//! The pointer-routing dispatch, split out of `run.rs` for the 1,200-line
//! ceiling (gate 11).
//!
//! Two entry points, one precedence: with a picker open, items highlight and
//! confirm and the chrome eats the gesture (gh #167); with nothing open, the
//! scrollbar owns its column and everything else falls through to the
//! renderer's selection and the transcript's click paths (gh #164).
//!
//! The renderer's own contract is that a motion event which changes no visual
//! state renders nothing - only the loops in `run.rs` decide what a gesture
//! means, and these functions are the part that decides it for the pointer.

use crate::chat::Chat;
use crate::chat_mouse::{PickerHit, ReleaseOutcome, ScrollbarHit};
use crate::run::Screen;
use lca_tui::engine::core::{MouseButton, MouseEvent};

/// Route one normalized mouse event (gh #164, the keystone dispatch):
/// the scrollbar owns its column (drag scrolls, steppers keep their
/// clicks), everything else falls through to the renderer's selection
/// and the click paths below. Returns whether the gesture is consumed.
/// Wheels never land here - the wheel arm in [`handle_input`] owns them.
/// Route one normalized mouse event with a picker open (gh #167):
/// items highlight and confirm, chrome eats the gesture, the backdrop
/// dismisses and falls through (the picker is gone, so the transcript
/// is clickable again). Returns whether the gesture is consumed.
pub(crate) fn handle_picker_mouse(event: MouseEvent, chat: &mut Chat, screen: &mut Screen) -> bool {
    let (width, height) = chat.world.size;
    match event {
        MouseEvent::Down {
            col,
            row,
            button: MouseButton::Left,
            ..
        } => {
            // Gh #210: a press dismisses any tooltip outright.
            chat.dismiss_tooltip();
            // The composer's Up-side path (gh #165) owns editor rows:
            // the Down falls through so the click can still focus.
            if let Some((top, len)) = chat.editor_rect(width, height)
                && row >= top
                && row < top.saturating_add(len)
            {
                return false;
            }
            match chat.picker_hit(col, row, width, height) {
                Some(PickerHit::Item(item)) => {
                    // Gh #386 gap 2: the press arm previews and remembers.
                    chat.picker_press(item);
                    screen.clear_selection();
                    true
                }
                Some(PickerHit::Chrome) => {
                    // Chrome owns the gesture (a release on it aborts) but
                    // previews nothing.
                    chat.picker_press_item(None);
                    screen.clear_selection();
                    true
                }
                _ => {
                    let _ = chat.handle_picker_key("", Some("escape"));
                    chat.picker_press = None;
                    false
                }
            }
        }
        MouseEvent::Move {
            col,
            row,
            button: None,
            ..
        } => {
            // Gh #386 gap 2: a **move** must not change the picker's selection.
            // The rolling window is `selected - max_visible/2`, so writing it here
            // re-centered the window under a stationary cursor and the list scrolled
            // itself. The table's hover arm is a no-op; calling it keeps every input
            // kind going through one place.
            if let Some(PickerHit::Item(_)) = chat.picker_hit(col, row, width, height) {
                chat.picker_hover(0);
            }
            // Gh #210: arm the tooltip clock (picker rows are unlabeled,
            // so this only ever rearms over plain cells).
            chat.note_hover(col, row, width, height, screen.scroll());
            // Gh #386 gap 2: the autocomplete popup is the other centered list, and
            // it had the same defect - `select_suggestion` writes the index the popup
            // window centers on. Hover no longer touches it; a click still does
            // (`chat_render.rs`'s `SuggestionAccepted` path).
            false
        }
        MouseEvent::Move {
            button: Some(_), ..
        } => chat.picker_press.is_some(),
        MouseEvent::Up { col, row, .. } => {
            if chat.picker_press.is_none() {
                return false;
            }
            // Gh #386 gap 2: the release arm. Same item confirms; anywhere else
            // aborts **and restores** the selection the press replaced.
            let hit = match chat.picker_hit(col, row, width, height) {
                Some(PickerHit::Item(item)) => Some(item),
                _ => None,
            };
            if chat.picker_release_item(hit) == ReleaseOutcome::Confirm {
                chat.confirm_picker_item();
            }
            true
        }
        _ => false,
    }
}

pub(crate) fn handle_mouse_event(event: MouseEvent, chat: &mut Chat, screen: &mut Screen) -> bool {
    if !chat.screen_mode {
        return false;
    }
    // Pi stops scrollbar hover while an overlay owns the viewport.
    if chat.picker_open() {
        chat.scrollbar_hover = false;
        screen.set_scrollbar_hover(false);
        chat.drawer_hover = false;
        return handle_picker_mouse(event, chat, screen);
    }
    if chat.world.modal_active() {
        chat.scrollbar_hover = false;
        screen.set_scrollbar_hover(false);
        chat.drawer_hover = false;
        return false;
    }
    let (width, height) = chat.world.size;
    // gh #209: the tab bar owns row 0 (its gestures never reach
    // the scrollbar or the transcript behind it).
    if chat.handle_tab_mouse(&event) {
        return true;
    }
    match event {
        MouseEvent::Down {
            col,
            row,
            button: MouseButton::Left,
            ..
        } => {
            // Gh #210: a press dismisses any tooltip outright.
            chat.dismiss_tooltip();
            if chat.scrollbar_hit(col, row, width, height, screen.scroll()) != ScrollbarHit::Track {
                return false;
            }
            // Pi's grab: on the thumb the pointer keeps its offset, off
            // it the thumb centers under the pointer.
            let grab = chat
                .scrollbar_for_frame(width, height, screen.scroll())
                .map(|geometry| {
                    if row >= geometry.thumb_top
                        && row < geometry.thumb_top.saturating_add(geometry.thumb_height)
                    {
                        row.saturating_sub(geometry.thumb_top)
                    } else {
                        geometry.thumb_height / 2
                    }
                })
                .unwrap_or(0);
            chat.scroll_drag = Some(grab);
            screen.begin_scrollbar_drag();
            true
        }
        MouseEvent::Move {
            col,
            row,
            button: None,
            ..
        } => {
            chat.refresh_scrollbar_hover(col, row, width, height, screen.scroll());
            screen.set_scrollbar_hover(chat.scrollbar_hover);
            // Gh #207: the drawer tab lifts under hover.
            chat.refresh_drawer_hover(col, row, width, height);
            // Gh #210: arm the tooltip clock.
            chat.note_hover(col, row, width, height, screen.scroll());
            false
        }
        MouseEvent::Move {
            row,
            button: Some(_),
            ..
        } => {
            let Some(grab) = chat.scroll_drag else {
                return false;
            };
            if let Some(scroll) =
                chat.scrollbar_drag_scroll(width, height, screen.scroll(), row, grab)
            {
                screen.set_scroll(scroll);
            }
            true
        }
        MouseEvent::Up { col, row, .. } => {
            chat.refresh_scrollbar_hover(col, row, width, height, screen.scroll());
            screen.set_scrollbar_hover(chat.scrollbar_hover);
            if chat.scroll_drag.take().is_some() {
                screen.end_scrollbar_drag();
                // A drag release never copies: selection never started.
                return true;
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "run_mouse_tests.rs"]
mod mouse_dispatch;
