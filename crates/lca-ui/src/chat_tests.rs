use super::*;
use lca_protocol::{ToolCall, ToolResult};
use lca_tui::engine::text::strip_terminal_sequences;
use std::path::PathBuf;

fn options() -> UiOptions {
    UiOptions {
        model_label: Arc::new(std::sync::Mutex::new("p/m".into())),
        thinking: Arc::new(std::sync::Mutex::new(None)),
        initial_lines: Vec::new(),
        plain: true,
        invoke_command: Arc::new(|_, _| CommandEffect::None),
        slash_commands: vec!["/help".into(), "/model".into(), "/login".into()],
        models: vec!["alpha".into(), "beta".into()],
        workspace: PathBuf::from("."),
        render_regions: None,
        ui_events: None,
        update_notice: None,
        login: None,
        complete_login: None,
        pick_login: None,
        confirm_login_grant: None,
        hooks: crate::state::UiHooks::default(),
        fullscreen: true,
    }
}

fn chat() -> Chat {
    Chat::new(options(), Arc::new(KeybindingsManager::new()))
}

fn strip(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| strip_terminal_sequences(l)).collect()
}

#[test]
fn typing_edits_and_enter_submits() {
    let mut chat = chat();
    for c in "hello".chars() {
        assert_eq!(chat.handle_key(&c.to_string()), Action::Continue);
    }
    assert_eq!(chat.editor.text(), "hello");
    assert_eq!(chat.handle_key("\r"), Action::Submit);
    assert_eq!(chat.take_submitted().as_deref(), Some("hello"));
    assert_eq!(chat.editor.text(), "");
}

#[test]
fn a_slash_command_dispatches_without_submitting() {
    let mut chat = chat();
    for c in "/help".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert!(chat.world.notice.as_deref().unwrap().contains("/model"));
    assert!(chat.take_submitted().is_none());
}

// Verifies: FR-UI-9 - Tab completion covers slash commands, command
// arguments, and file paths, and shows the candidates in a menu.
#[test]
fn tab_applies_the_completion_popup() {
    let mut chat = chat();
    for c in "/mo".chars() {
        chat.handle_key(&c.to_string());
    }
    assert!(chat.editor.suggestions().is_some());
    chat.handle_key("\t");
    assert_eq!(chat.editor.text(), "/model ");
}

#[test]
fn argument_completion_offers_models() {
    let mut chat = chat();
    for c in "/model al".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\t");
    assert_eq!(chat.editor.text(), "/model alpha");
}

#[test]
fn escape_cancels_a_running_turn() {
    let mut chat = chat();
    chat.turn_running = true;
    assert_eq!(chat.handle_key("\x1b"), Action::CancelTurn);
}

#[test]
fn ctrl_c_needs_two_taps_to_exit() {
    let mut chat = chat();
    assert_eq!(chat.handle_key("\x03"), Action::Continue);
    assert_eq!(chat.handle_key("\x03"), Action::Exit);
}

#[test]
fn reasoning_and_answer_are_separated() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ReasoningDelta("thinking".into()));
    chat.on_turn_event(TurnEvent::TextDelta("the answer".into()));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    let text = strip(&chat.render(60)).join("\n");
    assert!(text.contains("thinking"));
    assert!(text.contains("the answer"));
    assert!(!text.contains("thinkingthe answer"));
}

#[test]
fn a_finished_tool_names_the_tool_not_the_call_id() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "call-abc123".into(),
        name: "read".into(),
        arguments: "{\"path\":\"a.rs\"}".into(),
    }));
    chat.on_turn_event(TurnEvent::ToolFinished(ToolResult::ok(
        "call-abc123",
        "body",
    )));
    let text = strip(&chat.render(80)).join("\n");
    assert!(text.contains("read"));
    assert!(!text.contains("abc123"));
}

#[test]
fn a_large_bracketed_paste_becomes_a_marker() {
    let mut chat = chat();
    let big: String = (0..12)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    chat.handle_key(&format!("\x1b[200~{big}\x1b[201~"));
    assert_eq!(chat.editor.text(), "[paste #1 +12 lines]");
}

#[test]
fn modals_composite_over_the_viewport() {
    let mut chat = chat();
    chat.world.show_permission("rm -rf /tmp/x".into());
    let viewport = chat.viewport(80, 24, 0);
    assert_eq!(viewport.len(), 24);
    let text = strip(&viewport).join("\n");
    assert!(text.contains("rm -rf /tmp/x"));
}

