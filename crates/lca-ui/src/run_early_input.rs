//! Type-while-loading (gh #400, type-while-loading slice).
//!
//! # The defect, measured
//!
//! Starting `lca` runs a lot before the interactive loop exists: the grant
//! store, the merged config, the session resolve, then `Ui::new` loads the
//! extension registry (installed extensions, the project's own, the MCP plan,
//! native handles). The terminal only enters raw mode and starts its reader
//! thread inside `lca_ui::run`, *after* all of that. On this machine with three
//! extensions the window is ~200-300 ms; with a heavy project extension set it
//! grows with what is installed, which is exactly the wrong scaling — the users
//! with the most extensions get the worst startup.
//!
//! Two observed consequences, both reproducible in a real pane:
//!
//! - **Printed text happens to survive**, because the terminal's line
//!   discipline holds it in the tty buffer until raw mode drains it. It arrives
//!   late, in one lump, after the header — late, but not lost.
//! - **A quit key does not act.** Sent before raw mode, `Ctrl+C` is not
//!   interpreted by LCA at all: the shell's line discipline sees it and the
//!   process keeps starting. A user who wants out of a slow startup has to wait
//!   for it, then press it again. That is the real bug, and it is the one
//!   `FR-CORE-5`'s spirit names.
//!
//! # The fix
//!
//! Enter raw mode and start the reader **before** the expensive load, into a
//! buffer that outlives the sequencing, and hand that buffer to the loop. Not a
//! second terminal, not a second thread: [`EarlyInput`] owns one
//! `ProcessTerminal` for the whole startup, so there is no handover to get
//! wrong and no window where nobody is reading.
//!
//! # What the buffer must respect
//!
//! - **Order is preserved.** Keystrokes are replayed in arrival order. A version
//!   that lands the first three characters after the fourth is the same bug with
//!   extra steps.
//! - **Quit acts immediately.** `Ctrl+C` (and `Ctrl+D` with an empty editor)
//!   must not be buffered behind a load; [`EarlyInput::drain`] reports them, and
//!   the pre-loop phase acts on them at once.
//! - **Paste is one segment.** `ProcessTerminal`'s reader already assembles a
//!   bracketed paste into one `data` value (FR-UI-10), so buffering whole `data`
//!   values - never their characters - keeps a paste atomic for free.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use lca_tui::engine::terminal::{ProcessTerminal, Terminal};

/// Input captured before the interactive loop takes the terminal.
pub struct EarlyInput {
    /// The one terminal for the whole startup: raw mode and the reader thread
    /// are live from construction, and `lca_ui::run` adopts it rather than
    /// making a second one.
    pub terminal: ProcessTerminal,
    receiver: Receiver<String>,
    /// The sending half of `receiver`. The reader owns one clone; this one
    /// exists so a `$EDITOR` stop/start can re-point the reader at the same
    /// stream the loop is draining (gh #400) instead of the loop's unused one.
    sender: std::sync::mpsc::Sender<String>,
    /// A quit was seen. The startup phase stops at the next checkpoint.
    quit: bool,
    /// Everything else, in arrival order, for the loop to replay.
    pending: Vec<String>,
}

impl EarlyInput {
    /// Take the terminal now and begin buffering.
    ///
    /// `on_quit` is not a callback on purpose: a callback would have to be
    /// invoked from wherever the read lands, and the startup phase is not
    /// reentrant. The flag is polled at checkpoints instead, which is all a
    /// single-threaded startup needs.
    #[must_use]
    pub fn start() -> Self {
        let mut terminal = ProcessTerminal::new();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let tx_keep = tx.clone();
        terminal.start(
            Box::new(move |data| {
                let _ = tx.send(data);
            }),
            Box::new(|| {}),
        );
        Self {
            terminal,
            receiver: rx,
            sender: tx_keep,
            quit: false,
            pending: Vec::new(),
        }
    }

    /// Collect everything that has arrived so far, without blocking.
    ///
    /// Returns whether a quit was seen. Quit keys are removed from the pending
    /// stream: replaying them into a loop that is about to start would exit
    /// twice, and the phase that saw them has already acted.
    pub fn drain(&mut self) -> bool {
        while let Ok(data) = self.receiver.try_recv() {
            if is_quit(&data) {
                self.quit = true;
                continue;
            }
            self.pending.push(data);
        }
        self.quit
    }

    /// How many keystrokes are buffered right now, without draining them.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Whether a quit was already seen and acted on.
    #[must_use]
    pub fn quit(&self) -> bool {
        self.quit
    }

    /// Receive the next keystroke, blocking for at most `timeout`.
    ///
    /// The loop calls this instead of its own receiver when startup adopted a
    /// terminal: the reader thread that was started before the load is still
    /// filling this channel, so the loop reads the same stream the buffer read
    /// from - no handover, and no window where nobody is listening.
    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<String, std::sync::mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    /// Check for a quit without taking anything else, for a host that wants to
    /// stop its startup work early (gh #400).
    ///
    /// A user who presses `Ctrl+C` during a slow load asked to leave; the host
    /// calls this after the expensive step, and a `true` short-circuits the
    /// rest of startup. Non-blocking, so a checkpoint costs a `try_recv`.
    pub fn quit_requested(&mut self) -> bool {
        self.drain()
    }

