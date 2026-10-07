//! Fold-in defect from the composer-polish cycle (no issue number):
//! a slash command typed while a turn runs leaked to the model as text.
//! Reproduced live twice: `/compact` entered mid-turn was queued as a
//! steered message and the model received "/compact" as user text (and
//! helpfully wrote a summary - nonsense work).
//!
//! The rule: a slash command typed mid-turn dispatches as a command,
//! never as model text. Safe UI commands (`/help`) dispatch immediately;
//! turn-boundary commands (`/compact`) queue as pending commands and run
//! when the turn ends. The queue band shows a queued command as a
//! command, not a message.
//!
//! Verifies: FR-CORE-11 (a submitted message queues as a steer or a
//! follow-up) - a slash command is not a message, so it never queues as
//! one.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::{Chat, UiOptions};

fn chat(invoked: Arc<Mutex<Vec<(String, String)>>>) -> Chat {
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
            yolo: false,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
            plain: true,
            invoke_command: Arc::new(move |name: &str, argument: &str| {
                invoked
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((name.to_string(), argument.to_string()));
                lca_protocol::CommandEffect::None
            }),
            slash_commands: vec!["/help".into(), "/compact".into()],
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

fn invoked() -> Arc<Mutex<Vec<(String, String)>>> {
    Arc::new(Mutex::new(Vec::new()))
}

fn frame(chat: &Chat) -> String {
    chat.render(100)
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n")
}

// `/compact` typed mid-turn queues as a pending command: it reaches
// neither the running turn's steer queue nor the host dispatch yet, and
// the band shows it as a command.
#[test]
fn a_turn_boundary_command_queues_as_a_command_not_a_message() {
    let called = invoked();
    let mut chat = chat(called.clone());
    let steer = lca_protocol::steer_queue();
    chat.begin_turn(steer.clone());
    for c in "/compact".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(chat.pending.len(), 1, "one queued entry");
    assert!(
        chat.pending[0].is_command,
        "it is a command, not a message: {:?}",
        chat.pending[0]
    );
    assert_eq!(
        steer.lock().unwrap().len(),
        0,
        "the steer queue never sees command text"
    );
    assert!(
        called.lock().unwrap().is_empty(),
        "a turn-boundary command waits for turn end"
    );
    let text = frame(&chat);
    assert!(
        text.contains("[command]") && text.contains("/compact"),
        "the band shows a command:\n{text}"
    );
}

// At turn end the queued command runs through the host dispatch - and
// nothing resembling model text is submitted.
#[test]
fn a_queued_command_runs_at_turn_end_and_never_reaches_the_model() {
    let called = invoked();
    let mut chat = chat(called.clone());
    chat.begin_turn(lca_protocol::steer_queue());
    for c in "/compact".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(chat.take_next_pending(), None, "no model text is flushed");
    assert_eq!(
        *called.lock().unwrap(),
        vec![("compact".to_string(), String::new())],
        "the command ran through host dispatch"
    );
    assert_eq!(chat.take_submitted(), None, "nothing awaits the loop");
    assert!(
        !frame(&chat).contains("/compact"),
        "the transcript never shows it as a user message"
    );
}

// A queued command does not swallow the messages behind it: they still
// flush in order once the command has run.
#[test]
fn messages_behind_a_queued_command_still_flush_in_order() {
    let called = invoked();
    let mut chat = chat(called.clone());
    chat.begin_turn(lca_protocol::steer_queue());
    for c in "/compact".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.queue_submit("later".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(
        chat.take_next_pending().as_deref(),
        Some("later"),
        "the message behind the command still flushes"
    );
    assert_eq!(
        *called.lock().unwrap(),
        vec![("compact".to_string(), String::new())],
        "the command ran first"
    );
}

// `/help` typed mid-turn dispatches at once: the notice updates, nothing
// queues, and the model never hears about it.
#[test]
fn a_safe_command_dispatches_immediately_mid_turn() {
    let called = invoked();
    let mut chat = chat(called.clone());
    let steer = lca_protocol::steer_queue();
    chat.begin_turn(steer.clone());
    for c in "/help".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.world
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("commands:")),
        "the help notice landed at once: {:?}",
        chat.world.notice
    );
    assert!(chat.pending.is_empty(), "nothing queued");
    assert_eq!(steer.lock().unwrap().len(), 0, "nothing steered");
}
