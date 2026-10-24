//! Gh #372 bug 1: a toast must never wipe the scrollback.
//!
//! In scrollback mode `viewport` was handed the **entire session document**, and
//! `paint_toasts` indexed `i + 1` into it — correct for a screen slice of `height`
//! rows, wrong for a document of hundreds. So the toast spliced into **document
//! line 1**, far above the viewport, and because that index sat below
//! `prev_viewport_top` the renderer classified it as an out-of-viewport change and
//! called `full_render(true)` → `\x1b[2J\x1b[H\x1b[3J`. Screen *and* scrollback
//! were destroyed every time a toast appeared or expired, and the toast itself was
//! never visible.
//!
//! Two rows carry the harm rather than the arithmetic:
//!
//! * **no `\x1b[3J`** — scrollback destruction is the user-visible damage, so it is
//!   the guard. A coordinate fix that leaves a full render in place has fixed
//!   nothing.
//! * **the toast is visible** — the other half of the same sentence.
//!
//! Two rows were deleted during this cycle because mutation testing showed they
//! passed with the bug restored: one asserted frame height (unchanged by any
//! variant of this bug) and one checked that a toast was "not in row 0" of a
//! composed frame, which is never where the splice lands. They are named here so
//! the deletion is visible rather than silent.

use super::Chat;
use super::tests::{options, strip};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::main_screen::MainScreenRenderer;
use lca_tui::engine::terminal::FakeTerminal;
use std::sync::Arc;

/// A scrollback-mode chat with `turns` user turns, so the document is much taller
/// than the frame a small terminal shows.
fn scrollback_chat(turns: usize) -> Chat {
    let mut opts = options();
    opts.fullscreen = false;
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    let records: Vec<lca_protocol::Record> = (0..turns)
        .map(|i| lca_protocol::Record::User {
            v: 1,
            ts: i as u64,
            id: format!("u{i}"),
            parent: None,
            content: format!("turn {i}"),
            attachments: Vec::new(),
            queue: None,
        })
        .collect();
    chat.load_records(&records, None);
    chat
}

/// Drive one real frame through the main-screen renderer and return the bytes the
/// terminal received.
fn frame_bytes(
    renderer: &mut MainScreenRenderer,
    term: &mut FakeTerminal,
    chat: &Chat,
    width: u16,
    height: u16,
) -> String {
    let lines = chat.viewport(width, height, 0);
    renderer.render(term, lines, width, height);
    term.take_output()
}

const WIDTH: u16 = 60;
const HEIGHT: u16 = 12;

// Verifies: gh #372 bug 1 AC1 - a toast in scrollback mode over a document much
// taller than the terminal must not clear the screen **or the scrollback**.
//
// `\x1b[3J` is the scrollback-destroying sequence; `\x1b[2J` clears the screen.
// Both appear only in `full_render(true)`, which the renderer reaches when
// `first_changed_idx < prev_viewport_top` - the symptom of a change landing above
// the visible window.
#[test]
fn a_toast_over_a_tall_document_never_clears_the_scrollback() {
    let mut chat = scrollback_chat(40);
    let mut term = FakeTerminal::new(WIDTH, HEIGHT);
    let mut renderer = MainScreenRenderer::new();
    // First frame: paint the document and settle `prev_viewport_top`.
    frame_bytes(&mut renderer, &mut term, &chat, WIDTH, HEIGHT);

    chat.toast("settings saved");
    let out = frame_bytes(&mut renderer, &mut term, &chat, WIDTH, HEIGHT);
    assert!(
        !out.contains("\x1b[3J"),
        "a toast must not clear the scrollback: {out:?}"
    );
    assert!(
        !out.contains("\x1b[2J"),
        "a toast must not clear the screen: {out:?}"
    );

    // The expiry is the same event, so it must be clean too: the bug fires on
    // both sides of the toast's life. `expire_toasts` reads the real clock, so
    // backdate the toast rather than waiting one out.
    chat.toasts[0].until = std::time::Instant::now() - std::time::Duration::from_secs(1);
    chat.expire_toasts();
    let out = frame_bytes(&mut renderer, &mut term, &chat, WIDTH, HEIGHT);
    assert!(
        !out.contains("\x1b[3J"),
        "a toast expiring must not clear the scrollback: {out:?}"
    );
    assert!(
        !out.contains("\x1b[2J"),
        "a toast expiring must not clear the screen: {out:?}"
    );
}

