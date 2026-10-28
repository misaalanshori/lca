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
            // Gh #374 gap 4: three forms with two dwells. A verified copy is a
            // success and fades quickly; an unverified one and a failure both take
            // the longer window, because both carry something the reader has to
            // act on. The unverified text no longer opens with "copied" - that was
            // the lie this gap fixes.
            if report.is_error() {
                chat.toast_error(report.notice());
            } else {
                chat.toast_success(report.notice());
            }
        }
    }
}

/// Paste the clipboard into the composer, for a right-button press (gh #374 gap
/// 4).
///
/// Three outcomes, and every one of them says something - silence on a clipboard
/// read that failed is exactly the defect class this gap is about:
///
/// - text found: it goes into the editor through the same path `/paste` and
///   bracketed paste use, so a multi-line value is handled the one way LCA handles
///   pasted text;
/// - a read that found nothing: [`MISSING_CLIPBOARD_HINT`];
/// - no reader at all: the same toast, because an embedder without a clipboard is
///   indistinguishable to the user from one whose clipboard is empty.
///
/// No second clipboard mechanism: the reader is `UiHooks::read_clipboard`, which
/// the host fills from the bounded `arboard` path gh #406 built.
pub(crate) fn paste_from_clipboard(chat: &mut Chat) {
    let Some(read) = chat.world.options.hooks.read_clipboard.clone() else {
        chat.toast_error(MISSING_CLIPBOARD_HINT);
        return;
    };
    match read() {
        Some(text) if !text.is_empty() => {
            chat.editor.insert_str(&crate::state::sanitize_block(&text));
            chat.toast_success(PASTED_FROM_CLIPBOARD);
        }
        _ => chat.toast_error(MISSING_CLIPBOARD_HINT),
    }
}

/// What a right-button paste says when there was nothing to paste.
///
/// One sentence covers "no display", "empty clipboard" and "no reader": all three
/// mean the same thing to the user, and naming the difference would leak a
/// platform detail into a message they act on identically.
const MISSING_CLIPBOARD_HINT: &str =
    "\u{2717} nothing to paste: the clipboard is empty or unreadable";

/// What a successful right-button paste says (gh #374 gap 4).
const PASTED_FROM_CLIPBOARD: &str = "\u{2713} pasted from the clipboard";
