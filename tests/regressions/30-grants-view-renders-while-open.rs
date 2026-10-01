//! Cycle-4 defect: the `/grants` picker (S8) opened and handled keys, but
//! `Chat::viewport`'s resize guard listed every picker except the new one,
//! so the overlay was composited past the viewport it had not grown. A
//! picker that is open must reserve the height its overlay needs.
//!
//! Verifies: S8 (the grants view), FR-UI-3 (overlays composite over the
//! viewport).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::CommandEffect;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_ui::state::{Action, GrantEntry};
use lca_ui::{Chat, UiHooks, UiOptions};

fn chat(grants: Vec<GrantEntry>) -> Chat {
    let list = Arc::new(move || grants.clone());
    Chat::new(
        UiOptions {
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
            yolo: false,
            thinking_visibility: Default::default(),
            plain: true,
            invoke_command: Arc::new(|_, _| CommandEffect::None),
            slash_commands: vec!["/grants".into()],
            models: Vec::new(),
            workspace: PathBuf::from("."),
            render_regions: None,
            ui_events: None,
            update_notice: None,
            login: None,
            complete_login: None,
            pick_login: None,
            confirm_login_grant: None,
            hooks: UiHooks {
                grants: Some(list),
                ..UiHooks::default()
            },
            fullscreen: true,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

fn entry(install_consent: bool, subject: &str, detail: &str) -> GrantEntry {
    GrantEntry {
        install_consent,
        subject: subject.to_string(),
        detail: detail.to_string(),
        revocable: !install_consent,
    }
}

#[test]
fn the_grants_overlay_renders_while_the_picker_is_open() {
    let mut chat = chat(vec![
        entry(true, "openai-compatible", "enabled"),
        entry(false, "ad hoc", "echo tool-done"),
    ]);
    for byte in "/grants".bytes() {
        chat.handle_key(&(byte as char).to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    let framed = chat.viewport(100, 24, 0).join("\n");
    assert!(
        framed.contains("Grants for this project"),
        "the overlay is inside the viewport it reserved:\n{framed}"
    );
    assert!(framed.contains("Install consent"), "{framed}");
    assert!(framed.contains("echo tool-done"), "{framed}");
}
