//! Terminal I/O and protocol negotiation, ported from pi's
//! `packages/tui/src/terminal.ts`
//! (`pi-tui-re/src_re/tui-engine/terminal.md`).
//!
//! The [`Terminal`] trait is deliberately minimal and protocol-agnostic.
//! [`ProcessTerminal`] drives the real stdin/stdout; [`FakeTerminal`] backs
//! tests and the widget golden harnesses.
//!
//! Negotiation follows pi: one write pushes Kitty flags 7, queries them
//! (`CSI ?u`) and appends a Device-Attributes sentinel (`CSI c`). A terminal
//! that does not know Kitty answers DA first, which triggers the
//! `modifyOtherKeys` fallback with no startup timeout. Exit hygiene (disable
//! Kitty, drain stdin, pause before raw-off) protects the parent shell over
//! slow SSH.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::program_status::{
    PROGRAM_STATUS_QUERY, ProgramState, format_program_status, multiplexer_strips_osc,
    program_status_mode,
};
use super::sys;

/// Gh #522's reader-thread half, split for the line ceiling.
#[path = "reader_loop.rs"]
mod reader_loop;
#[cfg(test)]
pub(crate) use reader_loop::{apply_negotiation, dispatch};
pub(crate) use reader_loop::{program_status_active, reader_loop};

const KITTY_QUERY: &str = "\x1b[>7u\x1b[?u\x1b[c";
const PROGRESS_ACTIVE: &str = "\x1b]9;4;3\x07";
const PROGRESS_CLEAR: &str = "\x1b]9;4;0\x07";

/// How often a live `OSC 9;4;3` indicator is refreshed (gh #369).
///
/// Terminal emulators put a watchdog on `OSC 9;4` (roughly 10-15 s) so a
/// crashed CLI does not leave a spinner turning forever. A turn routinely
/// outlives that, so a single write at the start silently loses the
/// indicator mid-turn. pi refreshes every second
/// (`TERMINAL_PROGRESS_KEEPALIVE_MS`); so do we.
pub const PROGRESS_KEEPALIVE_MS: Duration = Duration::from_millis(1000);

/// Where the keepalive's bytes go. Separate from [`Terminal`] so the
/// cadence can be driven by a fake clock in a test rather than a sleeping
/// thread, and so the one place that decides *when* to write stays a pure
/// function of time.
pub trait ProgressGate {
    /// Emit `OSC 9;4;3` (indeterminate progress).
    fn write_progress_active(&self);
    /// Emit `OSC 9;4;0` (clear).
    fn write_progress_clear(&self);
}

/// The `OSC 9;4` keepalive's clock and arming state (gh #369).
///
/// Pure: no thread, no timer. [`Self::tick`] is called from the existing
/// tick seam (the UI loop's 50 ms `recv_timeout`), and it decides from the
/// caller-supplied `now` whether a refresh is due. That keeps the cadence
/// testable without sleeping and means the interface gains no new thread.
#[derive(Debug, Default)]
pub struct ProgressKeepalive {
    /// Whether the indicator is currently wanted.
    active: bool,
    /// When the last `OSC 9;4;3` went out (the arming write included).
    last_emit: Option<Instant>,
}

impl ProgressKeepalive {
    /// A keepalive that has never been armed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm or disarm the indicator. Writing `true` while already armed is
    /// deliberately not a re-emit: a second turn starting must not reset
    /// the clock, or a fast succession of turns would drift toward a burst.
    pub fn set_progress(&mut self, gate: &dyn ProgressGate, active: bool, now: Instant) {
        if active {
            if !self.active {
                self.active = true;
                gate.write_progress_active();
            }
            if self.last_emit.is_none() {
                self.last_emit = Some(now);
            }
        } else {
            if self.active {
                self.active = false;
                gate.write_progress_clear();
            }
            self.last_emit = None;
        }
    }

    /// Whether the indicator is currently armed.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Refresh the indicator when a full window has passed. A no-op while
    /// disarmed, which is what keeps an idle interface silent.
    pub fn tick(&mut self, gate: &dyn ProgressGate, now: Instant) {
        if !self.active {
            return;
        }
        let Some(last) = self.last_emit else {
            return;
        };
        if now.duration_since(last) >= PROGRESS_KEEPALIVE_MS {
            gate.write_progress_active();
            self.last_emit = Some(now);
        }
    }
}
const DEFAULT_SEQUENCE_TIMEOUT_MS: u64 = 50;
const DEFAULT_ESCAPE_TIMEOUT_MS: u64 = 10;
const DEFAULT_SSH_ESCAPE_TIMEOUT_MS: u64 = 100;

/// A boxed input callback.
pub type InputHandler = Box<dyn FnMut(String) + Send>;
/// A boxed resize callback.
pub type ResizeHandler = Box<dyn FnMut() + Send>;

