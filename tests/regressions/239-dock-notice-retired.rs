//! GitHub #239: the dock notice catch-all is retired — alerts route
//! to the transcript (pi's `showStatus`) or the top-right toast stack,
//! never above the editor.
//!
//! Verifies: each listed notice class lands in its new home, and the
//! dock carries no notice row.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::CommandEffect;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::{Chat, UiHooks, UiOptions};

fn options() -> UiOptions {
    UiOptions {
        prompt_slot: Default::default(),
        dialog_slot: Default::default(),
        pending_models: None,
        model_label: Arc::new(Mutex::new("p/m".into())),
        context_window: Arc::new(std::sync::Mutex::new(0)),
        thinking: Arc::new(Mutex::new(None)),
        theme: "auto".to_string(),
        theme_extra_dirs: Vec::new(),
        themes: lca_ui::theme::THEMES
            .iter()
            .map(|s| s.to_string())
            .collect(),
        initial_lines: Vec::new(),
        initial_header: (Vec::new(), Vec::new()),
        initial_records: Vec::new(),
        initial_tail_lines: Vec::new(),
        initial_messages: Vec::new(),
        open_resume_picker: false,
        yolo: false,
        early_input: None,
        thinking_visibility: Default::default(),
        codeblock_border: Default::default(),
        plain: true,
        invoke_command: Arc::new(|_, _| CommandEffect::None),
        slash_commands: vec!["/help".into(), "/model".into()],
        models: vec![lca_ui::ModelRow::from(("p/m".to_string(), "M".to_string()))],
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
        follow_up_mode: lca_protocol::QueueMode::OneAtATime,
    }
}

fn chat() -> Chat {
    Chat::new(options(), Arc::new(KeybindingsManager::new()))
}

// Verifies: gh #239 - a model switch confirmation lands in the
// transcript, not above the editor.
#[test]
fn model_switch_confirmation_lands_in_the_transcript() {
    let mut opts = options();
    opts.invoke_command =
        Arc::new(|_, _| CommandEffect::ShowWidget("model for this session: p/m".to_string()));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.model_picker.is_some(), "the picker opens");
    chat.handle_key("\r");
    assert!(
        chat.last_notice()
            .as_deref()
            .is_some_and(|n| n.contains("model for this session")),
        "the switch lands in history: {:?}",
        chat.last_notice()
    );
}

// Verifies: gh #239 - a compaction summary lands in the transcript;
// the working start stays out of history (the separator shows it).
#[test]
fn compaction_summary_lands_in_the_transcript() {
    let mut chat = chat();
    let state = Arc::new(Mutex::new(lca_ui::CompactState::Running(
        lca_ui::CompactionReason::Manual,
    )));
    let poll = state.clone();
    chat.world.options.hooks.poll_compact = Some(Arc::new(move || poll.lock().unwrap().clone()));
    assert!(chat.poll_compact(), "the start repaints");
    assert!(
        chat.last_notice().is_none() && chat.toasts.is_empty(),
        "the start stays out of history"
    );
    *state.lock().unwrap() = lca_ui::CompactState::Done("compacted: 42 tokens".to_string());
    assert!(chat.poll_compact(), "the summary repaints");
    assert!(
        chat.last_notice()
            .as_deref()
            .is_some_and(|n| n.contains("compacted: 42 tokens")),
        "the summary lands in history: {:?}",
        chat.last_notice()
    );
}

// Verifies: gh #239 - a host diagnostic floats as a toast, never in
// history, and fades on tick.
#[test]
fn host_diagnostics_float_and_fade() {
    let mut chat = chat();
    chat.on_turn_event(lca_protocol::TurnEvent::ExtensionEvent {
        extension: "host".into(),
        event: "cache-boundary-narrowed".into(),
        detail: "stable region diverged at message 1".into(),
    });
    assert!(
        chat.toasts
            .iter()
            .any(|toast| toast.text.contains("cache-boundary-narrowed")),
        "the diagnostic floats"
    );
    assert!(chat.last_notice().is_none(), "history stays clean");
    for toast in &mut chat.toasts {
        toast.until = std::time::Instant::now() - std::time::Duration::from_secs(1);
    }
    assert!(chat.tick(), "expiry repaints");
    assert!(chat.toasts.is_empty(), "faded toasts leave");
}

// Verifies: gh #239 - the dock carries no notice row: with history
// said and a toast afloat, no `• `-prefixed catch-all line renders.
#[test]
fn the_dock_carries_no_notice_row() {
    let mut chat = chat();
    chat.say("model for this session: p/m");
    chat.toast("✓ Copied to clipboard");
    let text = chat
        .viewport(100, 30, 0)
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !text.lines().any(|line| line.starts_with("• ")),
        "no catch-all row:\n{text}"
    );
    assert!(text.contains("model for this session"), "history renders");
    assert!(text.contains("Copied to clipboard"), "the toast renders");
}
