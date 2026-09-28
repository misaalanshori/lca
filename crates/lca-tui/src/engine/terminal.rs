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

use super::keys::set_kitty_protocol_active;
use super::stdin_buffer::{StdinBuffer, StdinEvent};
use super::sys;

const KITTY_QUERY: &str = "\x1b[>7u\x1b[?u\x1b[c";
const PROGRESS_ACTIVE: &str = "\x1b]9;4;3\x07";
const PROGRESS_CLEAR: &str = "\x1b]9;4;0\x07";
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
}

/// Resolve the lone-ESC reassembly window: SSH-aware, env-overridable.
pub fn resolve_escape_timeout_ms() -> u64 {
    if let Ok(v) = std::env::var("PI_TUI_ESC_TIMEOUT")
        && let Ok(ms) = v.parse::<u64>()
        && ms > 0
    {
        return ms;
    }
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
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
struct Shared {
    input: Mutex<InputHandler>,
    resize: Mutex<ResizeHandler>,
    kitty: AtomicBool,
    modify_other_keys: AtomicBool,
    cols: AtomicU16,
    rows: AtomicU16,
    shutdown: AtomicBool,
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

/// A real terminal over process stdin/stdout.
pub struct ProcessTerminal {
    shared: Arc<Shared>,
    raw_state: Option<sys::RawModeState>,
    reader: Option<JoinHandle<()>>,
    keyboard_protocol_pushed: bool,
    write_log: Option<PathBuf>,
    progress: Arc<AtomicBool>,
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessTerminal {
    /// Create a terminal bound to the process stdin/stdout.
    pub fn new() -> Self {
        let (cols, rows) = sys::terminal_size().unwrap_or((80, 24));
        Self {
            shared: Arc::new(Shared {
                input: Mutex::new(Box::new(|_| {})),
                resize: Mutex::new(Box::new(|| {})),
                kitty: AtomicBool::new(false),
                modify_other_keys: AtomicBool::new(false),
                cols: AtomicU16::new(cols),
                rows: AtomicU16::new(rows),
                shutdown: AtomicBool::new(false),
            }),
            raw_state: None,
            reader: None,
            keyboard_protocol_pushed: false,
            write_log: resolve_write_log(),
            progress: Arc::new(AtomicBool::new(false)),
        }
    }

    fn raw_write(&mut self, data: &str) {
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
            self.raw_state = sys::enable_raw_mode().ok();
        }
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
    }

    fn stop(&mut self) {
        if self.progress.swap(false, Ordering::SeqCst) {
            self.raw_write(PROGRESS_CLEAR);
        }
        self.raw_write("\x1b[?2004l");
        if self.keyboard_protocol_pushed || self.shared.kitty.load(Ordering::SeqCst) {
            self.raw_write("\x1b[<u");
            self.keyboard_protocol_pushed = false;
            self.shared.kitty.store(false, Ordering::SeqCst);
            set_kitty_protocol_active(false);
        }
        self.disable_modify_other_keys();
        self.shared.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.reader.take() {
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
            set_kitty_protocol_active(false);
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
                Ok(true) => {
                    let _ = sys::read_stdin(&mut buf);
                    last = Instant::now();
                }
                Ok(false) => {}
                Err(_) => break,
            }
        }
    }

    fn write(&mut self, data: &str) {
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
        if active {
            self.raw_write(PROGRESS_ACTIVE);
            self.progress.store(true, Ordering::SeqCst);
        } else {
            self.progress.store(false, Ordering::SeqCst);
            self.raw_write(PROGRESS_CLEAR);
        }
    }
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

fn reader_loop(shared: Arc<Shared>, escape_timeout_ms: u64) {
    let mut buffer = StdinBuffer::with_timeouts(DEFAULT_SEQUENCE_TIMEOUT_MS, escape_timeout_ms);
    let mut last_data = Instant::now();
    let mut last_size = sys::terminal_size().unwrap_or((80, 24));
    let mut buf = [0u8; 4096];

    while !shared.shutdown.load(Ordering::SeqCst) {
        match sys::wait_stdin(50) {
            Ok(true) => match sys::read_stdin(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    last_data = Instant::now();
                    for event in buffer.process_bytes(&buf[..n]) {
                        dispatch(&shared, event);
                    }
                }
                Err(_) => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }

        if let Some(timeout) = buffer.pending_timeout_ms()
            && last_data.elapsed().as_millis() as u64 >= timeout
        {
            for event in buffer.flush() {
                dispatch(&shared, event);
            }
        }

        if let Ok(size) = sys::terminal_size()
            && size != last_size
        {
            last_size = size;
            shared.cols.store(size.0, Ordering::SeqCst);
            shared.rows.store(size.1, Ordering::SeqCst);
            shared.fire_resize();
        }
    }
}

fn dispatch(shared: &Shared, event: StdinEvent) {
    match event {
        StdinEvent::Data(seq) => {
            if apply_negotiation(shared, &seq) {
                return;
            }
            shared.forward_input(seq);
        }
        StdinEvent::Paste(content) => {
            shared.forward_input(format!("\x1b[200~{content}\x1b[201~"));
        }
    }
}

/// Apply a negotiation reply, writing the fallback enable sequence when the
/// terminal is not Kitty-capable. Returns whether the sequence was consumed.
fn apply_negotiation(shared: &Shared, seq: &str) -> bool {
    match parse_negotiation(seq) {
        Some(Negotiation::KittyFlags(flags)) => {
            if flags != 0 {
                if shared.modify_other_keys.swap(false, Ordering::SeqCst) {
                    write_stdout("\x1b[>4;0m");
                }
                if !shared.kitty.swap(true, Ordering::SeqCst) {
                    set_kitty_protocol_active(true);
                }
            } else if !shared.modify_other_keys.load(Ordering::SeqCst) {
                write_stdout("\x1b[>4;2m");
                shared.modify_other_keys.store(true, Ordering::SeqCst);
            }
            true
        }
        Some(Negotiation::DeviceAttributes) => {
            if !shared.kitty.load(Ordering::SeqCst)
                && !shared.modify_other_keys.load(Ordering::SeqCst)
            {
                write_stdout("\x1b[>4;2m");
                shared.modify_other_keys.store(true, Ordering::SeqCst);
            }
            true
        }
        None => false,
    }
}

fn write_stdout(data: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(data.as_bytes());
    let _ = out.flush();
}

/// In-memory terminal for tests and golden harnesses.
#[derive(Default)]
pub struct FakeTerminal {
    output: Arc<Mutex<String>>,
    cols: u16,
    rows: u16,
    kitty: bool,
    buffer: StdinBuffer,
    input: Option<InputHandler>,
    resize: Option<ResizeHandler>,
}

impl FakeTerminal {
    /// A fake terminal of the given size.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            ..Default::default()
        }
    }

    /// Feed raw input bytes as if typed.
    pub fn feed(&mut self, data: &str) {
        let events = self.buffer.process(data);
        for event in events {
            match event {
                StdinEvent::Data(seq) => {
                    if let Some(handler) = self.input.as_mut() {
                        handler(seq);
                    }
                }
                StdinEvent::Paste(content) => {
                    if let Some(handler) = self.input.as_mut() {
                        handler(format!("\x1b[200~{content}\x1b[201~"));
                    }
                }
            }
        }
    }

    /// Fire a resize event.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        if let Some(handler) = self.resize.as_mut() {
            handler();
        }
    }

    /// Take everything written so far.
    pub fn take_output(&self) -> String {
        let mut out = self.output.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut *out)
    }

    /// Peek at everything written so far.
    pub fn output(&self) -> String {
        self.output
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl Terminal for FakeTerminal {
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler) {
        self.input = Some(on_input);
        self.resize = Some(on_resize);
    }

    fn stop(&mut self) {}

    fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}

    fn write(&mut self, data: &str) {
        self.output
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push_str(data);
    }

    fn columns(&self) -> u16 {
        self.cols
    }

    fn rows(&self) -> u16 {
        self.rows
    }

    fn kitty_protocol_active(&self) -> bool {
        self.kitty
    }

    fn move_by(&mut self, lines: i32) {
        if lines > 0 {
            self.write(&format!("\x1b[{lines}B"));
        } else if lines < 0 {
            self.write(&format!("\x1b[{}A", -lines));
        }
    }

    fn hide_cursor(&mut self) {
        self.write("\x1b[?25l");
    }

    fn show_cursor(&mut self) {
        self.write("\x1b[?25h");
    }

    fn clear_line(&mut self) {
        self.write("\x1b[K");
    }

    fn clear_from_cursor(&mut self) {
        self.write("\x1b[J");
    }

    fn clear_screen(&mut self) {
        self.write("\x1b[2J\x1b[H");
    }

    fn set_title(&mut self, title: &str) {
        self.write(&format!("\x1b]0;{title}\x07"));
    }

    fn set_progress(&mut self, _active: bool) {}
}