/// Minimal terminal interface. Everything above this is protocol-agnostic.
pub trait Terminal: Send {
    /// Begin: raw mode, input/resize handling, protocol negotiation.
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler);
    /// Restore terminal state.
    fn stop(&mut self);
    /// Drain stdin before exiting (SSH key-release protection).
    fn drain_input(&mut self, max_ms: u64, idle_ms: u64);
    /// Write output bytes.
    fn write(&mut self, data: &str);
    /// Current column count.
    fn columns(&self) -> u16;
    /// Current row count.
    fn rows(&self) -> u16;
    /// Whether the Kitty keyboard protocol is active.
    fn kitty_protocol_active(&self) -> bool;

    /// This terminal's Kitty keyboard protocol cell, for a `KeybindingsManager`
    /// to share (gh #323 defect 2).
    ///
    /// **Defaulted to `None`**, so the change is additive: a terminal that does not
    /// offer its state (a fake, a test double) is simply matched against the ambient
    /// value, which is the pre-#323 behaviour. A real terminal returns the same
    /// `Arc` its reader thread stores into, so negotiation is visible to the matcher
    /// the instant it happens.
    fn kitty_state(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        None
    }

    /// Why raw mode could not be enabled, when it could not (gh #323 defect 3).
    ///
    /// **Defaulted to `None`** so every implementation and every caller compiles
    /// unchanged - this is additive. A terminal that refused the mode change is
    /// still usable in a degraded way (input arrives cooked, so the app must not
    /// promise raw-mode key handling), and the reason is the only actionable part.
    ///
    /// The message names the handle and carries the OS code; see
    /// `sys::set_mode_refusal`. A caller that ignores this gets exactly the old
    /// behaviour, which is why the fix is a *report* rather than a loud failure the
    /// interface layer cannot yet route.
    fn raw_mode_error(&self) -> Option<String> {
        None
    }
    /// Move the cursor up (negative) or down (positive).
    fn move_by(&mut self, lines: i32);
    /// Hide the cursor.
    fn hide_cursor(&mut self);
    /// Show the cursor.
    fn show_cursor(&mut self);
    /// Clear the current line.
    fn clear_line(&mut self);
    /// Clear from the cursor to the end of the screen.
    fn clear_from_cursor(&mut self);
    /// Clear the whole screen and home the cursor.
    fn clear_screen(&mut self);
    /// Set the window title.
    fn set_title(&mut self, title: &str);
    /// Drive the taskbar progress indicator.
    fn set_progress(&mut self, active: bool);

    /// Refresh the `OSC 9;4` indicator if its window has elapsed (gh #369).
    ///
    /// Called from the interface's existing tick seam. A terminal with no
    /// indicator armed does nothing; the default is a no-op so the fake
    /// terminal and any other implementation are unaffected.
    fn tick_progress(&mut self) {}
    /// Report a lifecycle state over OSC 7501 (gh #213): working,
    /// blocked, done, error, idle, and clear on shutdown. Reports only
    /// when the terminal opted in (see `program_status`); otherwise a
    /// silent no-op.
    fn set_program_status(&mut self, state: ProgramState);

    /// Whether the terminal has gone away (gh #403 gap 4).
    ///
    /// **Defaulted to `false`** so every implementation and every caller compiles
    /// unchanged: a fake terminal, an embedder's own double, and the pre-#403
    /// behaviour all answer "still here". A real terminal answers the flag its
    /// reader sets, so the loop learns about a closed pty or a `SIGHUP` through the
    /// same question.
    fn hangup(&self) -> bool {
        false
    }

    /// Record that the terminal is gone, from a path outside the reader.
    ///
    /// The `SIGHUP` handler cannot reach a `ProcessTerminal` (it may only touch
    /// atomics), so the loop copies [`crate::engine::signals::requested`] in here
    /// once per iteration. After this every write is skipped and `stop` restores
    /// nothing - the dispose-before-touch ordering gh #403 gap 4 is about.
    fn note_hangup(&self) {}
}

/// Read one process environment variable as an injected-`env` reader:
/// `program_status` takes `impl Fn`, and a bare function item satisfies
/// it without a closure at each call site.
fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Resolve the lone-ESC reassembly window: SSH-aware, env-overridable.
pub fn resolve_escape_timeout_ms() -> u64 {
    escape_timeout_for(
        std::env::var("PI_TUI_ESC_TIMEOUT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|ms| *ms > 0),
        std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some(),
    )
}

/// The timeout decision, extracted so the SSH branch is testable without
/// mutating the process environment (a shared-env test would race every
/// other env-sensitive test in the workspace).
///
/// pi's contract (io-reliability.md §3): 10 ms normally, 100 ms over SSH
/// where latency lets `ESC` + char straddle two packets, and an explicit
/// `PI_TUI_ESC_TIMEOUT`-style override for both.
fn escape_timeout_for(override_ms: Option<u64>, ssh: bool) -> u64 {
    if let Some(ms) = override_ms
        && ms > 0
    {
        return ms;
    }
    if ssh {
        return DEFAULT_SSH_ESCAPE_TIMEOUT_MS;
    }
    DEFAULT_ESCAPE_TIMEOUT_MS
}

/// The negotiation replies the terminal watches for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Negotiation {
    KittyFlags(u32),
    DeviceAttributes,
}

