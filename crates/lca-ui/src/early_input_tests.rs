//! The early-input buffer (gh #400, type-while-loading).
//!
//! The defect these rows pin is a **measurement**, not an impression: with
//! `lca` starting, a `Ctrl+C` sent at 150 ms did not exit, while the same key at
//! 3 s did. The window is the load between process start and `lca_ui::run`
//! entering raw mode, and everything typed into it is either late (plain text,
//! held by the tty until raw mode drains it) or inert (quit keys, which nothing
//! was listening for).
//!
//! The buffer's contract has three parts, and each gets its own row below:
//! **nothing dropped**, **order preserved**, **quit acted on immediately**.
//!
//! The buffer takes whole `data` values, never characters. That is what makes a
//! bracketed paste atomic without any paste-specific code (FR-UI-10): the
//! terminal's reader already assembles one, so a paste that arrives mid-load is
//! one value in and one value out.

use super::*;

/// A test buffer, built without a terminal: the receiver is ours.
fn buffer() -> (std::sync::mpsc::Sender<String>, EarlyInput) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let sender = tx.clone();
    let mut terminal = ProcessTerminal::new();
    terminal.start(Box::new(|_| {}), Box::new(|| {}));
    (
        tx,
        EarlyInput {
            terminal,
            receiver: rx,
            sender,
            quit: false,
            pending: Vec::new(),
        },
    )
}

// Verifies: gh #400 - **nothing is dropped, and order is preserved.** A user who
// types during the load gets exactly the text they typed, in the order they
// typed it. The expected string is spelled out rather than counted, because
// "three characters arrived" passes even when they are the wrong three.
#[test]
fn keys_typed_during_load_are_replayed_in_order() {
    let (tx, mut early) = buffer();
    for key in ["h", "e", "l", "l", "o"] {
        tx.send(key.to_string()).expect("send");
    }
    assert!(!early.drain(), "no quit among them");

    let (_terminal, stream, replayed) = early.into_loop();
    let receiver = stream.receiver;
    assert_eq!(
        replayed,
        vec!["h", "e", "l", "l", "o"],
        "every keystroke, in the order it was typed"
    );
    // The receiver is handed over live: a key that arrives after the loop
    // starts still lands, which is the window `into_loop` must not close.
    drop(receiver);
}

// Verifies: gh #400 - **a quit key is acted on immediately**, not buffered
// behind the load. This is the measured defect: `Ctrl+C` at 150 ms did nothing
// while the same key later exited.
#[test]
fn a_quit_during_load_is_reported_not_buffered() {
    let (tx, mut early) = buffer();
    tx.send("a".to_string()).expect("send");
    tx.send("\u{3}".to_string()).expect("Ctrl+C");
    tx.send("b".to_string()).expect("send");
    assert!(early.drain(), "the quit is reported");
    assert!(early.quit(), "and remembered");

    // The quit is *not* replayed: the phase that saw it already acted, and
    // exiting twice is not a thing.
    let (_terminal, stream, replayed) = early.into_loop();
    let receiver = stream.receiver;
    assert_eq!(replayed, vec!["a", "b"], "the surrounding text survives");
    assert!(
        !replayed.iter().any(|data| is_quit(data)),
        "the quit key is consumed by the phase that saw it"
    );
    drop(receiver);
}

// Verifies: gh #400 - `Ctrl+D` is the other quit key. During load there is no
// editor to delete forward in, so it means exit, exactly as it does at an empty
// prompt.
#[test]
fn ctrl_d_also_quits_during_load() {
    let (tx, mut early) = buffer();
    tx.send("\u{4}".to_string()).expect("Ctrl+D");
    assert!(early.drain());
    assert!(is_quit("\u{3}") && is_quit("\u{4}"));
    assert!(!is_quit("d"), "a literal `d` is text, not a quit");
    assert!(!is_quit("\u{1b}"), "Escape is not a quit");
}

// Verifies: gh #400 (FR-UI-10) - **a paste is one atomic segment.** The buffer
// stores whole `data` values, so a multi-character paste that arrives mid-load
// is replayed as one value: it cannot be split, and it cannot reflow against a
// later keystroke.
#[test]
fn a_paste_arrives_as_one_segment() {
    let (tx, mut early) = buffer();
    // The terminal's reader hands a bracketed paste over as one `data` value.
    let paste = "line one\nline two\n";
    tx.send(paste.to_string()).expect("paste");
    tx.send("!".to_string()).expect("a keystroke after it");
    early.drain();

    let (_terminal, stream, replayed) = early.into_loop();
    let receiver = stream.receiver;
    assert_eq!(
        replayed,
        vec![paste.to_string(), "!".to_string()],
        "the paste is one segment, then the keystroke"
    );
    drop(receiver);
    assert!(
        replayed[0].contains('\n'),
        "a multi-line paste keeps its newlines"
    );
}

