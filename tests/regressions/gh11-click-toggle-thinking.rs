//! GitHub issue #11 (open): "Reasoning runs toggle via keybinding only.
//! pi has per-row mouse hit-testing (MouseRegion); LCA's transcript has
//! no per-row hit testing in either renderer."
//!
//! The transcript records which rendered rows belong to a collapsible
//! reasoning run (the hit map); a click on a reasoning-run row in the
//! alt-screen renderer toggles that run, like Ctrl+T. Main-screen never
//! captures the mouse by design (gh35/gh33 contract) - the click half is
//! alt-screen only.
//!
//! Verifies: FR-UI-22 (the toggle acts on a run) for the click path.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_tui::engine::alt_screen::AltScreenRenderer;
use lca_tui::engine::keybindings::KeybindingsManager;
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
            initial_messages: Vec::new(),
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

/// A chat with one five-line reasoning run (the default snippet shows
/// three lines plus the `… +N lines` marker; the last two stay hidden
/// until the run expands).
fn thinking_chat() -> Chat {
    let mut chat = chat();
    chat.transcript.push_user("quux question");
    chat.on_turn_event(lca_protocol::TurnEvent::ReasoningDelta(
        "alpha\nbeta\ngamma\ndelta\nepsilon".into(),
    ));
    chat.on_turn_event(lca_protocol::TurnEvent::TextDelta("the answer".into()));
    chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
        status: lca_protocol::TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
    });
    chat
}

fn frame(chat: &Chat) -> Vec<String> {
    chat.viewport(80, 24, 0)
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect()
}

/// Feed one SGR click (press + release on the same cell) through the
/// alt-screen input path and return the completed click cell, if any.
fn click(renderer: &mut AltScreenRenderer, col: u16, row: u16) -> Option<(u16, u16)> {
    renderer.handle_input(&format!("\x1b[<0;{};{}M", col + 1, row + 1));
    renderer.handle_input(&format!("\x1b[<0;{};{}m", col + 1, row + 1));
    renderer.take_clicked_cell()
}

// Clicking a reasoning-run row toggles exactly that run: the hidden
// lines appear and nothing else moves.
#[test]
fn clicking_a_reasoning_row_toggles_that_run() {
    let mut chat = thinking_chat();
    let before = frame(&chat);
    assert!(
        !before.iter().any(|l| l.contains("delta")),
        "the tail hides before the click"
    );
    let row = before
        .iter()
        .position(|l| l.contains("alpha"))
        .expect("a visible reasoning row");
    let mut renderer = AltScreenRenderer::new();
    let cell = click(&mut renderer, 4, row as u16).expect("a completed click");
    assert_eq!(
        chat.click_at(cell.0, cell.1, 0, 80, 24),
        lca_ui::ClickOutcome::ThinkingToggled
    );
    let after = frame(&chat);
    assert!(
        after.iter().any(|l| l.contains("delta")) && after.iter().any(|l| l.contains("epsilon")),
        "the run expanded:\n{}",
        after.join("\n")
    );
}

// A click on a row that belongs to no reasoning run does nothing.
#[test]
fn clicking_a_non_run_row_does_nothing() {
    let mut chat = thinking_chat();
    let before = frame(&chat);
    let row = before
        .iter()
        .position(|l| l.contains("quux question"))
        .expect("the user row");
    let mut renderer = AltScreenRenderer::new();
    let cell = click(&mut renderer, 4, row as u16).expect("a completed click");
    assert_eq!(
        chat.click_at(cell.0, cell.1, 0, 80, 24),
        lca_ui::ClickOutcome::Ignored
    );
    assert_eq!(frame(&chat), before, "nothing moved");
}

// Main-screen never captures the mouse, so the click half is alt-screen
// only: the same click coordinates are refused when not fullscreen.
#[test]
fn the_click_half_is_alt_screen_only() {
    let mut chat = thinking_chat();
    chat.screen_mode = false;
    assert_eq!(
        chat.click_at(4, 2, 0, 80, 24),
        lca_ui::ClickOutcome::Ignored
    );
}

// The jump indicator is clickable with the same map: a click on its row
// while scrolled away asks for the live bottom.
#[test]
fn the_jump_indicator_is_clickable() {
    let mut chat = chat();
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    // Scroll away from the live bottom: the indicator's row is the
    // transcript window's last row.
    let scrolled: Vec<String> = chat
        .viewport(80, 24, 12)
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect();
    assert!(
        scrolled.iter().any(|l| l.contains("Jump to latest")),
        "scrolled away, the indicator shows"
    );
    let row = scrolled
        .iter()
        .position(|l| l.contains("Jump to latest"))
        .expect("the indicator row");
    let mut renderer = AltScreenRenderer::new();
    let cell = click(&mut renderer, 40, row as u16).expect("a completed click");
    assert_eq!(
        chat.click_at(cell.0, cell.1, 12, 80, 24),
        lca_ui::ClickOutcome::JumpBottom
    );
}
