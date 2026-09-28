//! Driving the P6 interaction pack: a multi-line notice (from `/help`,
//! `/hotkeys`, or a command's block output) was pushed as one line string
//! containing embedded newlines. The renderer writes each line string
//! verbatim, so the embedded `\n` reached the terminal and corrupted the
//! screen. Every rendered line must be one visual line.
//!
//! Verifies: FR-UI-2.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::CommandEffect;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::{Chat, UiHooks, UiOptions};

fn chat() -> Chat {
    Chat::new(
        UiOptions {
            model_label: Arc::new(Mutex::new("p/m".into())),
            thinking: Arc::new(Mutex::new(None)),
            initial_lines: Vec::new(),
            plain: true,
            invoke_command: Arc::new(|_, _| CommandEffect::None),
            slash_commands: vec!["/help".into(), "/hotkeys".into()],
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
fn a_multiline_notice_renders_one_visual_line_per_line() {
    let mut chat = chat();
    chat.world.notice = Some("first line\nsecond line\nthird line".into());
    let lines = chat.render(60);
    assert!(
        lines.iter().all(|l| !l.contains('\n')),
        "a rendered line embedded a newline: {lines:?}"
    );
    let text = lines
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("first line"), "{text}");
    assert!(text.contains("second line"), "{text}");
    assert!(text.contains("third line"), "{text}");
}

#[test]
fn hotkeys_renders_without_embedded_newlines() {
    let mut chat = chat();
    for c in "/hotkeys".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let lines = chat.render(80);
    assert!(lines.iter().all(|l| !l.contains('\n')), "{lines:?}");
    let text = lines
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("keys:"), "{text}");
}