// Verifies: gh #400 - an empty buffer is a no-op: the loop starts with no
// replay and nothing invented. The common case must not cost anything.
#[test]
fn an_empty_buffer_replays_nothing() {
    let (_tx, mut early) = buffer();
    assert!(!early.drain());
    let (_terminal, stream, replayed) = early.into_loop();
    let receiver = stream.receiver;
    assert!(replayed.is_empty());
    drop(receiver);
}

// Verifies: gh #400 - **draining twice loses nothing.** The startup phase
// checks between steps, so `drain` is called repeatedly; a keystroke that
// arrived between two checks must not be lost by the earlier one.
#[test]
fn repeated_drains_accumulate() {
    let (tx, mut early) = buffer();
    tx.send("1".to_string()).expect("send");
    early.drain();
    tx.send("2".to_string()).expect("send");
    early.drain();
    tx.send("3".to_string()).expect("send");
    early.drain();

    let (_terminal, stream, replayed) = early.into_loop();
    let receiver = stream.receiver;
    assert_eq!(replayed, vec!["1", "2", "3"]);
    drop(receiver);
}

// Verifies: gh #400 - the buffer is **taken**, not copied, when the loop starts.
// A key replayed twice would appear twice in the composer, which is the sort of
// bug that looks like a stuck key.
#[test]
fn the_buffer_is_consumed_not_duplicated() {
    let (tx, mut early) = buffer();
    tx.send("x".to_string()).expect("send");
    early.drain();

    let (_terminal, receiver, first) = early.into_loop();
    assert_eq!(first, vec!["x"]);
    drop(receiver);
    // `into_loop` consumed `self`; the compile-time shape is the guarantee, and
    // this row documents it.
}

// Verifies: gh #400 - **all three spellings of Ctrl+C are a quit.** Found live,
// not in a unit test: `tmux` delivered `\x1b[99;5u` (Kitty CSI-u) because
// `start()` negotiates the keyboard protocol, so a buffer that only matched the
// raw `\u{3}` saw a key it did not recognise and the user's Ctrl+C during load
// did nothing. The keybindings engine already knows all three; startup has no
// keybindings manager yet, so the table lives here too.
#[test]
fn every_spelling_of_ctrl_c_and_ctrl_d_is_a_quit() {
    for sequence in [
        "\u{3}",           // the plain C0 byte
        "\u{1b}[99;5u",    // Kitty CSI-u
        "\u{1b}[27;5;99~", // xterm modifyOtherKeys
    ] {
        assert!(
            is_quit(sequence),
            "Ctrl+C in the {sequence:?} spelling is a quit"
        );
    }
    for sequence in ["\u{4}", "\u{1b}[100;5u", "\u{1b}[27;5;100~"] {
        assert!(
            is_quit(sequence),
            "Ctrl+D in the {sequence:?} spelling is a quit"
        );
    }
    // ...and a near miss is not: Kitty sends `ctrl+shift+c` as `99;6u`.
    assert!(!is_quit("\u{1b}[99;6u"), "shift+ctrl+c is not a quit");
    assert!(!is_quit("\u{1b}[97;5u"), "ctrl+a is not a quit");
    assert!(!is_quit("c"), "a literal c is text");
}

// Verifies: gh #400 - the Kitty spelling arrives as one `data` value and is
// consumed by the quit path, not replayed into the editor as escape junk.
#[test]
fn a_kitty_ctrl_c_is_consumed_rather_than_replayed() {
    let (tx, mut early) = buffer();
    tx.send("before".to_string()).expect("send");
    tx.send("\u{1b}[99;5u".to_string()).expect("Ctrl+C");
    assert!(early.drain(), "recognised as a quit");
    let (_terminal, stream, replayed) = early.into_loop();
    let receiver = stream.receiver;
    assert_eq!(
        replayed,
        vec!["before"],
        "no escape sequence leaks into the editor"
    );
    drop(receiver);
}

// Verifies: gh #400 - **the adopted stream survives a `$EDITOR` restart.** The
// editor path stops the terminal and starts it again; if the restart feeds the
// loop's *unused* channel instead of the adopted one, the session goes deaf
// once the editor exits. That is not hypothetical: it was a 120 s timeout in
// gh #121's real-terminal row, found by the full suite and nothing else.
//
// The stream carries both halves for exactly this reason, so a restart can
// re-point the reader at the channel the loop is draining.
#[test]
fn the_adopted_stream_can_be_reused_after_a_stop_and_start() {
    let (tx, early) = buffer();
    let (_terminal, stream, _pending) = early.into_loop();
    // A key arriving before any restart is on the adopted stream.
    tx.send("before".to_string()).expect("send");
    assert_eq!(
        stream.recv_timeout(std::time::Duration::from_millis(50)),
        Ok("before".to_string())
    );
    // `$EDITOR` exits and the reader is re-pointed at `stream.sender` (what
    // `run.rs` does). A key must still reach the same receiver.
    let restarted = stream.sender.clone();
    restarted.send("after".to_string()).expect("send");
    assert_eq!(
        stream.recv_timeout(std::time::Duration::from_millis(50)),
        Ok("after".to_string()),
        "the restarted reader feeds the stream the loop reads, not another one"
    );
}
