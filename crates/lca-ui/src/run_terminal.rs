//! The three things the loop does *to* the terminal rather than through it
//! (gh #403 gap 4).
//!
//! Split from `run.rs` at the 1,200-line ceiling, and because these three share
//! one property that must not drift: each one either hands the tty away or
//! declares it gone, so each one has to agree with the others about what the
//! terminal's state currently is.
//!
//! - [`restart_reader`] re-points the reader after a handoff. It exists because
//!   `$EDITOR` and an interactive `!` command already carried the same twelve
//!   lines twice; a third copy for a suspend would be three places for one rule.
//! - [`hangup_requested`] is the **one** question the loop asks about a terminal
//!   that went away. The reader's own detection and the `SIGHUP` flag reach it,
//!   so a closed pty and a signal stop being two paths that could disagree.
//! - [`suspend`] is `Ctrl+Z`: restore the tty, stop the process group, restore
//!   again - or, where there is no job control, say so and touch nothing.

use lca_tui::engine::terminal::{InputHandler, ResizeHandler, Terminal};
use lca_tui::engine::{signals, signals::Suspended};

use crate::run::Screen;
use crate::run_early_input::EarlyStream;

/// Whether the terminal is gone, recording the answer on the terminal so every
/// later write is skipped (gh #403 gap 4).
///
/// Two routes, one question:
///
/// - the **reader** sets the terminal's own flag when `poll` reports a hangup or a
///   read returns end-of-file (a closed pty);
/// - the **`SIGHUP` handler** can only write an atomic, so it sets a process-wide
///   one and the loop copies it in here.
///
/// The copy is per-iteration rather than once, because a `SIGHUP` can arrive
/// between any two frames.
pub(crate) fn hangup_requested(terminal: &dyn Terminal) -> bool {
    let gone = hangup_decision(terminal.hangup(), signals::requested());
    if gone {
        terminal.note_hangup();
    }
    gone
}

/// The decision [`hangup_requested`] makes, as a **pure function of its two inputs**
/// (gh #403 gap 4).
///
/// Split out because the two inputs come from different places with different
/// lifetimes - a per-terminal flag the reader owns, and a process-wide one a signal
/// handler has no choice but to use - and a row that drove them through the real
/// cells would be asserting across tests in one process, which is precisely how a
/// suite becomes flaky. Taking them as parameters makes the rule assertable *and*
/// keeps the ordering honest: either direction alone is enough.
#[must_use]
pub(crate) fn hangup_decision(terminal_hangup: bool, signal_requested: bool) -> bool {
    terminal_hangup || signal_requested
}

/// Re-point the reader at the stream the loop actually drains, after a handoff
/// gave the terminal to a child.
///
/// The subtlety that made this a helper rather than an inline pair: with an
/// adopted start-up terminal the loop reads the **early** channel, not
/// `input_tx`, so pointing a restarted reader at `input_tx` leaves the session
/// deaf once the child exits (found as a 120 s hang in gh #121's e2e).
pub(crate) fn restart_reader(
    terminal: &mut dyn Terminal,
    input_tx: &std::sync::mpsc::Sender<String>,
    resize_tx: &std::sync::mpsc::Sender<()>,
    early_receiver: Option<&EarlyStream>,
) {
    let tx = early_receiver.map_or_else(|| input_tx.clone(), |stream| stream.sender.clone());
    let rtx = resize_tx.clone();
    terminal.start(
        Box::new(move |d| {
            let _ = tx.send(d);
        }) as InputHandler,
        Box::new(move || {
            let _ = rtx.send(());
        }) as ResizeHandler,
    );
}

/// What a suspend request should become, given whether this platform has job
/// control (gh #403 gap 4).
///
/// The flag is a **parameter** rather than a `cfg!` read inside, so the rule is
/// assertable on the machine most of this project is tested on - the same reason
/// `format_key_text_for` takes its `macos` flag. A row that could only run on
/// Windows would be a row nobody runs.
///
/// `None` means "do it"; `Some(notice)` means "say this and do nothing".
#[must_use]
pub(crate) fn suspend_refusal(supported: bool) -> Option<&'static str> {
    if supported {
        return None;
    }
    Some(
        "suspend is not supported on Windows: run LCA in its own window to keep it \
         out of the way",
    )
}

/// `Ctrl+Z`: hand the terminal back, stop the process group, and take the
/// terminal again once something continues us.
///
/// The order is the whole feature: a process that stops while still holding raw
/// mode leaves the shell above it unusable, and one that stops without writing
/// the bare exit leaves the frame on screen. So it is `leave` → `stop` →
/// `SIGTSTP` → `start` → `enter`, which is the `$EDITOR` sequence with a signal
/// where the child was.
///
/// Where there is no job control nothing is touched at all: tearing the frame
/// down to attempt a suspend that cannot happen would be a visible fault for no
/// gain, and silence is the fault this cycle removes - hence the notice.
pub(crate) fn suspend(
    screen: &mut Screen,
    terminal: &mut dyn Terminal,
    input_tx: &std::sync::mpsc::Sender<String>,
    resize_tx: &std::sync::mpsc::Sender<()>,
    early_receiver: Option<&EarlyStream>,
    supported: bool,
) -> Suspended {
    if suspend_refusal(supported).is_some() {
        return Suspended::Unsupported;
    }
    // A handoff, not an exit: `preserve = true` keeps the transcript on the
    // alt screen's own stack so the resume puts the same frame back.
    screen.leave(terminal, true);
    terminal.stop();
    let suspended = signals::suspend();
    if suspended == Suspended::Resumed {
        restart_reader(terminal, input_tx, resize_tx, early_receiver);
        screen.enter(terminal);
    }
    suspended
}

#[cfg(test)]
#[path = "run_terminal_tests.rs"]
mod tests;
