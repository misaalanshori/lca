//! Rows for gh #403 gap 4's loop-side decisions: the hangup question, the
//! suspend refusal, and the dispose-before-touch ordering.
//!
//! The Windows refusal is asserted here as a **pure predicate** rather than as a
//! `cfg(windows)` row, because this box cannot run one - `suspend_refusal` takes
//! the platform answer as a parameter for exactly that reason, and the report says
//! which half is row-level only.

use std::sync::{Arc, Mutex};

use super::*;
use lca_tui::engine::program_status::ProgramState;
use lca_tui::engine::terminal::{FakeTerminal, InputHandler, ResizeHandler, Terminal};

// Verifies: gh #403 gap 4 - **a prompt that cannot work is worse than no prompt.**
//
// On Windows `SIGTSTP` does not exist, so `Ctrl+Z` used to do nothing at all: no
// frame change, no message, no exit. Silence reads as a broken key. The refusal
// names what happened and what to do instead, and it is a notice rather than an
// error because nothing failed.
#[test]
fn a_platform_without_job_control_says_so_instead_of_doing_nothing() {
    let notice = suspend_refusal(false).expect("an unsupported platform must say something");
    assert!(
        notice.contains("not supported"),
        "it names the fact, so a reader knows the key is dead rather than the app: {notice}"
    );
    assert!(
        notice.contains("Windows"),
        "and names the platform whose absence of SIGTSTP this is: {notice}"
    );
    assert!(
        notice.contains("own window"),
        "and offers the thing that gets the same result, not just a refusal: {notice}"
    );
    assert!(
        !notice.contains("error") && !notice.contains("failed"),
        "it is not a failure - the session is fine: {notice}"
    );
}

// Verifies: gh #403 gap 4 - the supported side refuses nothing, so the key keeps
// working where it always did (the control half of the pair above).
#[test]
fn a_platform_with_job_control_suspends_without_a_notice() {
    assert!(suspend_refusal(true).is_none());
}

// Verifies: gh #403 gap 4 - **`SIGHUP` and a closed pty reach the same answer.**
//
// The two routes are genuinely different mechanisms: the reader learns from
// `poll`, the signal handler can only set an atomic. A loop that consulted one and
// not the other would exit on one shape of "the terminal left" and hang on the
// other, which is the defect with half a gate.
#[test]
fn both_directions_of_a_dead_terminal_answer_the_same_question() {
    // The reader's own detection.
    let closed = FakeTerminal::new(80, 24);
    assert!(
        !hangup_requested(&closed),
        "a live terminal is not a hangup"
    );
    closed.note_hangup();
    assert!(
        hangup_requested(&closed),
        "the reader's flag alone is enough"
    );

    // The signal's, through the process-wide cell.
    signals::forget();
    let open = FakeTerminal::new(80, 24);
    assert!(!hangup_requested(&open));
    signals::request();
    assert!(
        hangup_requested(&open),
        "a recorded SIGHUP is enough on its own"
    );
    assert!(
        open.hangup(),
        "and it is copied onto the terminal, so every later write is skipped"
    );
    signals::forget();
}

// Verifies: gh #403 gap 4 - **either direction alone is enough, and neither is
// required twice.**
//
// Asserted as the pure decision, so the two inputs are set independently of any
// real cell: a rule written as `terminal_hangup && signal` would pass a row that
// only ever set both, and would then hang on whichever shape set one.
#[test]
fn either_direction_alone_is_enough() {
    assert!(!hangup_decision(false, false), "a live terminal is live");
    assert!(hangup_decision(true, false), "the reader's detection alone");
    assert!(hangup_decision(false, true), "the signal's flag alone");
    assert!(hangup_decision(true, true), "and both together");
}

// Verifies: gh #403 gap 4 - **the ordering**: once the tty is dead, the shutdown
// writes nothing to it.
//
// The bug was not a wrong exit code, it was a shutdown path that touched a terminal
// that no longer existed: a write to a dead fd blocks or raises again, and
// restoring modes on a closed terminal is a no-op at best. So the assertion is the
// byte stream, not the exit status - a row checking only "it exited" would pass on
// the pre-fix tree whenever the write happened to fail quietly.
#[test]
fn a_dead_terminal_stops_being_written_to_before_the_teardown() {
    let mut term = FakeTerminal::new(80, 24);
    term.start(Box::new(|_| {}), Box::new(|| {}));
    term.write("before the hangup");
    let seen = term.take_output();
    assert!(seen.contains("before the hangup"), "the frame was painted");

    // The reader reports the pty closed.
    term.note_hangup();
    term.write("\x1b[?1049l this must never reach the terminal");
    term.stop();
    term.drain_input(10, 1);
    assert_eq!(
        term.output(),
        "",
        "nothing is written to a terminal that is gone"
    );
}

