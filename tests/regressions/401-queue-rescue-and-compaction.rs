//! GitHub #401: three queue-UX gaps against pi's `interactive-mode.ts`.
//!
//! The abort rescue itself shipped with ADR-0038 (an aborted turn returns
//! every queued message to the editor); the issue asks for it to be
//! *pinned*, not rebuilt, and adds the two things around it that were
//! missing:
//!
//! 1. The restore announces itself and merges the live draft, so a user who
//!    kept typing while the turn ran keeps both - the queue first (it is
//!    older), the draft last, `\n\n` between, and the caret at the end.
//! 2. A submit during a compaction parks instead of being refused, and the
//!    parked messages flush when it ends: leading commands first, the first
//!    prompt starting the turn, the rest behind it, with the whole queue
//!    rolled back if no turn can start (pi's `restoreQueue`).
//! 3. The queue is discoverable: `Steering:` / `Follow-up:` labels and a
//!    `↳ <key> to edit all queued messages` hint whose key comes from the
//!    live binding registry, with `Alt+Up` (pi's key) doing the restore.
//!
//! This file is the end-to-end guard: it drives the real `Chat` through a
//! real rendered frame, so on the pre-fix tree every case here fails -
//! there was no compaction queue at all, the dock said `[steer]`/`[next]`
//! with a hardcoded `Alt+E`, and the restore neither announced nor merged.
//!
//! Verifies: NFR-24 (a shipped defect's guard), gh #401, FR-CORE-12,
//! ADR-0038 (annotated 2026-10-21 for the compaction queue).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::CommandEffect;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::{Chat, UiHooks, UiOptions};

fn chat() -> Chat {
    chat_with_bindings(Default::default())
}

/// A chat whose key overrides are `overrides`, for the hint rows.
fn chat_with_bindings(overrides: std::collections::BTreeMap<String, Vec<String>>) -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            dialog_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("p/m".into())),
            context_window: Arc::new(Mutex::new(0)),
            thinking: Arc::new(Mutex::new(None)),
            theme: "auto".to_string(),
            theme_extra_dirs: Vec::new(),
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
            early_input: None,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
            plain: true,
            invoke_command: Arc::new(|_, _| CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::from("."),
            keybinding_overrides: overrides.clone(),
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
            follow_up_mode: lca_protocol::QueueMode::OneAtATime,
        },
        Arc::new(KeybindingsManager::with_user_bindings(overrides)),
    )
}

fn frame(chat: &Chat) -> String {
    chat.render(100)
        .iter()
        .map(|line| strip_terminal_sequences(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Type a line into the editor and submit it.
fn submit(chat: &mut Chat, text: &str) {
    chat.editor.set_text(text);
    chat.handle_key("\r");
}

// Verifies: gh #401 item 1 + item 3 - the dock names each queued message the
// way pi does and hints the dequeue key the registry actually resolves, so a
// rebound dequeue cannot make the interface lie.
#[test]
fn the_queue_dock_speaks_pi_and_hints_the_live_dequeue_key() {
    let mut chat = chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit(
        "use nextest not cargo test".into(),
        lca_protocol::SubmitMode::Steer,
    );
    chat.queue_submit(
        "then update the changelog".into(),
        lca_protocol::SubmitMode::FollowUp,
    );
    let text = frame(&chat);
    assert!(
        text.contains("Steering: use nextest not cargo test"),
        "pi's steer label:\n{text}"
    );
    assert!(
        text.contains("Follow-up: then update the changelog"),
        "pi's follow-up label:\n{text}"
    );
    let key = chat.keybindings.primary_key("app.message.dequeue");
    assert_eq!(key, "alt+up", "pi's dequeue key is the primary binding");
    assert!(
        text.contains(&format!("↳ {key} to edit all queued messages")),
        "the hint names the resolved key:\n{text}"
    );
    // The hint is not a literal: a rebound dequeue prints the new key.
    let rebound_bindings = std::collections::BTreeMap::from([(
        "app.message.dequeue".to_string(),
        vec!["ctrl+g".to_string()],
    )]);
    let mut rebound = chat_with_bindings(rebound_bindings);
    rebound.begin_turn(lca_protocol::steer_queue());
    rebound.queue_submit("queued".into(), lca_protocol::SubmitMode::Steer);
    let text = frame(&rebound);
    assert!(
        text.contains("\u{21b3} ctrl+g to edit all queued messages"),
        "a rebound dequeue prints its new key:\n{text}"
    );
}

// Verifies: gh #401 item 1 - Escape's abort path restores the queue AND the
// draft the user typed meanwhile, queue first, and says how many came back.
// Removing `chat.restore_pending()` from `TurnState::reap` fails this.
#[test]
fn the_abort_rescue_merges_the_draft_behind_the_queue() {
    let mut chat = chat();
    chat.begin_turn(lca_protocol::steer_queue());
    submit(&mut chat, "wait, use nextest");
    chat.editor.set_text("actually never mind");
    let restored = chat.restore_pending();
    assert_eq!(restored, 1);
    assert_eq!(
        chat.editor.text(),
        "wait, use nextest\n\nactually never mind"
    );
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("Restored 1 queued message to editor"),
        "the rescue announces itself"
    );
    // Nothing was auto-sent: the user decides, with everything in the editor.
    assert_eq!(chat.take_submitted(), None);
}

// Verifies: gh #401 item 2 - typing during a compaction parks instead of
// being refused, and the flush moves the queue into the turn it belongs to.
#[test]
fn a_message_typed_during_compaction_survives_and_flushes() {
    let mut chat = chat();
    chat.compacting = true;
    submit(&mut chat, "after compaction please");
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("Queued message for after compaction"),
        "the park says when it will run"
    );
    assert!(frame(&chat).contains("Steering: after compaction please"));
    assert_eq!(chat.take_submitted(), None, "no turn under a compaction");
    chat.compacting = false;
    assert!(chat.flush_compaction_queue(), "the flush ran");
    assert_eq!(
        chat.take_submitted().as_deref(),
        Some("after compaction please"),
        "the parked message now starts its turn"
    );
}

// Verifies: gh #401 item 2 - a flush with no model puts the queue back
// whole (pi's `restoreQueue`), so a compaction ending during a broken login
// never eats the text.
#[test]
fn a_flush_that_cannot_start_a_turn_keeps_the_queue() {
    let mut chat = chat();
    *chat
        .world
        .options
        .model_label
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = String::new();
    chat.compacting = true;
    submit(&mut chat, "keep this");
    assert!(chat.flush_compaction_queue());
    assert_eq!(chat.compaction_queued.len(), 1, "still parked");
    assert_eq!(chat.compaction_queued[0].text, "keep this");
    assert_eq!(chat.take_submitted(), None);
    let text = frame(&chat);
    assert!(
        text.contains("Steering: keep this"),
        "still visible:\n{text}"
    );
}

// Verifies: gh #401 item 3 - `Alt+Up` takes the queue back with no picker
// open, which is the discoverable path the hint promises.
#[test]
fn alt_up_takes_the_queue_back() {
    let mut chat = chat();
    chat.begin_turn(lca_protocol::steer_queue());
    submit(&mut chat, "bring me back");
    chat.handle_key("\x1b[1;3A"); // Alt+Up
    assert!(chat.pending.is_empty());
    assert_eq!(chat.editor.text(), "bring me back");
}
