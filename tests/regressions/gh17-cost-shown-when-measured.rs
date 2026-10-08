//! GitHub issue #17 (open): "pi shows $0.000 unconditionally" - LCA hides
//! a zero cost (FR-UI-19's old reading), so a free model reads the same
//! as "no usage yet".
//!
//! Decision (composer-polish brief): show the cost once usage has been
//! measured - a measured `$0.0000` (a free model) is informative; silence
//! before any usage is noise and stays.
//!
//! Verifies: FR-UI-20 (the status area shows the session cost).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::{Chat, UiOptions};

fn chat() -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            dialog_slot: Default::default(),
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
            open_resume_picker: false,
            yolo: false,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
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

fn frame(chat: &Chat) -> String {
    chat.viewport(100, 30, 0)
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n")
}

// The policy: before any usage, the footer carries no cost segment -
// silence, not a zero.
#[test]
fn the_footer_hides_the_cost_before_any_usage() {
    let text = frame(&chat());
    assert!(
        !text.contains('$'),
        "no cost segment before the first usage-bearing turn:\n{text}"
    );
}

// A measured zero cost (a free model) is informative, so it shows.
#[test]
fn a_measured_zero_cost_is_shown() {
    let mut chat = chat();
    chat.on_turn_event(lca_protocol::TurnEvent::Usage(lca_protocol::Usage {
        input: 100,
        output: 10,
        ..Default::default()
    }));
    let text = frame(&chat);
    assert!(
        text.contains("$0.0000"),
        "a free model's measured zero cost reads as measured, not missing:\n{text}"
    );
}

// A real cost is unchanged by the policy.
#[test]
fn a_real_cost_is_shown_as_before() {
    let mut chat = chat();
    chat.on_turn_event(lca_protocol::TurnEvent::Usage(lca_protocol::Usage {
        input: 100,
        output: 10,
        cost: 0.0123,
        ..Default::default()
    }));
    let text = frame(&chat);
    assert!(
        text.contains("$0.0123"),
        "a priced turn keeps its cost:\n{text}"
    );
}
