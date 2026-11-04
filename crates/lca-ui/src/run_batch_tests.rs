//! Rows for gh #370 flaw 2: one iteration, many events.
//!
//! # What these rows are and are not
//!
//! The loop itself needs a terminal, a turn runner and a wake channel, so it is not
//! reachable from a unit test. What *is* reachable is the drain and the handling, and
//! the quantitative claim of the flaw is about **frame count**, which the fake terminal
//! exposes in the byte stream: every fullscreen frame is one `?2026h … ?2026l` block.
//!
//! So the two shapes are driven here with the same helpers the loop uses - one event per
//! paint (the old shape) and one paint per drained batch (the new one) - and the rows
//! assert both the frame count **and** that the two land the same final state. That is
//! the issue's claim measured, rather than a structural mirror of the loop: if the
//! batching dropped or reordered an event, the final state would differ.
//!
//! [`the_loop_drains_before_it_paints`](crate::run_batch) - the structural half - is
//! pinned in `tests/regressions/370-loop-wakes-on-every-source.rs`, because the
//! regression it prevents is someone "simplifying" the loop back to a single pop.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_tui::engine::main_screen::MainScreenRenderer;
use lca_tui::engine::terminal::FakeTerminal;

use crate::chat::Chat;
use crate::run::tests::{chat, options, test_waker};
use crate::run::{Screen, handle_input, input_helpers, switch_screen};
use lca_tui::engine::terminal::Terminal;

use input_helpers::{MAX_DRAINED, drain_input};

/// A drain over a fixed list of events, standing in for the loop's `try_recv`.
fn from_list(events: &mut Vec<String>) -> Vec<String> {
    let mut next = || {
        if events.is_empty() {
            None
        } else {
            Some(events.remove(0))
        }
    };
    drain_input(&mut next)
}

/// Drive the chat and the screen the way one loop iteration does, and paint the way the
/// loop's `if dirty { … }` block does.
///
/// This mirrors the loop's paint block on purpose: the rows here are about *how many*
/// times it runs, and mirroring is the only way to run it at all. The loop's own shape -
/// one `screen.render`, one `if dirty` - is guarded by the `370-*` regression file, so a
/// drift between this mirror and the loop is caught there rather than silently here.
struct Driver {
    chat: Chat,
    screen: Screen,
    term: FakeTerminal,
}

impl Driver {
    fn new() -> Self {
        let mut term = FakeTerminal::new(80, 24);
        let mut screen = Screen::Main(MainScreenRenderer::new());
        switch_screen(&mut screen, true, &mut term);
        Self {
            chat: chat(),
            screen,
            term,
        }
    }

    /// One loop iteration over one event: handle it, then paint. The **old** shape.
    fn iteration_per_event(&mut self, data: &str) {
        let (input_tx, _input_rx) = std::sync::mpsc::channel();
        let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
        let _ = handle_input(
            data,
            &mut self.chat,
            &mut self.screen,
            &mut self.term,
            &input_tx,
            &resize_tx,
            None,
            &None,
            &mut false,
        );
        self.paint();
    }

    /// The **new** shape: every drained event is handled in order, then one paint.
    fn iteration_over_batch(&mut self, batch: &[String]) {
        let (input_tx, _input_rx) = std::sync::mpsc::channel();
        let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
        for data in batch {
            let _ = handle_input(
                data,
                &mut self.chat,
                &mut self.screen,
                &mut self.term,
                &input_tx,
                &resize_tx,
                None,
                &None,
                &mut false,
            );
        }
        self.paint();
    }

    fn paint(&mut self) {
        let (width, height) = (self.term.columns(), self.term.rows());
        let scroll = self.chat.clamp_scroll(self.screen.scroll(), width, height);
        self.screen.set_scroll(scroll);
        let viewport = self.chat.viewport(width, height, scroll);
        let document = self.chat.take_document();
        self.screen
            .render(&mut self.term, viewport, document, width, height);
    }

    /// How many fullscreen frames the terminal received.
    ///
    /// Every frame is one `?2026h … ?2026l` block, so the closing byte is a frame
    /// marker. Counting by bytes rather than by a counter the implementation holds is
    /// what makes this a receipt: a counter inside the loop would count the loop's
    /// opinion of its own work.
    fn frames(&self) -> usize {
        self.term.output().matches("\x1b[?2026l").count()
    }

    /// What the composer holds, which is what a dropped keystroke would change.
    fn composer(&self) -> String {
        self.chat.editor.text()
    }
}

/// An SGR left-button press/motion/release at 1-based (col, row).
fn sgr(bits: u8, col: u16, row: u16, press: bool) -> String {
    format!("\x1b[<{bits};{col};{row}{}", if press { 'M' } else { 'm' })
}

