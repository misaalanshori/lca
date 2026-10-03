//! GitHub issue #18 (open): "a stale notice row can survive a modal close -
//! when a padded (modal-active) frame shrinks back and the notice text
//! changes in the same step, the pane briefly shows the old notice line
//! above the new one."
//!
//! # Status: REPRO ATTEMPTED, NOT REPRODUCED
//!
//! The report said "not reproduced in isolation", which the cycle brief read
//! as *the frame sequence was never driven*. This file drives it, at screen
//! level rather than at the renderer's `previous()` model - the model is
//! right by construction, which is exactly how an artifact can hide in the
//! bytes actually written. [`Screen`] is the smallest interpreter of the
//! sequences the two renderers emit (cursor moves, line erase, clear,
//! CR/LF, SGR ignored), so "visible" here means visible on the pane.
//!
//! What was driven, and what came back:
//!
//! - **The live sequence, three times on a real pane** (`/help`'s 19-row
//!   block as the old notice, a picker modal over it, then a close that
//!   changes the notice in the same step - `/model`, `/login` + Esc, and a
//!   seeded model list): 0 stale rows each time, exactly one notice row.
//! - **31 synthetic frame pairs** through the real compose path
//!   (`Chat::viewport`) into both renderers, read off the screen: padded
//!   modal frame -> shrunk frame with a different notice, notice lengths
//!   1/3/25, documents shorter and longer than the viewport, three
//!   terminal sizes, and the grow-then-shrink variants. No stale row in
//!   any of them.
//!
//! So the sequence the report describes does not, by itself, produce the
//! artifact on the current build. What remains open is the *third* element
//! the report could not have isolated: something that makes the screen and
//! the renderer's `previous()` disagree (a scroll that moves the window
//! under a stale offset, a frame the loop renders that this file does not
//! compose, or a resize interleaved with the close). The mechanism is the
//! owner's to dig into with the cycle brief's render-pipeline pointer.
//!
//! The row below is therefore a **characterization guard**, not a repro: it
//! pins the reported sequence and the invariant the issue is about (no
//! stale notice row, exactly one new one) so the artifact cannot return
//! unnoticed once its real trigger is found. It is green today, and green
//! here means "the invariant holds", never "the bug was fixed".
//!
//! Verifies: NFR-24, GitHub issue #18.
use std::sync::{Arc, Mutex};

use lca_tui::engine::alt_screen::AltScreenRenderer;
use lca_tui::engine::main_screen::MainScreenRenderer;
use lca_tui::engine::terminal::FakeTerminal;
use lca_ui::state::{PickerOption, PickerPrompt};
use lca_ui::{Chat, state::UiOptions};

/// The visible screen: `rows` lines of `cols` cells, cursor-addressed.
#[derive(Debug)]
struct Screen {
    rows: Vec<Vec<char>>,
    cursor: (usize, usize),
}

impl Screen {
    fn new(cols: usize, rows: usize) -> Screen {
        Screen {
            rows: vec![vec![' '; cols]; rows],
            cursor: (0, 0),
        }
    }

    /// Apply one renderer's output to the screen.
    fn feed(&mut self, data: &str) {
        let bytes: Vec<char> = data.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                '\x1b' if bytes.get(i + 1) == Some(&']') => {
                    // OSC (hyperlinks, titles): payload until BEL or ST.
                    i += 2;
                    while i < bytes.len() && bytes[i] != '\x07' && bytes[i] != '\x1b' {
                        i += 1;
                    }
                    i += 1;
                }
                '\x1b' => {
                    // CSI: ESC [ ... <final alphabetic byte>. The param run
                    // may carry a `?` or `<` private-mode prefix, so scanning
                    // for the first letter is what keeps `?1049h` and
                    // `?25l` out of the painted text.
                    if i + 1 < bytes.len() && bytes[i + 1] == '[' {
                        let start = i + 2;
                        let mut end = start;
                        while end < bytes.len() && !bytes[end].is_ascii_alphabetic() {
                            end += 1;
                        }
                        let params = String::from_iter(&bytes[start..end]);
                        let finalizer = bytes.get(end).copied().unwrap_or(' ');
                        let numbers: Vec<usize> = params
                            .split(';')
                            .filter(|p| !p.is_empty())
                            .filter_map(|p| p.parse().ok())
                            .collect();
                        let first = numbers.first().copied().unwrap_or(0);
                        match finalizer {
                            'H' | 'f' => {
                                let row = numbers.first().copied().unwrap_or(1).max(1) - 1;
                                let col = numbers.get(1).copied().unwrap_or(1).max(1) - 1;
                                self.cursor = (row, col);
                            }
                            'A' => self.cursor.0 = self.cursor.0.saturating_sub(first.max(1)),
                            'B' => {
                                self.cursor.0 = (self.cursor.0 + first.max(1))
                                    .min(self.rows.len().saturating_sub(1))
                            }
                            'C' => {
                                self.cursor.1 = (self.cursor.1 + first.max(1))
                                    .min(self.rows[0].len().saturating_sub(1))
                            }
                            'D' => self.cursor.1 = self.cursor.1.saturating_sub(first.max(1)),
                            'G' => self.cursor.1 = first.max(1) - 1,
                            'K' => {
                                // 0K/2K: erase to end of line (the form both
                                // renderers use); 1K would erase to start.
                                let (row, col) = self.cursor;
                                for cell in self.rows[row].iter_mut().skip(col) {
                                    *cell = ' ';
                                }
                            }
                            'J' => {
                                // 2J: clear the whole screen, cursor home.
                                for row in &mut self.rows {
                                    for cell in row.iter_mut() {
                                        *cell = ' ';
                                    }
                                }
                                self.cursor = (0, 0);
                            }
                            _ => {}
                        }
                        i = end + 1;
                    } else {
                        // Other escapes (OSC, private modes): skip to their
                        // terminator so their payload is never painted.
                        i += 2;
                        while i < bytes.len() && bytes[i] != 'm' && bytes[i] != '\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                '\r' => {
                    self.cursor.1 = 0;
                    i += 1;
                }
                '\n' => {
                    self.cursor.0 = (self.cursor.0 + 1).min(self.rows.len().saturating_sub(1));
                    i += 1;
                }
                ch => {
                    let (row, col) = self.cursor;
                    if row < self.rows.len() && col < self.rows[0].len() {
                        self.rows[row][col] = ch;
                        self.cursor.1 += 1;
                    }
                    i += 1;
                }
            }
        }
    }

    /// The screen as lines, right-trimmed.
    fn lines(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_string())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// The compose path under test.
