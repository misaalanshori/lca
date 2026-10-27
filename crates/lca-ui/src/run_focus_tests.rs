//! Gh #374 gap 2, the host's half: focus reports reach the renderer, and the
//! gesture state the renderer cannot see is dropped with them.
//!
//! The renderer rows (in `lca-tui`'s `alt_screen::focus_tests`) prove that *given*
//! a `FOCUS_OUT` a finished selection survives while an interrupted one does not.
//! These rows prove the other half, and it is the half the defect hid in: the
//! host only routed input starting with `ESC [ <` to the renderer, so `ESC [ O`
//! never got there at all. A guard on an unreachable arm is dead code that every
//! renderer row still passes — the #377/#379 failure mode — so the dispatch is
//! pinned here, from outside.
//!
//! Verifies: gh #374 gap 2 and NFR-24 (a released defect gets a named guard).

use super::*;
use lca_tui::engine::terminal::FakeTerminal;

/// A fullscreen screen, a chat, and the channels `handle_input` needs.
struct Harness {
    chat: Chat,
    screen: Screen,
    term: FakeTerminal,
    input_tx: std::sync::mpsc::Sender<String>,
    resize_tx: std::sync::mpsc::Sender<()>,
    _input_rx: std::sync::mpsc::Receiver<String>,
    _resize_rx: std::sync::mpsc::Receiver<()>,
}

impl Harness {
    fn new() -> Self {
        let mut term = FakeTerminal::new(80, 24);
        let mut screen = Screen::Main(MainScreenRenderer::new());
        // Fullscreen: the alt screen is the renderer that owns a selection.
        switch_screen(&mut screen, true, &mut term);
        let mut chat = chat();
        chat.screen_mode = true;
        let (input_tx, _input_rx) = std::sync::mpsc::channel();
        let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
        Self {
            chat,
            screen,
            term,
            input_tx,
            resize_tx,
            _input_rx,
            _resize_rx,
        }
    }

    /// One frame, so the renderer has rows for a selection to address.
    fn paint(&mut self, rows: &[&str]) {
        let lines: Vec<String> = rows.iter().map(|s| s.to_string()).collect();
        self.screen
            .render(&mut self.term, lines, Vec::new(), 80, 24);
        let _ = self.term.take_output();
    }

    fn send(&mut self, data: &str) -> InputResult {
        handle_input(
            data,
            &mut self.chat,
            &mut self.screen,
            &mut self.term,
            &self.input_tx,
            &self.resize_tx,
            None,
            &None,
            &mut false,
        )
    }

    fn selected(&self) -> String {
        self.screen.selected_text()
    }
}

/// A finished drag: press, motion, release — the shape the bug wipes.
fn finished_drag(h: &mut Harness, from: (u16, u16), to: (u16, u16)) {
    h.send(&format!("\x1b[<0;{};{}M", from.0, from.1));
    h.send(&format!("\x1b[<32;{};{}M", to.0, to.1));
    h.send(&format!("\x1b[<0;{};{}m", to.0, to.1));
}

// Verifies: gh #374 gap 2 - `FOCUS_OUT` reaches the renderer at all, and a
// finished selection survives it with the same bytes.
//
// This is the row that would have caught the defect twice over: the old code
// cleared the selection, and even a guarded version would have been unreachable
// because the dispatch stopped at `ESC [ <`.
//
// The selection alone does not prove reachability - a sequence that is dropped
// entirely also leaves a finished selection standing, which is a vacuous pass.
// So the row also asserts a side effect only the renderer produces: focus loss
// drops the scrollbar hover flag, which `handle_mouse` never needs and nothing
// else in this path writes.
#[test]
fn a_focus_loss_reaches_the_renderer_and_a_finished_selection_survives() {
    let mut h = Harness::new();
    h.paint(&["hello world"]);
    finished_drag(&mut h, (1, 1), (6, 1));
    assert_eq!(h.selected(), "hello", "the drag selected, and released");
    // Armed *after* the release: the release itself re-derives the hover from a
    // hit test, so setting it before would let the assertion pass trivially.
    h.screen.set_scrollbar_hover(true);

    assert!(matches!(h.send("\x1b[O"), InputResult::Continue));
    assert_eq!(
        h.selected(),
        "hello",
        "the finished selection survived the focus loss - this is the bug"
    );
    assert!(
        !h.screen.scrollbar_hovered(),
        "and the renderer ran: it dropped the scrollbar hover, so the sequence \
         was dispatched rather than swallowed"
    );
}

