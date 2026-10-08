//! Gh #212: an open extension panel ate its own toggle, trapping `/exit`.
//!
//! With the panel open every key routed to the panel interactor, so the
//! `app.panel.toggle` binding never fired and `/exit` never reached the
//! editor. The toggle is a host binding now: it flips the panel from the
//! modal path too.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_tui::engine::keybindings::KeybindingsManager;
use lca_ui::{Chat, UiOptions};

fn chat() -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            dialog_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("p/m".to_string())),
            context_window: Arc::new(std::sync::Mutex::new(0)),
            thinking: Arc::new(std::sync::Mutex::new(None)),
            theme: "auto".to_string(),
            theme_dir: PathBuf::new(),
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
            workspace: PathBuf::new(),
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

// Verifies: gh #212 - the toggle closes an open panel instead of being
// eaten by it, so `/exit` can reach the editor afterwards.
#[test]
fn an_open_panel_does_not_eat_its_own_toggle() {
    use lca_ui::state::Action;
    let mut chat = chat();
    chat.world.panel_open = true;
    // `alt+x` is the `app.panel.toggle` default.
    assert_eq!(chat.handle_key("\x1bx"), Action::Continue);
    assert!(!chat.world.panel_open, "the toggle closed the panel");
    for c in "/exit".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Exit, "/exit quits");
}
