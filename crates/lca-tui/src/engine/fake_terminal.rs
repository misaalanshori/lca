//! In-memory terminal for tests and golden harnesses (ceiling
//! split from `terminal.rs`): behavior unchanged, re-exported there so
//! existing `terminal::FakeTerminal` paths keep working.

use std::sync::{Arc, Mutex};

use super::program_status::{ProgramState, format_program_status, is_program_status_traffic};
use super::stdin_buffer::{StdinBuffer, StdinEvent};
use super::terminal::{InputHandler, ResizeHandler, Terminal};

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
                    // Gh #213: negotiation traffic never reaches the
                    // app, on the fake terminal exactly like the real
                    // reader's dispatch.
                    if is_program_status_traffic(&seq) {
                        continue;
                    }
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

    fn set_program_status(&mut self, state: ProgramState) {
        // The test double always reports: mode logic is covered
        // through the pure `program_status` predicates, so golden
        // harnesses assert exact bytes unconditionally.
        self.write(&format_program_status(state));
    }
}