// Verifies: gh #370 flaw 2 - **a burst of N distinct events costs one frame, not N**.
// This is the flaw's quantitative claim, measured the way the issue states it: the same
// events, the old shape and the new one, frames counted.
#[test]
fn a_burst_of_n_events_costs_one_frame_not_n() {
    // Twelve keystrokes, which is the smallest burst a fast typist produces and enough
    // that the difference is unambiguous.
    let burst: Vec<String> = "hello world!".chars().map(|c| c.to_string()).collect();
    assert_eq!(burst.len(), 12, "the fixture is twelve single-char events");

    let mut per_event = Driver::new();
    for event in &burst {
        per_event.iteration_per_event(event);
    }

    let mut batched = Driver::new();
    let mut queue = burst.clone();
    let batch = from_list(&mut queue);
    batched.iteration_over_batch(&batch);

    assert_eq!(
        per_event.composer(),
        "hello world!",
        "the old shape types every character (the fixture really applies)"
    );
    assert_eq!(
        batched.composer(),
        per_event.composer(),
        "**every** character survives the batch - a drain that kept only the last event \
         would leave `!` and nothing else"
    );
    assert_eq!(
        batched.frames(),
        1,
        "twelve events, one frame: found {}",
        batched.frames()
    );
    assert_eq!(
        per_event.frames(),
        per_event.composer().chars().count(),
        "and the old shape really was one frame per event, so the comparison is honest"
    );
    assert_eq!(
        batched.frames(),
        per_event.frames() - (burst.len() - 1),
        "the saving is exactly the events coalesced away"
    );
}

// Verifies: gh #370 flaw 2 - **a drain of keystrokes applies every character, in
// order**. The row a "keep the last event" drain fails: it would look like a speedup
// and eat the other eleven characters.
#[test]
fn a_drained_burst_of_keystrokes_applies_every_character_in_order() {
    let mut queue: Vec<String> = vec!["a", "b", "c", "\u{1b}[D", "X", "y"]
        .into_iter()
        .map(String::from)
        .collect();
    let batch = from_list(&mut queue);
    assert_eq!(batch.len(), 6, "every queued event was taken");
    assert!(queue.is_empty(), "and the channel is drained");

    let mut driver = Driver::new();
    driver.iteration_over_batch(&batch);
    // `abc`, left (the caret is now before `c`), insert `X` -> `abXc`, then `y` at that
    // same caret -> `abXyc`. A drain that kept only the last event would hold `y`.
    assert_eq!(
        driver.composer(),
        "abXyc",
        "every event was handled, and the order survives: a reordering would place `X` \
         somewhere else"
    );
}

// Verifies: gh #370 flaw 2 - **the drain is bounded, so a continuous producer cannot
// starve the frame**. A drained batch never exceeds `MAX_DRAINED`, and what is past the
// cap stays queued for the next iteration rather than being lost.
#[test]
fn a_continuous_producer_cannot_starve_the_frame() {
    let total = MAX_DRAINED * 2 + 37;
    let mut queue: Vec<String> = (0..total).map(|_| String::from("x")).collect();
    let batch = from_list(&mut queue);
    assert_eq!(
        batch.len(),
        MAX_DRAINED,
        "the drain stops at the bound rather than following a producer forever"
    );
    assert_eq!(
        queue.len(),
        total - MAX_DRAINED,
        "and the rest is still queued, not dropped"
    );

    // The bound is not a per-session limit: the next iteration takes the next slice.
    let second = from_list(&mut queue);
    assert_eq!(second.len(), MAX_DRAINED, "the loop comes straight back");
}

// Verifies: gh #370 flaw 2 - **an empty drain is not one empty event.** The idle case
// must return nothing, so the caller's "handle every event" loop does nothing and the
// paint decision stays with `dirty`. A drain that yielded a sentinel would type a stray
// byte or paint a spurious frame on every idle tick.
#[test]
fn an_empty_channel_drains_to_nothing() {
    let mut queue: Vec<String> = Vec::new();
    assert!(from_list(&mut queue).is_empty());
}