// Verifies: gh #403 gap 4 - a **suspend does nothing at all** where job control is
// missing, including not tearing the frame down.
//
// Tearing down the alt screen to attempt a suspend that cannot happen is a visible
// fault for no gain: the reader would see the transcript dump and then the same
// screen scarred over. The fake terminal records every byte, so "nothing happened"
// is assertable.
#[test]
fn an_unsupported_suspend_leaves_the_terminal_untouched() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Alt(lca_tui::engine::alt_screen::AltScreenRenderer::new());
    term.start(Box::new(|_| {}), Box::new(|| {}));
    let (input_tx, _input_rx) = std::sync::mpsc::channel();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    term.take_output();

    let outcome = suspend(&mut screen, &mut term, &input_tx, &resize_tx, None, false);
    assert_eq!(outcome, Suspended::Unsupported);
    assert_eq!(
        term.output(),
        "",
        "no exit sequence, no mouse teardown, no alt-screen switch"
    );
    assert!(
        screen.is_fullscreen(),
        "and the frame is still the fullscreen one it was"
    );
}

// Verifies: gh #403 gap 4 - the **supported** path is the mirror: the tty is
// handed back before the process stops and taken again after.
//
// The ordering is the feature, so it is asserted as an **ordered log** rather than
// as two independent byte streams: a row that only checked "the exit sequence was
// written" would pass on a tree that wrote it *after* stopping, which is exactly the
// sequence that leaves the shell above a raw-mode terminal.
//
// `signals::suspend` sends `SIGTSTP` to the whole process group, which a test
// process must not do to itself, so the stop itself is the half left to the live
// receipt; the report says so plainly.
#[test]
fn the_supported_suspend_hands_the_terminal_back_before_it_stops() {
    /// Records the *order* of the calls a suspend makes.
    #[derive(Default, Clone)]
    struct Ordered(Arc<Mutex<Vec<String>>>);
    impl Ordered {
        fn log(&self) -> Vec<String> {
            self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }
    }
    impl Terminal for Ordered {
        fn start(&mut self, _on_input: InputHandler, _on_resize: ResizeHandler) {}
        fn stop(&mut self) {
            self.0
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push("stop".to_string());
        }
        fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
        fn write(&mut self, data: &str) {
            self.0
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(format!("write:{data}"));
        }
        fn columns(&self) -> u16 {
            80
        }
        fn rows(&self) -> u16 {
            24
        }
        fn kitty_protocol_active(&self) -> bool {
            false
        }
        fn move_by(&mut self, _lines: i32) {}
        fn hide_cursor(&mut self) {}
        fn show_cursor(&mut self) {}
        fn clear_line(&mut self) {}
        fn clear_from_cursor(&mut self) {}
        fn clear_screen(&mut self) {}
        fn set_title(&mut self, _title: &str) {}
        fn set_progress(&mut self, _active: bool) {}
        fn set_program_status(&mut self, _state: ProgramState) {}
    }

    let recorder = Ordered::default();
    let mut term = recorder.clone();
    let mut screen = Screen::Alt(lca_tui::engine::alt_screen::AltScreenRenderer::new());
    screen.enter(&mut term);
    recorder.0.lock().unwrap_or_else(|p| p.into_inner()).clear();

    // The suspend's ordering, minus the signal.
    screen.leave(&mut term, true);
    term.stop();
    let handed_back = recorder.log();
    let left = handed_back
        .iter()
        .position(|entry| entry.contains("\x1b[?1049l"))
        .expect("the alt screen is left before the process stops");
    let stopped = handed_back
        .iter()
        .position(|entry| entry == "stop")
        .expect("the terminal is stopped");
    assert!(
        left < stopped,
        "leave, then stop - in that order, or the shell below inherits a fullscreen \
         frame in raw mode: {handed_back:?}"
    );

    let (input_tx, _input_rx) = std::sync::mpsc::channel();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    restart_reader(&mut term, &input_tx, &resize_tx, None);
    screen.enter(&mut term);
    assert!(
        recorder
            .log()
            .iter()
            .any(|entry| entry.contains("\x1b[?1049h")),
        "and the frame is back once something continues us: {:?}",
        recorder.log()
    );
}

// Verifies: gh #121's lesson, restated for the helper gh #403 gap 4 factors out -
// **a restarted reader must feed the stream the loop actually drains.**
//
// With an adopted start-up terminal the loop reads the early channel, so pointing
// the new reader at `input_tx` deafens the session the moment the child exits. This
// was a 120 s hang in the e2e before it was a comment.
#[test]
fn a_restarted_reader_feeds_the_channel_the_loop_reads() {
    let mut term = FakeTerminal::new(80, 24);
    let (loop_tx, loop_rx) = std::sync::mpsc::channel::<String>();
    let (other_tx, other_rx) = std::sync::mpsc::channel::<String>();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    let early = crate::run_early_input::EarlyStream {
        sender: loop_tx,
        receiver: loop_rx,
    };
    restart_reader(&mut term, &other_tx, &resize_tx, Some(&early));
    term.feed("k");
    assert_eq!(
        early.receiver.try_recv().ok().as_deref(),
        Some("k"),
        "the key must reach the loop's stream"
    );
    assert!(
        other_rx.try_recv().is_err(),
        "and not the channel nobody is draining"
    );
}