fn parse_negotiation(seq: &str) -> Option<Negotiation> {
    let bytes = seq.as_bytes();
    if !bytes.starts_with(b"\x1b[?") {
        return None;
    }
    // Kitty flags: ESC [ ? digits u
    if bytes.last() == Some(&b'u') {
        let body = &seq[3..seq.len() - 1];
        if !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit()) {
            return body.parse().ok().map(Negotiation::KittyFlags);
        }
        return None;
    }
    // Device attributes: ESC [ ? digits(;digits)* c
    if bytes.last() == Some(&b'c') {
        let body = &seq[3..seq.len() - 1];
        if !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit() || b == b';') {
            return Some(Negotiation::DeviceAttributes);
        }
    }
    None
}

/// Shared state the reader thread touches.
pub(crate) struct Shared {
    input: Mutex<InputHandler>,
    resize: Mutex<ResizeHandler>,
    /// This terminal's Kitty keyboard protocol state (gh #323 defect 2).
    ///
    /// An `Arc` rather than a plain `AtomicBool` because the key matcher needs to
    /// read the *same* cell: a `KeybindingsManager` holds a clone (see
    /// `set_kitty_state`), so negotiation on the reader thread is visible to the
    /// matcher with no notification and, crucially, without a process-wide flag.
    kitty: std::sync::Arc<AtomicBool>,
    modify_other_keys: AtomicBool,
    /// An OSC 7501 support reply arrived (gh #213): auto-mode
    /// reporting may flow.
    program_status: AtomicBool,
    cols: AtomicU16,
    rows: AtomicU16,
    shutdown: AtomicBool,
    /// Set by the reader thread as it leaves, so `stop` can retry the
    /// Windows synchronous-I/O cancel until the reader is actually gone.
    reader_done: AtomicBool,
    /// The terminal is **gone** (gh #403 gap 4): set by the reader when `poll`
    /// reports a hangup or a read returns end-of-file, and by the loop when a
    /// `SIGHUP` is recorded. Once set the process stops touching the tty - a write
    /// to a dead fd either blocks or raises again, and restoring modes on a closed
    /// terminal is a no-op at best.
    hangup: AtomicBool,
}

impl Shared {
    fn forward_input(&self, seq: String) {
        if let Ok(mut handler) = self.input.lock() {
            handler(seq);
        }
    }

    fn fire_resize(&self) {
        if let Ok(mut handler) = self.resize.lock() {
            handler();
        }
    }
}

/// Writes the `OSC 9;4` bytes the keepalive asks for, echoing them to the
/// write log when one is configured (gh #369: a capture must show the
/// keepalive, or the live-verification path could not prove it).
struct TerminalWriter {
    log: Option<PathBuf>,
}

impl TerminalWriter {
    fn emit(&self, data: &str) {
        write_stdout(data);
        if let Some(path) = &self.log
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        {
            let _ = f.write_all(data.as_bytes());
        }
    }
}

impl ProgressGate for TerminalWriter {
    fn write_progress_active(&self) {
        self.emit(PROGRESS_ACTIVE);
    }
    fn write_progress_clear(&self) {
        self.emit(PROGRESS_CLEAR);
    }
}

