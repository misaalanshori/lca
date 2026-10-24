//! Input-path helpers split out of `run.rs` for gate 11's 1,200-line ceiling
//! (gh #370 flaw 1 pushed the loop over). Behaviour unchanged: these are the tail
//! of `handle_input`'s mouse arm, moved whole.

use crate::chat::Chat;
use crate::copy_ladder::copy_through_ladder;
use crate::run::{Screen, link_notice};
use lca_tui::engine::terminal::Terminal;

/// Finish a mouse event that reached the screen: open a clicked link, and copy a
/// release over a selection through the clipboard ladder (issues #2 / gh #398).
///
/// Extracted from `handle_input` for gate 11's 1,200-line ceiling (gh #370 flaw 1
/// pushed the file over); behaviour unchanged.
pub(crate) fn finish_mouse_event(
    chat: &mut Chat,
    screen: &mut Screen,
    terminal: &mut dyn Terminal,
    data: &str,
) {
    // A click on an OSC-8 link opens it (R6).
    if let Some(url) = screen.take_clicked_link() {
        let outcome = chat
            .world
            .options
            .hooks
            .open_url
            .as_ref()
            .map(|open| open(&url));
        chat.toast(link_notice(&url, outcome));
    }
    // Copy on release (issue #2): prefer a verified native
    // clipboard (R6); OSC 52 is the honest fallback, and the
    // notice says which one ran.
    if data.ends_with('m') && screen.copy_on_select() {
        let text = screen.selected_text();
        if !text.is_empty() {
            let report = {
                let mut send = |chunk: &str| screen.copy_osc52(terminal, chunk);
                copy_through_ladder(
                    &text,
                    chat.world.options.hooks.copy_to_clipboard.as_ref(),
                    Some(&mut send),
                )
            };
            chat.toast(report.notice());
        }
    }
}