// Verifies: gh #374 gap 2 - the mirror: an unreleased press is cancelled, so the
// dispatch is not merely forwarding the sequence to something that keeps it.
#[test]
fn a_focus_loss_reaches_the_renderer_and_cancels_an_interrupted_drag() {
    let mut h = Harness::new();
    h.paint(&["hello world"]);
    h.send("\x1b[<0;1;1M");
    h.send("\x1b[<32;6;1M");
    assert_eq!(h.selected(), "hello", "mid-drag there is a highlight");

    h.send("\x1b[O");
    assert_eq!(
        h.selected(),
        "",
        "an interrupted drag keeps nothing after focus loss"
    );
}

// Verifies: gh #374 gap 2 - the host's own gesture state is dropped too. The
// scrollbar grab offset lives on `Chat`, where no renderer reset can reach it;
// leaving it armed would make the next motion scroll a bar the user is not
// holding - the error that the "just guard the clear" fix would have shipped.
#[test]
fn focus_loss_drops_the_host_side_scrollbar_grab() {
    let mut h = Harness::new();
    h.paint(&["hello world"]);
    h.chat.scroll_drag = Some(2);
    h.chat.scrollbar_hover = true;

    h.send("\x1b[O");
    assert!(
        h.chat.scroll_drag.is_none(),
        "the scrollbar grab is gone: a later motion is not a drag"
    );
    assert!(
        !h.chat.scrollbar_hover,
        "and the hover is gone with the pointer"
    );
}

// Verifies: gh #374 gap 2 - `FOCUS_IN` is consumed and resumes nothing, asserted
// through the contract a user feels: a press, then a focus round trip, then an
// orphaned release must produce **no highlight and no copy**. If `FOCUS_OUT`
// never reached the renderer the press would still be live, and that release
// would paint a selection and copy it - which is pi's
// "orphaned events must not extend it" case (`tui-alt-screen.test.ts`).
#[test]
fn focus_in_resumes_nothing_and_an_orphaned_release_neither_selects_nor_copies() {
    let mut h = Harness::new();
    h.paint(&["hello world"]);
    // A press and a move, then the focus round trip - no release yet.
    h.send("\x1b[<0;1;1M");
    h.send("\x1b[<32;6;1M");
    assert_eq!(h.selected(), "hello", "mid-drag there is a highlight");

    h.send("\x1b[O");
    assert!(matches!(h.send("\x1b[I"), InputResult::Continue));
    assert_eq!(
        h.selected(),
        "",
        "the interrupted gesture stayed cancelled across the round trip"
    );
    let _ = h.term.take_output();

    // The release that arrives after focus returns belongs to a gesture that no
    // longer exists.
    h.send("\x1b[<0;6;1m");
    assert_eq!(
        h.selected(),
        "",
        "an orphaned release paints nothing: the gesture was aborted, not paused"
    );
    let out = h.term.output();
    assert!(
        !out.contains("\x1b]52;c;"),
        "and copies nothing - a stale release must not reach the clipboard: {out:?}"
    );
    assert_eq!(
        h.chat.editor.text(),
        "",
        "and the sequence did not reach the composer as typing"
    );
}

// Verifies: gh #374 gap 2 - copy after focus loss still works, read at the wire
// rather than at the renderer: the `OSC 52` the copy path emits still carries the
// selection's bytes after a `FOCUS_OUT`. The brief asks for this explicitly,
// because a selection that survives but cannot be copied is a different bug.
#[test]
fn a_copy_after_focus_loss_still_puts_the_bytes_on_the_wire() {
    let mut h = Harness::new();
    h.paint(&["hello world"]);
    finished_drag(&mut h, (1, 1), (6, 1));
    h.send("\x1b[O");
    let _ = h.term.take_output();

    // Ctrl+X with a live selection copies it through the ladder (gh #9); this
    // host has no native clipboard, so OSC 52 is the honest step.
    h.send("\x18");
    let out = h.term.output();
    assert!(
        out.contains("\x1b]52;c;"),
        "the copy still ran after focus loss: {out:?}"
    );
    assert!(
        out.contains("aGVsbG8="),
        "and carried the selection (`hello`, base64): {out:?}"
    );
}