/// A real terminal over process stdin/stdout.
pub struct ProcessTerminal {
    shared: Arc<Shared>,
    raw_state: Option<sys::RawModeState>,
    /// Why raw mode was refused, if it was (gh #323 defect 3).
    raw_mode_error: Option<String>,
    reader: Option<JoinHandle<()>>,
    keyboard_protocol_pushed: bool,
    write_log: Option<PathBuf>,
    /// The `OSC 9;4` indicator's arming state and clock (gh #369). Uses the
    /// same `write_log`-backed writer as every other sequence, so a
    /// `LCA_TUI_WRITE_LOG` capture records the keepalive too.
    progress: ProgressKeepalive,
    /// The last non-`clear` badge state, re-emitted after a `start()`
    /// (gh #369: `$EDITOR` stops the terminal, which clears the badge; the
    /// resumed interface must restore it rather than leave the tab blank).
    program_status: Option<ProgramState>,
    /// Whether the alternate screen is active, folded from the `?1049h` and
    /// `?1049l` sequences that cross [`Terminal::write`]. The renderers
    /// drive those through the trait, so this tracks what the terminal is
    /// actually showing (gh #33).
    in_alt_screen: bool,
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessTerminal {
    /// Point resize notifications at a new handler, without restarting the
    /// reader (gh #400).
    ///
    /// `start` installs both handlers, but the startup path claims the
    /// terminal before the layout exists, so its resize handler has nowhere
    /// to send a size. Re-pointing afterwards lets the loop own resizes
    /// without a stop/start cycle — and a stop/start would drop keys, which is
    /// the very thing the early claim exists to prevent.
    pub fn set_resize_handler(&mut self, on_resize: ResizeHandler) {
        if let Ok(mut handler) = self.shared.resize.lock() {
            *handler = on_resize;
        }
    }

    /// Adopt the size the tty has **now**, and fire the resize handler if it moved
    /// (gh #522).
    ///
    /// A `stop`/`start` handoff (the external editor, an interactive `!` command, a
    /// suspend) gives the tty to a child and takes it back later. A resize that lands
    /// in that window is seen by nobody: the reader thread does not exist, and on
    /// restart it re-seeds its own `last_size` from the size it finds - so the new
    /// size is never *different* from anything, no resize fires, and the next frame
    /// is composed at the width the interface recorded **before** the handoff. Measured
    /// in a real pane: 140-column caps painted into a 60-column terminal, three trials
    /// of three.
    ///
    /// So the restart is where the frame's geometry is re-derived. The size is one fact
    /// with one owner (`shared.cols`/`rows`); the reader only mirrors it, which is what
    /// makes re-seeding it here correct rather than a second source.
    fn adopt_current_size(&self) {
        if let Ok(size) = sys::terminal_size() {
            self.adopt_size(size);
        }
    }

    /// [`Self::adopt_current_size`] with the size supplied, so the rule is assertable
    /// without a tty (the same shape `format_key_text_for` takes its `macos` flag).
    pub(crate) fn adopt_size(&self, size: (u16, u16)) {
        let moved = size.0 != self.shared.cols.load(Ordering::SeqCst)
            || size.1 != self.shared.rows.load(Ordering::SeqCst);
        if !moved {
            return;
        }
        self.shared.cols.store(size.0, Ordering::SeqCst);
        self.shared.rows.store(size.1, Ordering::SeqCst);
        self.shared.fire_resize();
    }

    /// Create a terminal bound to the process stdin/stdout.
    pub fn new() -> Self {
        let (cols, rows) = sys::terminal_size().unwrap_or((80, 24));
        Self {
            shared: Arc::new(Shared {
                input: Mutex::new(Box::new(|_| {})),
                resize: Mutex::new(Box::new(|| {})),
                kitty: std::sync::Arc::new(AtomicBool::new(false)),
                modify_other_keys: AtomicBool::new(false),
                program_status: AtomicBool::new(false),
                cols: AtomicU16::new(cols),
                rows: AtomicU16::new(rows),
                shutdown: AtomicBool::new(false),
                reader_done: AtomicBool::new(false),
                hangup: AtomicBool::new(false),
            }),
            raw_state: None,
            raw_mode_error: None,
            reader: None,
            keyboard_protocol_pushed: false,
            write_log: resolve_write_log(),
            progress: ProgressKeepalive::new(),
            program_status: None,
            in_alt_screen: false,
        }
    }

    /// A [`ProgressGate`] over this terminal's own writer.
    ///
    /// An owned handle rather than borrowing `self`, because
    /// [`ProgressKeepalive::set_progress`] takes `&mut self.progress` at the
    /// same time: two disjoint fields, which the borrow checker accepts only
    /// if the gate does not capture `self` wholesale.
    fn writer(&self) -> TerminalWriter {
        TerminalWriter {
            log: self.write_log.clone(),
        }
    }

    fn raw_write(&mut self, data: &str) {
        // Gh #403 gap 4: nothing leaves for a terminal that is gone. This is the
        // single gate every write passes (`write`, the OSC helpers, the renderers),
        // so the ordering is one check rather than a caller-by-caller rule.
        if self.shared.hangup.load(Ordering::SeqCst) {
            return;
        }
        let mut out = std::io::stdout();
        let _ = out.write_all(data.as_bytes());
        let _ = out.flush();
        if let Some(path) = &self.write_log
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        {
            let _ = f.write_all(data.as_bytes());
        }
    }

    fn disable_modify_other_keys(&mut self) {
        if !self.shared.modify_other_keys.load(Ordering::SeqCst) {
            return;
        }
        self.raw_write("\x1b[>4;0m");
        self.shared.modify_other_keys.store(false, Ordering::SeqCst);
    }
}

impl Terminal for ProcessTerminal {
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler) {
        if let Ok(mut h) = self.shared.input.lock() {
            *h = on_input;
        }
        if let Ok(mut h) = self.shared.resize.lock() {
            *h = on_resize;
        }
        if sys::stdin_is_tty() {
            // Gh #323 defect 3: a refusal used to vanish into `.ok()`, leaving the
            // app believing it was in raw mode while the console stayed cooked. The
            // mode is still not applied - there is nothing to apply - but the reason
            // is kept and exposed through [`Terminal::raw_mode_error`], and written
            // to the write log when one is configured so a capture shows it.
            match sys::enable_raw_mode() {
                Ok(state) => self.raw_state = Some(state),
                Err(err) => {
                    if let Some(path) = &self.write_log
                        && let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                    {
                        let _ = std::io::Write::write_all(
                            &mut f,
                            format!("raw-mode refused: {err}\n").as_bytes(),
                        );
                    }
                    self.raw_mode_error = Some(err.to_string());
                }
            }
        }
        // Gh #522: the tty may have changed size while a child owned it, and the
        // reader about to start seeds its own baseline from the size it finds - so
        // without this the change is invisible to everyone. Done after the handlers
        // above are installed, so the notification actually reaches the loop.
        self.adopt_current_size();
        // A previous `stop` set the shutdown flag; clear it so a restart
        // (the external editor) gets a live reader thread.
        self.shared.shutdown.store(false, Ordering::SeqCst);
        let shared = self.shared.clone();
        self.raw_write("\x1b[?2004h");
        let escape_timeout = resolve_escape_timeout_ms();
        let handle = std::thread::Builder::new()
            .name("lca-tui-input".into())
            .spawn(move || reader_loop(shared, escape_timeout))
            .ok();
        self.reader = handle;
        self.keyboard_protocol_pushed = true;
        let query = KITTY_QUERY.to_string();
        self.raw_write(&query);
        // Gh #213: ask about OSC 7501 beside the Kitty negotiation -
        // an unaware terminal ignores the unknown OSC. Skipped when
        // reporting is forced off, forced on (no detection needed), or
        // stripped by the multiplexer.
        // Gh #369: a suspend/resume (`$EDITOR`) `stop`s the terminal, which
        // writes `state=clear`; putting the remembered badge back means the
        // tab bar is not left blank for the rest of the session. pi does the
        // same in `start()` (`writeProgramStatus`).
        if let Some(state) = self.program_status {
            self.raw_write(&format_program_status(state));
        }
        if program_status_mode(env_var).needs_query() && !multiplexer_strips_osc(env_var) {
            self.raw_write(PROGRAM_STATUS_QUERY);
        }
    }

