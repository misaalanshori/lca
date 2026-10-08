//! R11 defect: a keypress that only cancels the permission countdown
//! (FR-UI-18) rebuilt the modal with `respond: None`, orphaning the worker
//! waiting for the decision, so every later Allow/Deny was a no-op. A key
//! that is not a decision must keep the responder; only a decision closes
//! the modal.
//!
//! Verifies: FR-UI-18.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::CommandEffect;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_ui::state::PermissionModal;
use lca_ui::{Chat, UiHooks, UiOptions};

fn chat() -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            dialog_slot: Default::default(),
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
            open_resume_picker: false,
            yolo: false,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
            plain: true,
            invoke_command: Arc::new(|_, _| CommandEffect::None),
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
            hooks: UiHooks::default(),
            fullscreen: true,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

#[test]
fn cancelling_the_countdown_keeps_the_responder() {
    let mut chat = chat();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    chat.world.permission = Some(PermissionModal {
        action: "rm -rf /tmp/x".into(),
        respond: Some(tx),
        deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
    });
    // A key that is not a decision only cancels the countdown.
    chat.handle_key("x");
    assert!(chat.world.permission.is_some(), "the modal stays open");
    // Deny still reaches the waiting worker.
    chat.handle_key("d");
    assert!(chat.world.permission.is_none());
    assert_eq!(
        rx.try_recv().ok(),
        Some(lca_permissions::Decision::Denied),
        "the decision reached the worker"
    );
}
