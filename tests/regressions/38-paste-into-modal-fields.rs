//! Released 0.5.2 defect (the owner's real-terminal report): paste worked in
//! the prompt editor but was dropped by every single-line surface — the
//! masked secret field, the base-URL and model-id fields, and the picker
//! search boxes all handled keys only. `stdin_buffer` parsed the bracketed
//! paste and the terminal layer handed it on, but `printable()` returned
//! `None` for it, so the bytes reached the modal and vanished.
//!
//! TUI cycle 7 R1 hoisted the editor's paste normalization (tmux CSI-u
//! decode, CRLF/tab normalization) into one shared primitive
//! (`lca_tui::widgets::paste`) and wired every surface to it; a single-line
//! field flattens newlines (pi's `input.ts` contract), while the editor
//! keeps them and turns a large paste into a marker.
//!
//! Verifies: NFR-24 (a released defect's guard), R1 (paste is a primitive
//! of every text input).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::CommandEffect;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::{Chat, LoginNext, UiHooks, UiOptions};

fn chat() -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            dialog_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("p/m".into())),
            context_window: Arc::new(Mutex::new(0)),
            thinking: Arc::new(Mutex::new(None)),
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
            yolo: false,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
            plain: true,
            invoke_command: Arc::new(|_, _| CommandEffect::None),
            slash_commands: vec!["/login".into()],
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
            fullscreen: false,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

/// One real bracketed-paste event, the shape the engine emits.
fn paste(text: &str) -> String {
    format!("\x1b[200~{text}\x1b[201~")
}

fn strip(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| strip_terminal_sequences(line))
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: the 0.5.2 defect is fixed — a paste into the masked secret field
// lands in the buffer and is masked, never dropped and never shown.
#[test]
fn a_paste_into_the_masked_secret_field_lands_and_masks() {
    let mut chat = chat();
    chat.apply_login_next(LoginNext::Secret {
        provider: "p".into(),
        label: "API key (input hidden)".into(),
        masked: true,
    });
    let secret = "sk-released-defect-value";
    chat.handle_key(&paste(secret));
    assert_eq!(
        chat.world
            .secret
            .as_ref()
            .expect("the prompt is open")
            .input,
        secret,
        "the paste reached the field (the 0.5.2 bug dropped it)"
    );
    let frame = strip(&chat.viewport(80, 24, 0));
    assert!(
        !frame.contains(secret),
        "the secret never reaches the frame:\n{frame}"
    );
    assert!(
        frame.contains(&"*".repeat(secret.chars().count())),
        "the pasted value is masked:\n{frame}"
    );
}

// Verifies: a multi-line paste into a single-line field is flattened (pi's
// `input.ts` contract), so it cannot break the field or submit mid-paste.
#[test]
fn a_multi_line_paste_into_a_single_line_field_flattens() {
    let mut chat = chat();
    chat.apply_login_next(LoginNext::Secret {
        provider: "p".into(),
        label: "Base URL".into(),
        masked: false,
    });
    chat.handle_key(&paste("https://api.example.com/v1\r\nsecond\ttab"));
    assert_eq!(
        chat.world
            .secret
            .as_ref()
            .expect("the prompt is open")
            .input,
        "https://api.example.com/v1second    tab"
    );
}