// Verifies: gh #372 bug 1 AC2 - the toast is visible, at the top of the *screen*.
//
// It was painted into document row 1, hundreds of rows above the frame, so the user
// never saw the message that was supposed to explain something.
#[test]
fn a_toast_in_scrollback_mode_lands_in_the_visible_frame() {
    let mut chat = scrollback_chat(40);
    let frame: Vec<String> = strip(&chat.viewport(WIDTH, HEIGHT, 0));
    let without = frame.iter().position(|row| row.contains("settings saved"));
    assert!(without.is_none(), "no toast yet");

    chat.toast("settings saved");
    let frame: Vec<String> = strip(&chat.viewport(WIDTH, HEIGHT, 0));
    let at = frame
        .iter()
        .position(|row| row.contains("settings saved"))
        .expect("the toast must be in the frame the user sees, not at document row 1");
    // `viewport` returns the document with the frame's tail composed, so the
    // toast's absolute index is document-relative. Screen row 1 is
    // `len - HEIGHT + 1`: the top of the visible window, one row below the tab
    // bar's row 0 - exactly where #239 puts it, and where the fix must keep it.
    let screen_top = frame.len().saturating_sub(HEIGHT as usize);
    assert_eq!(
        at,
        screen_top + 1,
        "the toast floats at screen row 1 (document row {} + 1), found at {at}",
        screen_top
    );
}

// Verifies: gh #372 bug 1 AC3 - the direct statement of the offscreen splice.
//
// With the bug, `paint_toasts` wrote into `lines[1]` of the whole document - the
// second row of the session, hundreds of rows above the window - which is precisely
// the index the renderer then refused to touch differentially. This row reads the
// same buffer the renderer reads.
#[test]
fn the_renderers_document_has_no_toast_in_its_early_rows() {
    let mut chat = scrollback_chat(40);
    chat.toast("settings saved");
    let document = chat.viewport(WIDTH, HEIGHT, 0);
    let early: Vec<usize> = document
        .iter()
        .enumerate()
        .take(document.len().saturating_sub(HEIGHT as usize))
        .filter(|(_, row)| row.contains("settings saved"))
        .map(|(i, _)| i)
        .collect();
    assert!(
        early.is_empty(),
        "the toast appeared above the visible window at document rows {early:?} - \
         that is the offscreen splice that triggers full_render"
    );
}

// Verifies: gh #372 bug 1 AC4 - the mode that already worked still works.
//
// Alt-screen passes a `height`-sized frame, so its toast always landed at row 1 of
// the screen. The fix must not disturb it: the contract is "a screen slice", and
// fullscreen hands one directly.
#[test]
fn alt_screen_still_paints_its_toast_at_screen_row_one() {
    let mut opts = options();
    opts.fullscreen = true;
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.toast("settings saved");
    let frame = strip(&chat.viewport(WIDTH, HEIGHT, 0));
    assert_eq!(
        frame.len(),
        HEIGHT as usize,
        "fullscreen frames are exactly the terminal's height"
    );
    assert!(
        frame[1].contains("settings saved"),
        "the alt-screen toast rides screen row 1: {:?}",
        frame[1]
    );
}

// Verifies: gh #372 bug 1 - the toast floats over the frame rather than displacing
// it, in **both** modes: the frame keeps the terminal's height and exactly one row
// changes. A toast that grew the frame would move the dock and the caret.
#[test]
fn a_toast_is_an_overlay_in_both_modes() {
    for fullscreen in [true, false] {
        let mut opts = options();
        opts.fullscreen = fullscreen;
        let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
        let before = strip(&chat.viewport(WIDTH, HEIGHT, 0));
        chat.toast("done");
        let after = strip(&chat.viewport(WIDTH, HEIGHT, 0));
        assert_eq!(
            before.len(),
            after.len(),
            "fullscreen={fullscreen}: a toast is an overlay, not a layout change"
        );
        let changed = before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            changed, 1,
            "fullscreen={fullscreen}: one toast touches exactly one row"
        );
        assert!(
            after.iter().any(|row| row.contains("done")),
            "fullscreen={fullscreen}: and that row is visible"
        );
    }
}

// Verifies: gh #372 bug 1 AC5, the sibling - `compose_overlays_bottom_anchored` was
// handed the same whole document two lines before the toast painter, and it does
// **not** share the coordinate confusion. It computes `viewport_start =
// lines.len() - height` and slices, so the modals anchor to the visible bottom.
//
// Pinned rather than merely reported, because the two functions sit beside each other
// and a future "simplification" that made them symmetric in the wrong direction would
// reintroduce the bug for every overlay at once.
#[test]
fn the_sibling_overlay_composer_anchors_to_the_bottom_not_row_one() {
    let mut chat = scrollback_chat(40);
    let before = strip(&chat.viewport(WIDTH, HEIGHT, 0));
    chat.world.panel_open = true;
    let after = strip(&chat.viewport(WIDTH, HEIGHT, 0));

    // The side panel paints the frame's right margin. Document row 1 is a startup
    // header line, so if the composer had spliced there it would show there.
    let early: Vec<usize> = after
        .iter()
        .enumerate()
        .take(after.len().saturating_sub(HEIGHT as usize))
        .filter(|(i, row)| before.get(*i).is_some_and(|b| b != *row))
        .map(|(i, _)| i)
        .collect();
    assert!(
        early.is_empty(),
        "compose_overlays_bottom_anchored rewrote document rows {early:?} above the \
         window - the sibling would then carry the same defect"
    );
}
