//! GitHub issue #16 (open): "picker overlays anchor above tall notices
//! instead of above the composer" - with a tall notice up (e.g. after
//! `/help`), pickers stacked above the notice rather than sitting
//! directly above the composer.
//!
//! Decision (composer-polish brief): pickers anchor directly above the
//! composer (pi's bottom-anchored shape), overlaying the notice area
//! when present. One layout rule in the overlay composition; every
//! picker inherits it.

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
            slash_commands: vec!["/help".into(), "/model".into()],
            models: vec![
                ("alpha".to_string(), "alpha".to_string()),
                ("beta".to_string(), "beta".to_string()),
            ],
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

// With a tall notice up, the picker's box sits directly above the
// composer: its bottom border is the row before the editor's first row,
// so the notice area behind it is overlaid, not stacked under.
#[test]
fn the_picker_box_sits_directly_above_the_composer() {
    let mut chat = chat();
    for c in "/help".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.world
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("commands:")),
        "the tall notice is up: {:?}",
        chat.world.notice
    );
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.model_picker.is_some(), "the picker is open");

    let rows: Vec<String> = chat
        .viewport(100, 30, 0)
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect();
    assert_eq!(rows.len(), 30, "fullscreen sizes to the viewport");
    let bottom = rows
        .iter()
        .rposition(|l| l.contains('╰'))
        .expect("the picker box bottom border is on screen");
    let editor = rows
        .iter()
        .rposition(|l| l.starts_with("> "))
        .expect("the editor row is on screen");
    assert_eq!(
        bottom + 1,
        editor,
        "the picker sits directly above the composer (box bottom {bottom}, editor {editor}):\n{}",
        rows.join("\n")
    );
}
