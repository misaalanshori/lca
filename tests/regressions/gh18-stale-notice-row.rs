//! GitHub issue #18 (open): "a stale notice row can survive a modal close -
//! when a padded (modal-active) frame shrinks back and the notice text
//! changes in the same step, the pane briefly shows the old notice line
//! above the new one."
//!
//! # Status: fixed - the batch is ordered, both renderers (gh #18 fix-up)
//!
//! The first attempt asserted the **final screen state** after a state
//! transition, which is why it stayed clean: "briefly shows" is a
//! *transient*, and a transient lives in the write stream and in
//! intermediate tokens, not in an end state. This file now carries the
//! instrument for that (the steering note's three leads, in order):
//!
//! **Lead 1 - the raw write stream.** [`Screen::feed_with`] replays the
//! bytes token by token and inspects the screen after each one;
//! [`ChunkedTerminal`] keeps every renderer `write` as its own chunk so the
//! two levels can be told apart. Before the fix it found the artifact:
//! frame B painted the new notice's row before the old notice's
//! continuation rows were cleared, so a double-notice state existed inside
//! that one write. A terminal that honors `?2026` never painted it; one
//! that ignores or flattens synchronized output did - which is what the
//! original report saw.
//!
//! The order is now the fix, and it deliberately diverges from pi: pi's
//! diff loop (`packages/tui/src/tui-alt-screen.ts`, the `else` branch of
//! its redraw) emits `ESC[{row};1H` + `ESC[2K` + line per changed row in
//! one downward pass and relies on `?2026` to hide the intermediate. Both
//! LCA renderers now clear the rows a frame touches *before* painting any
//! of them, with the same rows, clears and text and only the order
//! changed - so no terminal, sync-honoring or not, shows the old notice
//! beside the new one. The rows below cite that baseline and this brief.
//!
//! The real-pane receipt (`tmux pipe-pane`, testing-plan §14) still
//! describes the shape: over `/help` -> `/login` -> Esc, 14051 raw bytes,
//! 14/14 batches sync-delimited, and the old notice's text is never
//! written again after the new notice's (offsets 4072 then 13755).
//!
//! **Lead 2 - two renders instead of one.** Refuted at the handler:
//! `Chat::handle_login_picker` does `self.world.picker.take()` and assigns
//! `login cancelled` in the same match arm, so one keystroke commits both
//! halves of the change into one state and one compose. The sequence is
//! swept anyway as three composes (A = modal + old notice, A-prime =
//! no modal + old notice, B = no modal + new notice) - the frame a
//! two-pass loop would show - and it is clean at both levels.
//!
//! **Lead 3 - a resize interleaved with the step.** Swept as frame A at
//! 60x20 (modal + old notice) followed by the new notice at 60x12, both
//! renderers: clean at both levels, and the final screen is stale-free.
//!
//! # What the rows below pin
//!
//! The contract, at the two levels that decide visibility: no double
//! notice at a write boundary, and no double notice inside a write that
//! is *not* synchronized. They are green today, and green here means "the
//! invariant holds on this build", never "the bug was fixed".
//!
//! # The bar these rows now hold
//!
//! Replaying a batch byte by byte, the state "old notice text visible AND
//! new notice text visible" never exists - not at a write boundary and not
//! mid-batch - through both renderers, for the reported pair, the
//! split-step triple, a tall pane, and a resize interleaved with the step.
//! Before the fix the row fails with `a write painted both notices
//! mid-batch (writes [2] of 3)`; after it, it passes.
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

    /// Apply one renderer's output to the screen, calling `after` once per
    /// parsed token. The callback is how a transient is caught: a state
    /// that exists only between two escape sequences is still a state the
    /// terminal passed through, and "briefly shows" is exactly that.
    fn feed_with(&mut self, data: &str, mut after: impl FnMut(&Screen)) {
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
                                // Clamped: a model of a real screen, which
                                // cannot park the cursor past its last row
                                // (a resize replay feeds a taller frame's
                                // rows through a shorter screen).
                                let row = numbers.first().copied().unwrap_or(1).max(1) - 1;
                                let col = numbers.get(1).copied().unwrap_or(1).max(1) - 1;
                                self.cursor = (
                                    row.min(self.rows.len().saturating_sub(1)),
                                    col.min(self.rows[0].len().saturating_sub(1)),
                                );
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
                                if let Some(cells) = self.rows.get_mut(row) {
                                    for cell in cells.iter_mut().skip(col) {
                                        *cell = ' ';
                                    }
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
            after(self);
        }
    }

    /// Apply one renderer's output without inspecting the intermediate
    /// states.
    fn feed(&mut self, data: &str) {
        self.feed_with(data, |_| {});
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
        prompt_slot: Default::default(),
        dialog_slot: Default::default(),
        pending_models: None,
        model_label: Arc::new(Mutex::new("p/m".into())),
        context_window: Arc::new(Mutex::new(0)),
        thinking: Arc::new(Mutex::new(None)),
        theme: "auto".to_string(),
        theme_extra_dirs: Vec::new(),
        themes: lca_ui::theme::THEMES
            .iter()
            .map(|s| s.to_string())
            .collect(),
        initial_lines: Vec::new(),
        initial_records: Vec::new(),
        initial_tail_lines: Vec::new(),
        initial_messages: Vec::new(),
        open_resume_picker: false,
        yolo: false,
        thinking_visibility: Default::default(),
        codeblock_border: Default::default(),
        plain: true,
        invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
        slash_commands: Vec::new(),
        models: Vec::new(),
        workspace: std::path::PathBuf::from("."),
        keybinding_overrides: Default::default(),
        keybinding_error: None,
        render_regions: None,
        ui_events: None,
        update_notice: None,
        login: None,
        complete_login: None,
        pick_login: None,
        confirm_login_grant: None,
        confirm_switch: None,
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

// ---------------------------------------------------------------------------
// Transient-level harness: what the *write stream* does, not what the last
// frame leaves behind. A transient is invisible to an end-state assertion by
// definition - this is the instrument the steering note asks for.
// ---------------------------------------------------------------------------

use lca_tui::engine::terminal::{InputHandler, ResizeHandler, Terminal};

/// A terminal that keeps every `write` as its own chunk, so a sequence can
/// be replayed one write at a time (and one token at a time inside it).
struct ChunkedTerminal {
    chunks: Vec<String>,
    cols: u16,
    rows: u16,
}

impl ChunkedTerminal {
    fn new(cols: u16, rows: u16) -> ChunkedTerminal {
        ChunkedTerminal {
            chunks: Vec::new(),
            cols,
            rows,
        }
    }
}

impl Terminal for ChunkedTerminal {
    fn start(&mut self, _on_input: InputHandler, _on_resize: ResizeHandler) {}
    fn stop(&mut self) {}
    fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
    fn write(&mut self, data: &str) {
        self.chunks.push(data.to_string());
    }
    fn columns(&self) -> u16 {
        self.cols
    }
    fn rows(&self) -> u16 {
        self.rows
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
}

/// One step of a sequence: a composed frame at a terminal size.
struct Step {
    lines: Vec<String>,
    width: u16,
    height: u16,
}

fn step(lines: Vec<String>, width: u16, height: u16) -> Step {
    Step {
        lines,
        width,
        height,
    }
}

/// Run `steps` through one renderer, returning every `write` it made.
fn drive_alt(steps: &[Step]) -> Vec<String> {
    let mut term = ChunkedTerminal::new(steps[0].width, steps[0].height);
    let mut renderer = AltScreenRenderer::new();
    renderer.enter(&mut term);
    for s in steps {
        term.cols = s.width;
        term.rows = s.height;
        renderer.render_lines(&mut term, s.lines.clone(), s.width, s.height);
    }
    term.chunks
}

fn drive_main(steps: &[Step]) -> Vec<String> {
    let mut term = ChunkedTerminal::new(steps[0].width, steps[0].height);
    let mut renderer = MainScreenRenderer::new();
    for s in steps {
        term.cols = s.width;
        term.rows = s.height;
        renderer.render(&mut term, s.lines.clone(), s.width, s.height);
    }
    term.chunks
}

/// Replay the whole stream and report the two levels that decide whether a
/// transient is *seen*:
///
/// - `visible`: a write **boundary** at which both notices are on screen -
///   what any terminal shows once `?2026` releases the batch. This is the
///   instrument the steering note asks for first.
/// - `uncovered`: a double-notice state *inside* a write - what a terminal
///   that ignores or flattens `?2026` paints row by row, and what the
///   original report saw. Both lists must be empty: that is the bar the
///   gh #18 fix-up sets. (The baseline - pi's diff loop, the `else` branch
///   of `tui-alt-screen.ts` - shares the intra-write states and relies on
///   synchronized output to hide them; LCA now orders the batch instead.)
///
/// Returns `(visible, uncovered, inspected_tokens)`.
fn double_notice_states(
    chunks: &[String],
    width: u16,
    height: u16,
    old: &str,
    new: &str,
) -> (Vec<usize>, Vec<usize>, usize) {
    let both = |state: &Screen| {
        let lines = state.lines();
        lines.iter().any(|l| l.contains(old)) && lines.iter().any(|l| l.contains(new))
    };
    let mut screen = Screen::new(width as usize, height as usize);
    let mut visible = Vec::new();
    let mut uncovered = Vec::new();
    let mut checked = 0usize;
    for (index, chunk) in chunks.iter().enumerate() {
        let mut inside = false;
        screen.feed_with(chunk, |state| {
            checked += 1;
            if both(state) {
                inside = true;
            }
        });
        if both(&screen) {
            visible.push(index);
        }
        if inside {
            uncovered.push(index);
        }
    }
    (visible, uncovered, checked)
}

const OLD_NOTICE: &str = "NOTICE-X";
const NEW_NOTICE: &str = "NOTICE-Y";

/// The old notice as a block, the new one as a single line - the shape the
/// report describes (a long notice replaced by a short one).
fn notice_block() -> String {
    "NOTICE-X first line\nNOTICE-X second line\nNOTICE-X third line".to_string()
}

/// The reported pair: a padded (modal-active) frame, then a shrunk one with
/// a different notice.
fn reported_pair(width: u16, height: u16) -> Vec<Step> {
    let mut chat = chat();
    chat.world.notice = Some(notice_block());
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
    chat.world.notice = Some(NEW_NOTICE.to_string());
    let b = chat.viewport(width, height, 0);
    vec![step(a, width, height), step(b, width, height)]
}

/// The split-step version: the handler may compose twice, so the pane can
/// legitimately hold a frame where the modal is gone but the notice is
/// still the old one (A-prime) before the notice changes (B).
fn split_step_sequence(width: u16, height: u16) -> Vec<Step> {
    let mut chat = chat();
    chat.world.notice = Some(notice_block());
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
    let a_prime = chat.viewport(width, height, 0);
    chat.world.notice = Some(NEW_NOTICE.to_string());
    let b = chat.viewport(width, height, 0);
    vec![
        step(a, width, height),
        step(a_prime, width, height),
        step(b, width, height),
    ]
}

// Verifies: gh #18, lead 1 - the transient never exists in the write
// stream. Every token of every frame is applied to a screen and the screen
// is inspected after each one, so a state that lives only between two
// escape sequences still counts. Covers the reported two-frame pair and
// the split-step triple, through both renderers.
#[test]
fn no_transient_double_notice_in_the_write_stream() {
    for (label, steps) in [
        ("reported pair", reported_pair(60, 20)),
        ("split-step triple", split_step_sequence(60, 20)),
        ("reported pair, tall pane", reported_pair(100, 30)),
    ] {
        for (mode, chunks) in [("alt", drive_alt(&steps)), ("main", drive_main(&steps))] {
            let (visible, uncovered, checked) =
                double_notice_states(&chunks, 60, 20, OLD_NOTICE, NEW_NOTICE);
            assert!(
                visible.is_empty(),
                "{label} ({mode}): a write boundary showed both notices (writes {visible:?} \
                 of {})",
                chunks.len()
            );
            assert!(
                uncovered.is_empty(),
                "{label} ({mode}): a write painted both notices mid-batch \
                 (writes {uncovered:?} of {})",
                chunks.len()
            );
            assert!(checked > 0, "{label} ({mode}): the stream was replayed");
        }
    }
}

// Verifies: gh #18, lead 3 - a terminal resize interleaved with the step.
// The frame arrives at a different height in the same sequence, which is
// the branch where the main renderer's height-change full clear runs.
#[test]
fn a_resize_interleaved_with_the_close_leaves_no_stale_row() {
    let wide = reported_pair(60, 20);
    let narrow = {
        let mut chat = chat();
        chat.world.notice = Some(NEW_NOTICE.to_string());
        vec![step(chat.viewport(60, 12, 0), 60, 12)]
    };
    let steps: Vec<Step> = wide.into_iter().chain(narrow).collect();

    for (mode, chunks) in [("alt", drive_alt(&steps)), ("main", drive_main(&steps))] {
        // The screen is modelled at the larger size for the replay (a real
        // resize keeps the buffer), then the final state is read at the
        // size the pane ends on.
        let (visible, uncovered, checked) =
            double_notice_states(&chunks, 60, 20, OLD_NOTICE, NEW_NOTICE);
        assert!(
            visible.is_empty(),
            "{mode}: resize interleaving left both notices at a write boundary ({visible:?})"
        );
        assert!(
            uncovered.is_empty(),
            "{mode}: resize interleaving painted both notices mid-batch \
             ({uncovered:?}) after {checked} tokens"
        );
        let mut final_screen = Screen::new(60, 12);
        final_screen.feed(&chunks.join(""));
        let final_lines = final_screen.lines();
        let stale: Vec<&String> = final_lines
            .iter()
            .filter(|l| l.contains(OLD_NOTICE))
            .collect();
        assert!(
            stale.is_empty(),
            "{mode}: stale row after the resize: {stale:?}"
        );
    }
}