/// Whether the terminal forwards OSC 8 hyperlinks (pi's capability ladder,
/// `terminal-image.md` §1).
///
/// tmux and screen do not forward OSC 8 by default, and an unknown terminal
/// is treated as not forwarding: on a terminal that swallows OSC 8 the URL
/// vanishes from the rendered output, so the conservative answer shows
/// `text (url)` instead (`markdown.md` §6). Cached after the first call.
pub fn supports_hyperlinks() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| {
        let term = std::env::var("TERM").unwrap_or_default();
        if std::env::var_os("TMUX").is_some()
            || std::env::var_os("STY").is_some()
            || term.starts_with("tmux")
            || term.starts_with("screen")
        {
            return false;
        }
        for var in [
            "KITTY_WINDOW_ID",
            "GHOSTTY_RESOURCES_DIR",
            "WEZTERM_PANE",
            "ITERM_SESSION_ID",
            "WT_SESSION",
            "ALACRITTY_SOCKET",
            "VSCODE_INJECTION",
            "ZED_TERM",
        ] {
            if std::env::var_os(var).is_some() {
                return true;
            }
        }
        // Unknown: off, so a URL never vanishes.
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_clears_the_shutdown_flag() {
        // The external editor stops and restarts the terminal; without a
        // reset the new reader thread sees the old shutdown flag and dies.
        let mut term = ProcessTerminal::new();
        term.start(Box::new(|_| {}), Box::new(|| {}));
        term.stop();
        assert!(term.shared.shutdown.load(Ordering::SeqCst));
        term.start(Box::new(|_| {}), Box::new(|| {}));
        assert!(!term.shared.shutdown.load(Ordering::SeqCst));
        term.stop();
    }

    #[test]
    fn negotiation_replies_parse() {
        assert_eq!(
            parse_negotiation("\x1b[?7u"),
            Some(Negotiation::KittyFlags(7))
        );
        assert_eq!(
            parse_negotiation("\x1b[?0u"),
            Some(Negotiation::KittyFlags(0))
        );
        assert_eq!(
            parse_negotiation("\x1b[?62;1;2c"),
            Some(Negotiation::DeviceAttributes)
        );
        assert_eq!(parse_negotiation("\x1b[A"), None);
    }

    #[test]
    fn fake_terminal_routes_input_and_output() {
        let mut term = FakeTerminal::new(80, 24);
        let got = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = got.clone();
        term.start(
            Box::new(move |s| sink.lock().unwrap().push(s)),
            Box::new(|| {}),
        );
        term.feed("\x1b[Ax");
        term.write("hello");
        assert_eq!(
            *got.lock().unwrap(),
            vec!["\x1b[A".to_string(), "x".to_string()]
        );
        assert_eq!(term.take_output(), "hello");
    }

    #[test]
    #[allow(unsafe_code)] // SAFETY: single-threaded test process; restored immediately
    fn escape_timeout_is_env_overridable() {
        unsafe { std::env::set_var("PI_TUI_ESC_TIMEOUT", "42") };
        assert_eq!(resolve_escape_timeout_ms(), 42);
        unsafe { std::env::remove_var("PI_TUI_ESC_TIMEOUT") };
    }
}