    /// Take everything for the loop: the terminal, the receiver the early
    /// reader is still feeding, and the buffered input.
    ///
    /// The terminal is **not** restarted. Its reader thread has been running
    /// since [`EarlyInput::start`], and restarting it would drop keys that
    /// arrived between the last drain and the loop's first tick - the exact
    /// window this feature exists to close. The loop instead drains the same
    /// receiver, so the reader's output simply continues into it.
    ///
    /// Buffered input is returned rather than pushed through a callback: the
    /// loop owns the interpretation path, and replaying through *its* handler
    /// is what makes an early key mean exactly what a late one does.
    #[must_use]
    pub fn into_loop(mut self) -> (ProcessTerminal, EarlyStream, Vec<String>) {
        self.drain();
        (
            self.terminal,
            EarlyStream {
                sender: self.sender,
                receiver: self.receiver,
            },
            self.pending,
        )
    }
}

/// Whether `data` is a quit key: `Ctrl+C`, or `Ctrl+D`.
///
/// Both are checked here rather than in the loop because startup has no editor
/// to be empty or not: during load there is nothing to delete forward, so
/// `Ctrl+D` means exit, exactly as it does at an at-rest prompt.
///
/// **Three spellings, not one.** The raw control byte is only the first:
///
/// - `\u{3}` / `\u{4}` — the plain C0 byte (a terminal without the keyboard
///   protocol, and every test).
/// - `\x1b[99;5u` / `\x1b[100;5u` — Kitty's CSI-u, which the terminal starts
///   sending the moment `start()` negotiates the protocol. This is what tmux
///   actually delivers, and missing it was a live-only bug: the buffer held the
///   sequence, no quit was recognised, and a user's `Ctrl+C` during load did
///   nothing.
/// - `\x1b[27;5;99~` / `\x1b[27;5;100~` — xterm's modifyOtherKeys spelling of
///   the same two keys.
///
/// The keybindings engine already treats all three as `ctrl+c`
/// (`keys.rs`, `ctrl_letters_use_control_codes`); this is the same table, kept
/// beside the buffer because startup has no keybindings manager yet.
#[must_use]
pub fn is_quit(data: &str) -> bool {
    matches!(
        data,
        "\u{3}" | "\u{4}" | "\x1b[99;5u" | "\x1b[100;5u" | "\x1b[27;5;99~" | "\x1b[27;5;100~"
    )
}

/// A shared handle the startup phase checks between steps.
pub type QuitFlag = Arc<std::sync::atomic::AtomicBool>;

#[cfg(test)]
#[path = "early_input_tests.rs"]
mod tests;

/// The two halves of the stream an adopted terminal feeds.
///
/// Both halves travel together because the loop needs the receiver to read and
/// the sender to re-point a restarted reader (`$EDITOR` stops the terminal and
/// starts it again). Splitting them is what made the editor path hang: the
/// restart fed a channel nobody was reading.
pub struct EarlyStream {
    /// Feeds the reader this loop drains.
    pub sender: std::sync::mpsc::Sender<String>,
    /// What the loop drains.
    pub receiver: Receiver<String>,
}

impl EarlyStream {
    /// The next keystroke, blocking for at most `timeout`.
    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<String, std::sync::mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

/// The terminal the loop drives, plus whatever startup buffered for it.
pub struct AdoptedTerminal {
    /// The one terminal for the whole session.
    pub terminal: ProcessTerminal,
    /// The receiver to read keystrokes from. `Some` means startup claimed the
    /// terminal early and this is its stream; `None` means the loop made its
    /// own and this is the loop's own channel.
    pub receiver: Option<EarlyStream>,
    /// Keys that arrived before the loop existed, in arrival order.
    pub startup_keys: Vec<String>,
}

impl AdoptedTerminal {
    /// Take the terminal for the loop.
    ///
    /// When startup claimed it early, its reader thread is already live and
    /// filling a channel - the loop reads *that* channel, so there is no
    /// handover and no window where nobody is listening. Otherwise the loop
    /// starts its own terminal, exactly as before.
    ///
    /// `resize` is where resize notifications go; an adopted terminal is
    /// re-pointed at it, because it was started before any layout existed.
    #[must_use]
    pub fn take(
        early: Option<EarlyInput>,
        input_tx: &std::sync::mpsc::Sender<String>,
        resize_tx: &std::sync::mpsc::Sender<()>,
    ) -> Self {
        match early {
            Some(early) => {
                let (mut terminal, stream, startup_keys) = early.into_loop();
                let rtx = resize_tx.clone();
                terminal.set_resize_handler(Box::new(move || {
                    let _ = rtx.send(());
                }));
                Self {
                    terminal,
                    receiver: Some(stream),
                    startup_keys,
                }
            }
            None => {
                let mut terminal = ProcessTerminal::new();
                let itx = input_tx.clone();
                let rtx = resize_tx.clone();
                terminal.start(
                    Box::new(move |data| {
                        let _ = itx.send(data);
                    }),
                    Box::new(move || {
                        let _ = rtx.send(());
                    }),
                );
                Self {
                    terminal,
                    receiver: None,
                    startup_keys: Vec::new(),
                }
            }
        }
    }
}