    fn stop(&mut self) {
        // Gh #403 gap 4: **dispose before touching the terminal.** Once the tty is
        // known to be gone, every sequence below is at best a no-op and at worst a
        // block or a second raise on a dead fd - and `restore_raw_mode` on a closed
        // terminal cannot do anything either. So the teardown is: stop the reader,
        // drop the raw-mode state without writing, and say nothing.
        //
        // The reader must still be joined: it is a thread, and leaving it parked is
        // the other half of the defect. It is already unblocked (its own flag is
        // set, and `poll` returns immediately on a hung-up fd).
        if self.shared.hangup.load(Ordering::SeqCst) {
            self.shared.shutdown.store(true, Ordering::SeqCst);
            if let Some(handle) = self.reader.take() {
                #[cfg(windows)]
                {
                    let deadline = Instant::now() + Duration::from_millis(500);
                    while !self.shared.reader_done.load(Ordering::SeqCst)
                        && Instant::now() < deadline
                    {
                        cancel_blocking_read(&handle);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
                let _ = handle.join();
            }
            // Dropped, not restored: there is no terminal to restore.
            self.raw_state = None;
            return;
        }
        // Gh #213: reset the tab badge when reporting flowed (a
        // no-op when it never did).
        self.set_program_status(ProgramState::Clear);
        // Gh #369: disarm through the keepalive, so a pending refresh clock
        // cannot outlive the interface and stray-write after exit.
        self.progress
            .set_progress(&self.writer(), false, Instant::now());
        self.raw_write("\x1b[?2004l");
        if self.keyboard_protocol_pushed || self.shared.kitty.load(Ordering::SeqCst) {
            self.raw_write("\x1b[<u");
            self.keyboard_protocol_pushed = false;
            // The terminal's own cell is the source (gh #323 defect 2); the
            // process-ambient value is only the fallback for a match made with no
            // terminal attached, so a real terminal must not write it - doing so is
            // how one pane's teardown used to deafen another's matcher.
            self.shared.kitty.store(false, Ordering::SeqCst);
        }
        self.disable_modify_other_keys();
        self.shared.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.reader.take() {
            #[cfg(windows)]
            {
                // One cancel can miss: the reader can pass its `shutdown`
                // check and slip into `ReadFile` between the flag and the
                // cancel, then block forever. Retry until it reports done.
                let deadline = Instant::now() + Duration::from_millis(500);
                while !self.shared.reader_done.load(Ordering::SeqCst) && Instant::now() < deadline {
                    cancel_blocking_read(&handle);
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            let _ = handle.join();
        }
        if let Some(state) = self.raw_state.take() {
            sys::restore_raw_mode(state);
        }
    }

    fn drain_input(&mut self, max_ms: u64, idle_ms: u64) {
        if self.keyboard_protocol_pushed || self.shared.kitty.load(Ordering::SeqCst) {
            self.raw_write("\x1b[<u");
            self.keyboard_protocol_pushed = false;
            self.shared.kitty.store(false, Ordering::SeqCst);
        }
        self.disable_modify_other_keys();
        let deadline = Instant::now() + Duration::from_millis(max_ms);
        let mut last = Instant::now();
        let mut buf = [0u8; 1024];
        while Instant::now() < deadline {
            let remaining = idle_ms.saturating_sub(last.elapsed().as_millis() as u64);
            if remaining == 0 {
                break;
            }
            match sys::wait_stdin(i32::try_from(remaining).unwrap_or(i32::MAX)) {
                Ok(outcome) if outcome.is_key() => {
                    let _ = sys::read_stdin(&mut buf);
                    last = Instant::now();
                }
                // Draining input during a shutdown: a resize is not input and
                // there is nothing left to repaint for.
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    fn write(&mut self, data: &str) {
        // gh #33: fold the renderers' own screen switches into the flag the
        // drop reads, so a completed teardown is distinguishable from a
        // crash that skipped it.
        if data.contains("\x1b[?1049h") {
            self.in_alt_screen = true;
        }
        if data.contains("\x1b[?1049l") {
            self.in_alt_screen = false;
        }
        self.raw_write(data);
    }

    fn columns(&self) -> u16 {
        self.shared.cols.load(Ordering::SeqCst)
    }

    fn rows(&self) -> u16 {
        self.shared.rows.load(Ordering::SeqCst)
    }

    fn kitty_protocol_active(&self) -> bool {
        self.shared.kitty.load(Ordering::SeqCst)
    }

    fn kitty_state(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        Some(self.shared.kitty.clone())
    }

    fn raw_mode_error(&self) -> Option<String> {
        self.raw_mode_error.clone()
    }

    fn hangup(&self) -> bool {
        self.shared.hangup.load(Ordering::SeqCst)
    }

    fn note_hangup(&self) {
        self.shared.hangup.store(true, Ordering::SeqCst);
    }

    fn move_by(&mut self, lines: i32) {
        if lines > 0 {
            self.raw_write(&format!("\x1b[{lines}B"));
        } else if lines < 0 {
            self.raw_write(&format!("\x1b[{}A", -lines));
        }
    }

    fn hide_cursor(&mut self) {
        self.raw_write("\x1b[?25l");
    }

    fn show_cursor(&mut self) {
        self.raw_write("\x1b[?25h");
    }

    fn clear_line(&mut self) {
        self.raw_write("\x1b[K");
    }

    fn clear_from_cursor(&mut self) {
        self.raw_write("\x1b[J");
    }

    fn clear_screen(&mut self) {
        self.raw_write("\x1b[2J\x1b[H");
    }

    fn set_title(&mut self, title: &str) {
        self.raw_write(&format!("\x1b]0;{title}\x07"));
    }

    fn set_progress(&mut self, active: bool) {
        self.progress
            .set_progress(&self.writer(), active, Instant::now());
    }

    fn tick_progress(&mut self) {
        self.progress.tick(&self.writer(), Instant::now());
    }

    fn set_program_status(&mut self, state: ProgramState) {
        // Gh #369: remember the last non-`clear` state so `start()` can put
        // it back after a suspend (the external editor), which `stop()`s and
        // therefore clears the badge.
        if state != ProgramState::Clear {
            self.program_status = Some(state);
        }
        if program_status_active(&self.shared) {
            self.raw_write(&format_program_status(state));
        }
    }
}

impl Drop for ProcessTerminal {
    /// A panic that unwinds past the interactive loop skips the renderer's
    /// `leave` and the explicit `stop`, leaving the terminal in the alt
    /// screen with mouse tracking on and raw mode set (pi's `uncaughtCrash`).
    /// Restore it defensively here; every write is harmless when the state
    /// is already restored, so the normal exit path only pays a few bytes.
    ///
    /// *"Harmless" stopped being true for `?1049l`* (gh #33): emitted when
    /// the alternate screen is already gone, it restores the cursor the
    /// switch *saved* - the top-left - and undoes the exit park, dropping
    /// the shell prompt back onto LCA's transcript. So it is gated on the
    /// flag `write` keeps; the mouse, wrap and cursor writes are neutral and
    /// still go out for the crash path.
    fn drop(&mut self) {
        self.stop();
        self.raw_write(&teardown_sequence(self.in_alt_screen));
    }
}

/// What the drop writes: mouse tracking off, the alternate screen left only
/// while it is still active, then wrap and cursor restored.
///
/// `?1004l` is here as well as in the alt screen's own `MOUSE_DISABLE` (gh #374
/// gap 2). The renderer's leave runs on an orderly exit; this sequence runs on a
/// **drop**, which is the crash and panic path, where the renderer never got to
/// write its own teardown. Focus reporting left on there would keep delivering
/// `ESC [ O` to a shell prompt that has no idea what it is.
fn teardown_sequence(in_alt_screen: bool) -> String {
    let mut out = String::from("\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l");
    if in_alt_screen {
        out.push_str("\x1b[?1049l");
    }
    out.push_str("\x1b[?7h\x1b[?25h");
    out
}

/// A panic-hook restore writer: stdout in production, a recorder in
/// tests. A plain function pointer so it lives in a `static`.
type PanicWriter = fn(&[u8]);

fn stdout_panic_writer(bytes: &[u8]) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

static PANIC_WRITER: Mutex<PanicWriter> = Mutex::new(stdout_panic_writer);
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);
static INSTALL_HOOK_ONCE: std::sync::Once = std::sync::Once::new();

/// The restore bytes the panic hook emits: leave the alternate screen,
/// mouse tracking off, wrap and cursor restored — the same sequence the
/// normal exit path writes, always assuming the worst case (#96). stdin
/// is deliberately not drained here: there is no event loop left to
/// protect from key-release, and a bounded-or-not read inside a crashing
/// process is where a fast abort goes to hang.
pub fn panic_restore_bytes() -> Vec<u8> {
    teardown_sequence(true).into_bytes()
}

/// Whether [`install_panic_hook`] has run in this process.
pub fn panic_hook_installed() -> bool {
    HOOK_INSTALLED.load(Ordering::SeqCst)
}

/// Install the crash restore: the hook writes [`panic_restore_bytes`]
/// and then runs the previously installed hook, so the default panic
/// report still prints. The runtime calls the hook before unwinding or
/// aborting, which is what makes this hold under release
/// `panic = "abort"` where `Drop` guards never run. Safe to call on
/// every entry path: the hook installs once, the flag records every
/// call.
pub fn install_panic_hook() {
    install_panic_hook_with(stdout_panic_writer);
}

/// [`install_panic_hook`] with an explicit writer — the seam the guard
/// drives with a recorder.
pub fn install_panic_hook_with(writer: PanicWriter) {
    *PANIC_WRITER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = writer;
    INSTALL_HOOK_ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let writer = *PANIC_WRITER
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            writer(&panic_restore_bytes());
            // gh #81: the file first (it is the artifact), then the
            // previous hook's console report.
            if let Some(context) = super::crash::context_clone() {
                super::crash::write_crash_log(&context, info);
            }
            previous(info);
        }));
    });
    HOOK_INSTALLED.store(true, Ordering::SeqCst);
}

fn resolve_write_log() -> Option<PathBuf> {
    let env = std::env::var_os("PI_TUI_WRITE_LOG")?;
    if env.is_empty() {
        return None;
    }
    let path = PathBuf::from(env);
    if path.is_dir() {
        let pid = std::process::id();
        Some(path.join(format!("lca-tui-{pid}.log")))
    } else {
        Some(path)
    }
}

/// Unblock a reader thread parked in a synchronous `ReadFile` on the
/// console handle, so it can observe `shutdown` and return. Without this
/// `stop()`'s join hangs forever: a console input handle can signal for
/// non-character events, so `wait_stdin` may report readable while
/// `ReadFile` still blocks.
#[cfg(windows)]
fn cancel_blocking_read(handle: &std::thread::JoinHandle<()>) {
    use std::os::windows::io::AsRawHandle;
    sys::cancel_blocking_read(handle.as_raw_handle());
}

fn write_stdout(data: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(data.as_bytes());
    let _ = out.flush();
}

pub use super::fake_terminal::FakeTerminal;

/// Whether the terminal forwards OSC 8 hyperlinks (pi's capability ladder,
/// `terminal-image.ts` `detectCapabilitiesFromEnvironment` +
/// `probeTmuxHyperlinks`, documented in `terminal-image.md` §1).
///
/// The ladder in pi's order: an env override, then the tmux client's own
/// `client_termfeatures` (tmux only re-emits OSC 8 when it lists
/// `hyperlinks`), then `screen` (never), then the known terminals, then
/// unknown = off - on a terminal that swallows OSC 8 the URL would vanish
/// from the rendered output, so the conservative answer prints
/// `text (url)` instead (`markdown.md` §6). Cached after the first call.
///
/// `LCA_HYPERLINKS=1|0` mirrors pi's `PI_HYPERLINKS`: it forces the answer,
/// which is how a receipt captures the OSC 8 bytes under tmux.
pub fn supports_hyperlinks() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| detect_hyperlinks(tmux_forwards_hyperlinks))
}