// Verifies: FR-CORE-11 - a message submitted while a turn runs queues
// (steer) instead of starting a second turn, and shows in the band.
#[test]
fn steering_queues_while_a_turn_runs() {
    let mut chat = chat();
    let steer = lca_protocol::steer_queue();
    chat.begin_turn(steer.clone());
    for c in "mid-turn note".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert_eq!(chat.pending.len(), 1);
    assert_eq!(chat.pending[0].mode, lca_protocol::SubmitMode::Steer);
    assert_eq!(
        steer.lock().unwrap().len(),
        1,
        "the steer reached the running turn's boundary queue"
    );
    let text = strip(&chat.render(80)).join("\n");
    assert!(text.contains("mid-turn note"), "the pending band shows it");
}

// Verifies: FR-CORE-12 - an aborted turn returns its queue to the editor.
#[test]
fn abort_restores_pending_to_the_editor() {
    let mut chat = chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("one".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_submit("two".into(), lca_protocol::SubmitMode::FollowUp);
    chat.restore_pending();
    assert!(chat.pending.is_empty());
    assert_eq!(chat.editor.text(), "one\ntwo");
}

// Verifies: FR-CORE-11 - follow-ups auto-run in order at turn end.
#[test]
fn follow_ups_run_in_order() {
    let mut chat = chat();
    chat.queue_submit("first".into(), lca_protocol::SubmitMode::FollowUp);
    chat.queue_submit("second".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(chat.take_next_pending().as_deref(), Some("first"));
    assert_eq!(chat.take_next_pending().as_deref(), Some("second"));
    assert!(chat.take_next_pending().is_none());
}

// Verifies: FR-UI-14 - `!cmd` runs and shows a bash card; `!!` is
// excluded from context.
#[test]
fn shell_mode_shows_a_bash_card() {
    let mut options = options();
    options.hooks.run_shell = Some(Arc::new(|cmd: &str, excluded: bool| {
        format!("ran {cmd} excluded={excluded}")
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "!!echo hi".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    let text = strip(&chat.render(80)).join("\n");
    assert!(text.contains("bash"), "{text}");
    assert!(text.contains("echo hi"), "{text}");
    assert!(text.contains("excluded=true"), "{text}");
}

// Verifies: FR-UI-15 - Ctrl+X Ctrl+E asks the loop for the external editor.
#[test]
fn ctrl_x_ctrl_e_opens_the_external_editor() {
    let mut chat = chat();
    assert_eq!(chat.handle_key("\x18"), Action::Continue); // Ctrl+X
    assert_eq!(chat.handle_key("\x05"), Action::ExternalEditor); // Ctrl+E
}

// Verifies: FR-UI-21 - `/fullscreen` toggles the screen mode and persists.
#[test]
fn fullscreen_toggles_and_persists() {
    let persisted = Arc::new(std::sync::Mutex::new(None));
    let sink = persisted.clone();
    let mut options = options();
    options.fullscreen = true;
    options.hooks.persist_screen_mode = Some(Arc::new(move |fullscreen| {
        *sink.lock().unwrap() = Some(fullscreen);
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    assert!(chat.screen_mode);
    for c in "/fullscreen".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(!chat.screen_mode);
    assert_eq!(*persisted.lock().unwrap(), Some(false));
}

// Verifies: FR-UI-11 - Alt+Up/Down hop between the user's own messages.
#[test]
fn alt_up_and_down_jump_between_prompts() {
    let mut chat = chat();
    chat.world.resize(80, 24);
    chat.transcript.push_user("first question");
    chat.transcript.append_text("an answer");
    chat.transcript.finish_assistant();
    chat.transcript.push_user("second question");
    assert!(chat.transcript.user_offsets(80, &chat.theme).len() >= 2);
    assert_eq!(chat.handle_key("\x1b[1;3A"), Action::Continue); // Alt+Up
    assert!(chat.jump_target.is_some());
    assert!(chat.take_jump_scroll(80, 24).is_some());
    assert_eq!(chat.handle_key("\x1b[1;3B"), Action::Continue); // Alt+Down
    assert!(chat.jump_target.is_some());
}

// Verifies: FR-UI-16 - `/tree` browses session branches and `/fork`
// creates one at a message.
#[test]
fn tree_browses_branches_and_fork_creates_one() {
    let mut options = options();
    options.hooks.session_tree = Some(Arc::new(|| {
        vec![
            ("root".into(), "root * (session)".into()),
            ("child".into(), "child (session)".into()),
        ]
    }));
    options.hooks.fork_at = Some(Arc::new(|n: usize| format!("forked at {n}: newbranch")));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/tree".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.tree_picker.is_some());
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(viewport.contains("root"), "{viewport}");
    assert!(viewport.contains("child"), "{viewport}");
    chat.handle_key("j");
    chat.handle_key("\r");
    assert!(
        chat.world.notice.as_deref().unwrap().contains("--resume"),
        "{:?}",
        chat.world.notice
    );
    for c in "/fork 1".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap()
            .contains("forked at 1"),
        "{:?}",
        chat.world.notice
    );
}

// Verifies: FR-UI-18 - a permission prompt shows a visible,
// keyboard-interruptible auto-approve countdown.
#[test]
fn permission_modal_shows_a_countdown() {
    let mut chat = chat();
    chat.world.permission = Some(crate::state::PermissionModal {
        action: "rm -rf /tmp/x".into(),
        respond: None,
        deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
    });
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(viewport.contains("auto-approves in"), "{viewport}");
}

// Verifies: FR-UI-17 - `/theme` previews live and restores on cancel.
#[test]
fn theme_picker_previews_and_restores() {
    let mut chat = chat();
    let original = chat.theme_name.clone();
    for c in "/theme".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.theme_picker.is_some());
    let start = chat.theme_picker.as_ref().unwrap().selected;
    chat.handle_key("j");
    assert_eq!(
        chat.theme_picker.as_ref().unwrap().selected,
        (start + 1).min(crate::theme::THEMES.len() - 1)
    );
    assert_eq!(chat.theme_name, original, "preview does not commit");
    chat.handle_key("\x1b"); // Escape restores
    assert!(chat.theme_picker.is_none());
    assert_eq!(chat.theme_name, original);
}

// Verifies: FR-UI-20 (the thinking level in the status area; R1)
#[test]
fn thinking_picker_sets_the_level() {
    let mut chat = chat();
    assert!(chat.thinking_level().is_none());
    for c in "/thinking".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.thinking_picker.is_some());
    assert_eq!(
        chat.thinking_picker.as_ref().unwrap().selected,
        0,
        "unset is the current row"
    );
    chat.handle_key("j"); // move to `off`
    chat.handle_key("\r");
    assert_eq!(chat.thinking_level().as_deref(), Some("off"));
    assert!(chat.thinking_picker.is_none());
    let text = strip(&chat.render(120)).join("\n");
    assert!(
        text.contains("\u{2022} off"),
        "footer shows the level:\n{text}"
    );
}

// Verifies: FR-UI-12 - Ctrl+R searches the transcript, highlights
// matches, and navigates between them.
#[test]
fn ctrl_r_searches_the_transcript() {
    let mut chat = chat();
    chat.world.resize(80, 24);
    chat.transcript.push_user("alpha question");
    chat.transcript.append_text("an answer");
    chat.transcript.finish_assistant();
    chat.transcript.push_user("beta question");
    chat.handle_key("\x12"); // Ctrl+R
    assert!(chat.search.is_some());
    for c in "question".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.search.as_deref(), Some("question"));
    assert!(chat.search_matches.len() >= 2, "two prompts match");
    assert!(chat.jump_target.is_some());
    chat.handle_key("\r"); // next match
    assert!(chat.search_index >= 1);
    let viewport = chat.viewport(80, 24, 0);
    assert!(
        viewport.iter().any(|l| l.contains("\x1b[7m")),
        "matches are highlighted"
    );
    chat.handle_key("\x1b"); // escape closes
    assert!(chat.search.is_none());
}

// Verifies: FR-UI-20 - the status area shows the cwd, the active model,
// and the queued-message count.
#[test]
fn status_area_shows_cwd_model_and_queue() {
    let mut chat = chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("queued".into(), lca_protocol::SubmitMode::FollowUp);
    let text = strip(&chat.render(120)).join("\n");
    assert!(text.contains("p/m"), "model shown:\n{text}");
    assert!(text.contains("1 queued"), "queue count shown:\n{text}");
}

// Verifies: FR-UI-21 - `/hotkeys` prints the binding registry.
#[test]
fn hotkeys_lists_bindings() {
    let mut chat = chat();
    for c in "/hotkeys".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let notice = chat.world.notice.as_deref().unwrap_or_default();
    assert!(notice.contains("keys:"), "{notice}");
    assert!(notice.contains("enter"), "{notice}");
}

// Verifies: FR-CORE-12 - edit-all-queued returns the queue to the editor
// and removes it from the boundary queue.
#[test]
fn edit_all_queued_restores_and_clears_the_boundary_queue() {
    let mut chat = chat();
    let steer = lca_protocol::steer_queue();
    chat.begin_turn(steer.clone());
    chat.queue_submit("one".into(), lca_protocol::SubmitMode::Steer);
    assert_eq!(steer.lock().unwrap().len(), 1);
    chat.handle_key("\x1be"); // Alt+E
    assert!(chat.pending.is_empty());
    assert_eq!(chat.editor.text(), "one");
    assert!(steer.lock().unwrap().is_empty());
}