// Verifies: gh #370 flaw 2 - **a mouse-move burst is handled in order, and the frame
// count is one.** The brief's suggested shape was "collapse to the last position";
// LCA handles every position instead, because the intermediate ones are observable to
// the *gesture* (a drag's selection is built from the positions it passed through). For
// a drag the final state happens to be the same either way - which is exactly why
// collapsing looked plausible - so this row pins the frames saved and the fact that no
// position was dropped, and the keystroke row above is what proves the distinction has
// teeth.
#[test]
fn a_mouse_move_burst_paints_once_and_keeps_every_position() {
    let mut queue: Vec<String> = Vec::new();
    // A press in the transcript, then a drag rightwards and down, then release.
    queue.push(sgr(0, 5, 5, true));
    for col in (6..=15).step_by(1) {
        // SGR motion: bit 32 set, button 0 held (bits 0..1 = 0).
        queue.push(sgr(32, col, 5 + (col - 6) / 4, true));
    }
    queue.push(sgr(0, 15, 7, false));
    let moves = queue.len() - 2;
    assert_eq!(moves, 10, "ten motions between the press and the release");

    let batch = from_list(&mut queue);
    assert_eq!(batch.len(), moves + 2, "all of them were drained");

    let mut driver = Driver::new();
    driver.iteration_over_batch(&batch);
    assert_eq!(
        driver.frames(),
        1,
        "the whole drag cost one frame: found {}",
        driver.frames()
    );

    // The selection the drag built is the range the positions describe, so the last
    // position is what the final state shows - the reason collapsing would have been
    // invisible here.
    let mut per_event = Driver::new();
    let mut single = Vec::new();
    single.push(sgr(0, 5, 5, true));
    for col in (6..=15).step_by(1) {
        single.push(sgr(32, col, 5 + (col - 6) / 4, true));
    }
    single.push(sgr(0, 15, 7, false));
    for event in &single {
        per_event.iteration_per_event(event);
    }
    assert_eq!(
        driver.frames() + (single.len() - 1),
        per_event.frames(),
        "the frames saved are the motion count"
    );
    // The positions are observable to the **gesture**, which is why collapsing them was
    // rejected: the highlight is built from the cells the drag passed through. If the
    // drain had kept only the release, or reordered the motions, this range would be
    // empty or shorter - so the range is the proof that every position was handled.
    let batched_selection = driver.screen.selected_text();
    assert!(
        !batched_selection.trim().is_empty(),
        "the drag selected text, so the press and motions were applied"
    );
    assert_eq!(
        batched_selection,
        per_event.screen.selected_text(),
        "and the batch built the same highlight as one event per frame - batching changed          the frame count, not the gesture"
    );
}

// Verifies: gh #370 flaw 2 - **a paste is not split by the drain.** A bracketed paste
// arrives as one `data` value from the terminal's reader (FR-UI-10), so the drain takes
// it whole; a drain that split input into characters, or that capped mid-paste, would
// corrupt a multi-line paste. The row uses a paste larger than nothing in particular but
// asserts the atomicity claim: one event in, all of it in the composer.
#[test]
fn a_paste_arrives_atomically_through_the_batch() {
    let payload = "fn main() {\n    println!(\"hi\");\n}\n";
    let paste = format!("\u{1b}[200~{payload}\u{1b}[201~");
    let mut queue: Vec<String> = vec![String::from("p"), String::from("q"), paste.clone()];
    let batch = from_list(&mut queue);
    assert_eq!(batch.len(), 3, "the paste is one event among three");
    assert_eq!(batch[2], paste, "and it is byte-identical, unsplit");

    let mut driver = Driver::new();
    driver.iteration_over_batch(&batch);
    let expected = format!("pq{payload}");
    assert_eq!(
        driver.composer(),
        expected,
        "the whole block landed, in order, after the two keystrokes"
    );
    assert_eq!(
        driver.frames(),
        1,
        "and it cost one frame, not one per line"
    );
}

// Verifies: gh #370 flaw 2 - **one keystroke after an idle period is handled on the
// iteration it arrives.** The latency half: the drain must not defer a lone event, so a
// drain of a single event is exactly what the old shape did. Asserted as a count rather
// than a stopwatch, because a duration assertion on a shared CI box is a coin flip - the
// *deterministic* statement is "one event, one iteration, one frame".
#[test]
fn a_lone_keystroke_is_handled_on_its_own_iteration() {
    let mut queue: Vec<String> = vec![String::from("z")];
    let batch = from_list(&mut queue);
    assert_eq!(
        batch,
        vec![String::from("z")],
        "taken immediately, not held"
    );
    let mut driver = Driver::new();
    driver.iteration_over_batch(&batch);
    assert_eq!(driver.composer(), "z", "and handled on that iteration");
    assert_eq!(driver.frames(), 1, "one event is still one frame");
}

// Verifies: gh #370 flaw 2 - **the bound never splits a realistic burst.** The cap
// exists only to stop a pathology, so it must sit far above anything a frame's worth of
// real input produces; a cap that a normal drag hit would reintroduce the flaw one
// iteration later. Measured rather than asserted against itself: the largest burst these
// rows drive is drained in one go.
#[test]
fn the_bound_never_splits_a_realistic_burst() {
    // The widest real burst any row here drives is `MAX_DRAINED * 2 + 37`; a *realistic*
    // one is a fast drag or a paste, both far smaller. Drive a 200-event drag - a
    // 200-cell sweep, well beyond any single frame's input - and require one batch.
    let mut queue: Vec<String> = Vec::new();
    queue.push(sgr(0, 1, 5, true));
    for col in 2..200u16 {
        queue.push(sgr(32, col, 5, true));
    }
    queue.push(sgr(0, 199, 5, false));
    assert_eq!(queue.len(), 200, "the fixture is a 200-cell drag");
    let batch = from_list(&mut queue);
    assert_eq!(
        batch.len(),
        200,
        "a 200-cell drag is one batch, so the bound never splits real input \
         (MAX_DRAINED = {MAX_DRAINED})"
    );
    let _ = test_waker(); // the loop's waker exists; these rows never block on it
    let _ = options(); // and `tests::options` is still the shared fixture
}