/// The ladder itself, with the tmux probe injected so tests do not need a
/// live tmux client.
fn detect_hyperlinks(tmux_probe: impl Fn() -> bool) -> bool {
    detect_hyperlinks_with(tmux_probe, |key| std::env::var(key).ok())
}

/// The ladder over an injected environment reader (tests pass a map;
/// `lca-tui` denies `unsafe`, so tests never touch process env).
fn detect_hyperlinks_with(
    tmux_probe: impl Fn() -> bool,
    env: impl Fn(&str) -> Option<String>,
) -> bool {
    // pi's PI_HYPERLINKS override: only `1` and `0` count.
    match env("LCA_HYPERLINKS").as_deref() {
        Some("1") => return true,
        Some("0") => return false,
        _ => {}
    }
    let term = env("TERM").unwrap_or_default().to_lowercase();
    let term_program = env("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    // pi: emit OSC 8 under tmux only when tmux confirms it forwards.
    if env("TMUX").is_some() || term.starts_with("tmux") {
        return tmux_probe();
    }
    // pi: screen does not forward OSC 8.
    if term.starts_with("screen") {
        return false;
    }
    if env("KITTY_WINDOW_ID").is_some() || term_program == "kitty" {
        return true;
    }
    if env("GHOSTTY_RESOURCES_DIR").is_some()
        || term_program == "ghostty"
        || term.contains("ghostty")
    {
        return true;
    }
    if env("WEZTERM_PANE").is_some() || term_program == "wezterm" {
        return true;
    }
    if env("WARP_SESSION_ID").is_some()
        || env("WARP_TERMINAL_SESSION_UUID").is_some()
        || term_program == "warpterminal"
    {
        return true;
    }
    if env("ITERM_SESSION_ID").is_some() || term_program == "iterm.app" {
        return true;
    }
    if env("WT_SESSION").is_some() {
        return true;
    }
    if matches!(term_program.as_str(), "alacritty" | "vscode" | "zed") {
        return true;
    }
    if env("TERMINAL_EMULATOR").is_some_and(|v| v.eq_ignore_ascii_case("jetbrains-jediterm")) {
        return false;
    }
    // Unknown (and pi's Windows-console branch): off, so a URL never
    // disappears from the screen.
    false
}

/// Whether this session is Apple's Terminal.app, which misrenders half-block
/// glyph art (gh #399).
///
/// pi's `isAppleTerminalSession` (`packages/tui/src/terminal.ts:44`) — the whole
/// rule is `platform == darwin && TERM_PROGRAM == Apple_Terminal`. Both halves
/// matter: some other program can set `TERM_PROGRAM`, and a Linux terminal can
/// claim the name, so neither half alone is the terminal pi measured. The pair
/// is what makes this a rule rather than a guess.
///
/// The symptom on that terminal is a gap between rows and half blocks that do
/// not meet their neighbours, which turns a two-row logo into something that
/// reads as broken art. Callers fall back to a wordmark ([`crate::engine`]'s
/// callers do); this function only answers the question.
///
/// `LCA_HALF_BLOCKS=1|0` forces the answer, matching `LCA_HYPERLINKS`'s shape:
/// it is how a receipt demonstrates the fallback on a machine that is not a Mac.
pub fn is_apple_terminal() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| {
        detect_apple_terminal(cfg!(target_os = "macos"), |key| std::env::var(key).ok())
    })
}

/// The rule over an injected platform flag and environment reader, so tests
/// never touch process env (`lca-tui` denies `unsafe`) and both halves of the
/// pair can be exercised on one machine: a Linux box claiming
/// `TERM_PROGRAM=Apple_Terminal` is not the terminal pi measured, and a Mac
/// without that variable is not either.
fn detect_apple_terminal(is_macos: bool, env: impl Fn(&str) -> Option<String>) -> bool {
    match env("LCA_HALF_BLOCKS").as_deref() {
        Some("0") => return true,
        Some("1") => return false,
        _ => {}
    }
    is_macos && env("TERM_PROGRAM").as_deref() == Some("Apple_Terminal")
}

/// pi's `probeTmuxHyperlinks`: ask the tmux client whether its
/// `client_termfeatures` lists `hyperlinks`. A client that does not answer
/// inside 250 ms (pi's timeout) is treated as not forwarding, so a hung
/// socket cannot stall the first render.
fn tmux_forwards_hyperlinks() -> bool {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    let mut child = match Command::new("tmux")
        .args(["display-message", "-p", "#{client_termfeatures}"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(_) => return false,
        }
    }
    let mut features = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut features);
    }
    features.split(',').any(|f| f.trim() == "hyperlinks")
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
