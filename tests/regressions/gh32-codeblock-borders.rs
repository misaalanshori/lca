//! gh #32: fenced code blocks could only be drawn four-sided, so every
//! mouse selection of copied code dragged `│ ` and ` │` along with it.
//! The config key reaches the transcript here: `full` keeps the shipped
//! frame (the no-regression half), `horizontal` drops the side pipes so
//! a selection pastes clean, and `none` draws nothing at all.
//!
//! Verifies: FR-UI-7 (the transcript renders markdown) for the frame's
//! three shapes.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_tui::widgets::markdown::CodeBlockBorder;
use lca_ui::{Chat, UiOptions};

const FENCE: &str = "```python\nprint(1)\nprint(2)\n```";

fn chat(border: CodeBlockBorder) -> Chat {
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
            codeblock_border: border,
            plain: true,
            invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::from("."),
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
            hooks: lca_ui::UiHooks::default(),
            fullscreen: true,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

fn rows(border: CodeBlockBorder) -> Vec<String> {
    let mut chat = chat(border);
    chat.on_turn_event(lca_protocol::TurnEvent::TextDelta(FENCE.into()));
    chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
        status: lca_protocol::TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
    });
    chat.render(100)
        .into_iter()
        .map(|row| strip_terminal_sequences(&row))
        .collect()
}

// The shipped shape, unchanged: frame corners and side pipes on every
// code line (this is what every existing markdown snapshot pins).
#[test]
fn full_keeps_the_shipped_frame() {
    let out = rows(CodeBlockBorder::Full);
    assert!(
        out.iter().any(|row| row.trim_start().starts_with('╭')),
        "the framed top row:\n{out:?}"
    );
    assert!(
        out.iter().any(|row| row.contains("│ print(1)")),
        "side pipes stay in `full`:\n{out:?}"
    );
    assert!(
        out.iter().any(|row| row.trim_start().starts_with('╰')),
        "the framed bottom row:\n{out:?}"
    );
}

// The issue's shape: bars top and bottom, code lines bare - a terminal
// selection pastes exactly what the fence wrote.
#[test]
fn horizontal_drops_the_side_pipes_and_keeps_the_bars() {
    let out = rows(CodeBlockBorder::Horizontal);
    let code: Vec<&str> = out
        .iter()
        .map(String::as_str)
        .filter(|row| row.contains("print("))
        .collect();
    assert_eq!(code.len(), 2, "both code lines render: {out:?}");
    for row in &code {
        assert!(
            !row.contains('│'),
            "no side pipes to clean up after a copy: {row:?}"
        );
        assert!(
            row.trim_start() == row.trim_start().trim_end(),
            "no padding to clean up either: {row:?}"
        );
    }
    assert!(
        out.iter()
            .any(|row| row.trim_start().starts_with("── python")),
        "the top bar carries the language: {out:?}"
    );
    // The bar below the code - the row right after the last code line,
    // so the transcript's own separator row cannot be mistaken for it.
    let last = out
        .iter()
        .position(|row| row.contains("print(2)"))
        .expect("the second code line");
    let below = out.get(last + 1).map(String::as_str).unwrap_or("");
    assert!(
        !below.trim().is_empty() && below.trim().chars().all(|c| c == '─'),
        "the bottom bar sits right under the code: {below:?} in {out:?}"
    );
    assert!(
        !out.iter().any(|row| row.contains('╭') || row.contains('╰')),
        "no frame corners either: {out:?}"
    );
}

// `none`: the fence's lines and nothing else.
#[test]
fn none_draws_nothing_but_the_code() {
    let out = rows(CodeBlockBorder::None);
    let code: Vec<&str> = out
        .iter()
        .map(String::as_str)
        .filter(|row| row.contains("print("))
        .collect();
    assert_eq!(code.len(), 2, "{out:?}");
    for row in &code {
        assert!(!row.contains('│') && !row.contains('─'), "{row:?}");
    }
    // No bar under the code: the row after the last code line is the
    // transcript's own spacing (its separator row further down is not
    // this block's doing).
    let last = out
        .iter()
        .position(|row| row.contains("print(2)"))
        .expect("the second code line");
    let below = out.get(last + 1).map(String::as_str).unwrap_or("─");
    assert!(
        below.trim().is_empty() || !below.trim().chars().all(|c| c == '─'),
        "no bar under the code: {below:?} in {out:?}"
    );
    assert!(
        !out.iter().any(|row| row.contains('╭') || row.contains('╰')),
        "no frame: {out:?}"
    );
}