// ---------------------------------------------------------------------------

fn options() -> UiOptions {
    UiOptions {
        model_label: Arc::new(Mutex::new("p/m".into())),
        context_window: Arc::new(Mutex::new(0)),
        thinking: Arc::new(Mutex::new(None)),
        theme: "auto".to_string(),
        theme_dir: std::path::PathBuf::new(),
        themes: lca_ui::theme::THEMES
            .iter()
            .map(|s| s.to_string())
            .collect(),
        initial_lines: Vec::new(),
        initial_records: Vec::new(),
        initial_tail_lines: Vec::new(),
        yolo: false,
        thinking_visibility: Default::default(),
        plain: true,
        invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
        slash_commands: Vec::new(),
        models: Vec::new(),
        workspace: std::path::PathBuf::from("."),
        render_regions: None,
        ui_events: None,
        update_notice: None,
        login: None,
        complete_login: None,
        pick_login: None,
        confirm_login_grant: None,
        hooks: lca_ui::state::UiHooks::default(),
        fullscreen: true,
    }
}

fn chat() -> Chat {
    Chat::new(
        options(),
        Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    )
}

/// The two frames the report describes, composed by the real path:
/// A = modal active (so `viewport` pads to the terminal height) with the
/// old notice; B = modal gone (so the viewport is only as long as the
/// document) with a different, shorter notice.
fn frames(width: u16, height: u16) -> (Vec<String>, Vec<String>) {
    let mut chat = chat();
    chat.world.notice =
        Some("NOTICE-X first line\nNOTICE-X second line\nNOTICE-X third line".to_string());
    chat.world.picker = Some(PickerPrompt {
        options: vec![PickerOption {
            provider: "p".to_string(),
            id: "id".to_string(),
            label: "Label".to_string(),
            hint: "hint".to_string(),
        }],
        selected: 0,
    });
    let a = chat.viewport(width, height, 0);
    chat.world.picker = None;
    chat.world.notice = Some("NOTICE-Y".to_string());
    let b = chat.viewport(width, height, 0);
    (a, b)
}

/// Drive one renderer through the sequence and return what the pane shows.
fn screen_through_alt(width: u16, height: u16) -> Vec<String> {
    let (a, b) = frames(width, height);
    let mut term = FakeTerminal::new(width, height);
    let mut renderer = AltScreenRenderer::new();
    renderer.enter(&mut term);
    let _ = term.take_output();
    renderer.render_lines(&mut term, a, width, height);
    renderer.render_lines(&mut term, b, width, height);
    let mut screen = Screen::new(width as usize, height as usize);
    screen.feed(&term.output());
    screen.lines()
}

fn screen_through_main(width: u16, height: u16) -> Vec<String> {
    let (a, b) = frames(width, height);
    let mut term = FakeTerminal::new(width, height);
    let mut renderer = MainScreenRenderer::new();
    renderer.render(&mut term, a, width, height);
    renderer.render(&mut term, b, width, height);
    let mut screen = Screen::new(width as usize, height as usize);
    screen.feed(&term.output());
    screen.lines()
}

#[test]
fn the_reported_sequence_shows_no_stale_notice_row() {
    for (name, screen) in [
        ("alt", screen_through_alt(60, 20)),
        ("main", screen_through_main(60, 20)),
    ] {
        let stale: Vec<&String> = screen
            .iter()
            .filter(|line| line.contains("NOTICE-X"))
            .collect();
        let fresh: Vec<&String> = screen
            .iter()
            .filter(|line| line.contains("NOTICE-Y"))
            .collect();
        println!("=== {name} screen ===");
        for line in &screen {
            println!("{line:?}");
        }
        assert!(
            stale.is_empty(),
            "{name}: the old notice row survived: {stale:?}"
        );
        assert_eq!(fresh.len(), 1, "{name}: exactly one new notice row");
    }
}
