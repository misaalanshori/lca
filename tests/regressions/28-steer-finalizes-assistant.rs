//! Audit defect: a steered message injected at the model-call boundary
//! pushed a user entry into the transcript without ending the assistant
//! entry before it, so the earlier assistant message kept its streaming
//! marker (`▍`) for the rest of the turn. A turn split by a steer has more
//! than one assistant entry; only the live one may stream.
//!
//! Verifies: FR-CORE-11.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::{CommandEffect, TurnEvent};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::{Chat, UiHooks, UiOptions};

fn chat() -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("p/m".into())),
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
            invoke_command: Arc::new(|_, _| CommandEffect::None),
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
            hooks: UiHooks::default(),
            fullscreen: true,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

#[test]
fn a_steer_finalizes_the_assistant_before_it() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta("first call".into()));
    chat.on_turn_event(TurnEvent::UserInjected {
        text: "steer".into(),
        mode: "steer".into(),
    });
    chat.on_turn_event(TurnEvent::TextDelta("second call".into()));
    let lines: Vec<String> = chat
        .render(60)
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect();
    let streaming = lines.iter().filter(|l| l.contains('▍')).count();
    assert_eq!(
        streaming,
        1,
        "only the live assistant streams:\n{}",
        lines.join("\n")
    );
}
