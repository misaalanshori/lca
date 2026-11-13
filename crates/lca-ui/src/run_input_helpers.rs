//! Input-path helpers split out of `run.rs` for gate 11's 1,200-line ceiling
//! (gh #370 flaw 1 pushed the loop over). Behaviour unchanged: these are the tail
//! of `handle_input`'s mouse arm, moved whole.

// One iteration, many events (gh #370 flaw 2).
//
// # The flaw
//
// The loop popped **one** input event per iteration and then did a full
// `chat.viewport()` + `screen.render()` pass. A fullscreen mouse drag emits dozens of
// SGR `Move` sequences per second, so every cell of the drag cost a complete repaint -
// and the same was true of a fast typist and of a large paste.
//
// # Why "drain everything" is not the fix
//
// A continuous producer - a drag, a fast paste, a held key's repeat - keeps its
// channel non-empty. `while let Ok(..) = try_recv()` would then never leave the drain
// and the loop would stop rendering entirely: the queue empties only as fast as it
// fills, and it fills faster than a frame can be composed. So the drain is
// **bounded**.
//
// # The bound: whatever has already arrived, and nothing that has not
//
// [`drain_input`] takes exactly what is in the channel when it is called - no clock,
// no `sleep`, no "wait a millisecond for more". That is the right bound for two
// reasons:
//
// - **It cannot add latency to the first keystroke of a burst.** A deadline or a
//   "settle window" would delay handling by construction, which is exactly the
//   regression the brief warns about: flaw 1's receipt was "median echo 20.1 -> 20.3
//   ms", and a coalescing change is where an extra millisecond hides.
// - **A continuous producer cannot starve the frame**, because the drain never waits
//   for more: it renders with what it has, and the next iteration drains again. At
//   worst a frame is painted with one event's worth of work still pending.
//
// `MAX_DRAINED` then caps the *work* per iteration, so a producer that filled a
// thousand events during one slow paint cannot make the next iteration's drain itself
// unbounded. Whatever is past the cap stays queued, and the wake that put it there is
// still pending - so the loop comes straight back for it rather than sleeping.
//
// # Keystrokes are never dropped
//
// Every drained event is **handled**, in arrival order. Nothing is coalesced away: a
// mouse burst is not collapsed to its last position, because the intermediate
// positions are not observable *to the frame* but they are observable *to the
// gesture* - a selection built by a drag is the union of the positions it passed
// through, so dropping one shortens the highlight. Collapsing was rejected rather than
// implemented: it would look like a speedup and would silently eat characters for
// keyboards, and a per-kind rule ("mouse may collapse, keys may not") is a second
// contract for the next event kind to get wrong.
//
// # What this does not change
//
// [`dirty`](crate::run) remains the single render decision, and input keeps absolute
// priority. The change is *how many events one iteration consumes*, not when the loop
// paints.
/// The most events one iteration will handle before painting (gh #370 flaw 2).
///
/// A cap on consumption rather than on arrival: it exists only so an iteration's own
/// work is bounded when a producer has filled the channel during a slow paint. 256 is
/// chosen to be **well above** any plausible single-frame burst - a 120 Hz drag emits at
/// most a couple of dozen motions in 16 ms, and a bracketed paste arrives as one event -
/// so the cap never splits normal input, while a pathology still gets a frame.
///
/// Hitting the cap is not an error and loses nothing: the remaining events stay in the
/// channel, and the next iteration drains them before the loop sleeps, because a
/// producer's wake is still pending in the wake channel.
pub const MAX_DRAINED: usize = 256;

/// Take every event that is **already** in the channel, up to [`MAX_DRAINED`].
///
/// `next` is the loop's own pop - a `try_recv` on whichever stream it is reading (its
/// own channel, or the adopted terminal's). Taking the pop as a closure is what lets
/// this be tested without a terminal and keeps the two sources one code path; the loop
/// passes the same closure it used to call once.
///
/// Returns an empty vector when nothing is pending, which is the ordinary idle case: a
/// caller must not treat "no events" as "one empty event".
///
/// The events are returned **in arrival order**, and the caller must handle every one.
/// Draining into a `Vec` rather than handling in place is deliberate: the caller has to
/// interleave `Exit`, so it owns the loop over the result - but the vector is the proof
/// that nothing was dropped, and a `Vec` of `String`s cannot be silently truncated the
/// way a "last event wins" variable can.
pub fn drain_input(next: &mut dyn FnMut() -> Option<String>) -> Vec<String> {
    let mut drained = Vec::new();
    while drained.len() < MAX_DRAINED {
        match next() {
            Some(event) => drained.push(event),
            None => break,
        }
    }
    drained
}

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
    // Gh #403 gap 3: the ladder is **files -> image -> text**, and this is the
    // seam that owns the editor, so it is where the order is decided (the CLI's
    // hook only supplies the file list). A file list wins over text even when
    // both are present: what the user copied is the files.
    let files = chat
        .world
        .options
        .hooks
        .read_clipboard_files
        .clone()
        .map(|read_files| read_files());
    match crate::state_paste::pick_tier(files, false, None) {
        crate::state_paste::PasteTier::Files(text) if text.starts_with("paste refused:") => {
            // A refusal is its own answer: show it, insert nothing, and do not
            // consult the text tier underneath it.
            chat.toast_error(&text);
            return;
        }
        crate::state_paste::PasteTier::Files(text) => {
            chat.editor.insert_str(&crate::state::sanitize_block(&text));
            chat.toast_success(PASTED_FROM_CLIPBOARD);
            return;
        }
        _ => {}
    }
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
