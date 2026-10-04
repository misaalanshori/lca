//! gh #35: in fullscreen, scrolling up scrolled the whole screen - the
//! prompt editor, separator, and footer walked off the bottom with the
//! transcript. The frame now splits into a fixed dock and a transcript
//! window that alone slices by scroll, with a virtual scrollbar on the
//! window's margin and a jump-to-bottom indicator while scrolled away.
//!
//! The hard rule lives here end to end: a selection across a row that
//! carries the scrollbar copies exactly the transcript's text.
//!
//! Verifies: FR-UI-20 (the status line follows the live state) for the
//! dock half, FR-UI-24 (the main-screen contract) by staying out of it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::selection::{Granularity, Selection, SelectionPoint};
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::{Chat, UiOptions};

fn chat() -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("openai-compatible/m".to_string())),
            context_window: Arc::new(std::sync::Mutex::new(0)),
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
            codeblock_border: Default::default(),
            plain: true,
            invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::from("."),
            render_regions: None,
            ui_events: None,
            update_notice: None,
            login: None,
            complete_login: None,
            pick_login: None,
            confirm_login_grant: None,
            hooks: lca_ui::UiHooks::default(),
            fullscreen: true,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

/// A fullscreen chat with a transcript long enough to scroll.
fn long_chat() -> Chat {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    chat
}

const W: u16 = 80;
const H: u16 = 24;

fn strip(row: &str) -> String {
    strip_terminal_sequences(row)
}

// The symptom the issue reported: the editor and footer hold their rows
// whatever the scroll, so the dock never scrolls away.
#[test]
fn the_dock_pins_while_the_transcript_scrolls() {
    let chat = long_chat();
    let at_bottom = chat.viewport(W, H, 0);
    let scrolled = chat.viewport(W, H, 18);
    assert_eq!(scrolled.len(), H as usize, "the frame fills the viewport");
    let editor = |rows: &[String]| {
        rows.iter()
            .rposition(|row| strip(row).trim_start().starts_with("> "))
    };
    let bottom_editor = editor(&at_bottom).expect("editor at the bottom");
    let scrolled_editor = editor(&scrolled).expect("editor while scrolled");
    assert_eq!(
        &at_bottom[bottom_editor..],
        &scrolled[scrolled_editor..],
        "the dock's rows are byte-identical at both scrolls"
    );
}

// The scrollbar marks the transcript window and nothing else, and hides
// itself when the transcript fits - the row fact the selection clamp
// below relies on.
#[test]
fn the_scrollbar_marks_the_window_and_hides_when_it_fits() {
    let frame = long_chat();
    let geometry = frame.scrollbar_for_frame(W, H, 0).expect("overflowing");
    let rows = frame.viewport(W, H, 0);
    for row in rows.iter().take(geometry.rows as usize) {
        let row = strip(row);
        assert!(row.ends_with('│') || row.ends_with('┃'), "{row:?}");
    }
    for row in rows.iter().skip(geometry.rows as usize) {
        let row = strip(row);
        assert!(!row.ends_with('│') && !row.ends_with('┃'), "{row:?}");
    }

    let mut short = chat();
    short.screen_mode = true;
    short.transcript.push_user("one line");
    assert!(
        short.scrollbar_for_frame(W, H, 0).is_none(),
        "no transcript to position a thumb in"
    );
}

// The indicator shows exactly while scrolled away from the live bottom
// and names the key that returns there.
#[test]
fn the_indicator_shows_while_scrolled_and_hides_at_the_bottom() {
    let chat = long_chat();
    let text = |scroll: u16| {
        chat.viewport(W, H, scroll)
            .iter()
            .map(|row| strip(row))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(!text(0).contains("Jump to latest message"), "at the bottom");
    let scrolled = text(14);
    assert!(
        scrolled.contains("Jump to latest message"),
        "while scrolled:\n{scrolled}"
    );
    let key = lca_tui::engine::keybindings::key_text("tui.altScreen.bottom");
    assert!(
        scrolled.contains(&key),
        "named for the key that returns: {scrolled}"
    );
}

// The hard rule, end to end: the row carries the scrollbar, the
// selection is dragged across it, and the copy holds only the text -
// the same pair of facts the engine row pins, proven here over the rows
// the viewport actually paints.
#[test]
fn selecting_a_scrollbar_row_copies_exactly_the_text() {
    let chat = long_chat();
    let geometry = chat.scrollbar_for_frame(W, H, 0).expect("overflowing");
    let rows = chat.viewport(W, H, 0);
    let row = rows
        .iter()
        .take(geometry.rows as usize)
        .position(|row| {
            let row = strip(row);
            row.trim().len() > 1 && (row.ends_with('│') || row.ends_with('┃'))
        })
        .expect("a content row that carries the bar");

    let mut selection = Selection::new();
    selection.start(
        SelectionPoint {
            row: row as u16,
            col: 0,
        },
        Granularity::Char,
        1,
        &rows[row],
    );
    // Dragged to the frame's right edge, bar included.
    selection.update(
        SelectionPoint {
            row: row as u16,
            col: W,
        },
        &rows[row],
    );
    selection.end();
    selection.set_scrollbar(Some((geometry.column, geometry.rows)));

    let text = selection.active_text(&rows);
    assert!(!text.is_empty(), "there was text to select");
    assert!(
        !text.ends_with('│') && !text.ends_with('┃'),
        "the scrollbar never reaches the copy: {text:?}"
    );
    let painted = strip(&rows[row]);
    let content = painted
        .trim_end()
        .trim_end_matches('│')
        .trim_end_matches('┃')
        .trim_end()
        .to_string();
    assert_eq!(
        text, content,
        "the copy is exactly the row's text, bar and padding aside"
    );
}
