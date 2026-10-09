use super::*;
use lca_protocol::CommandEffect;
use lca_protocol::{ToolCall, ToolResult};
use lca_tui::engine::text::strip_terminal_sequences;
use std::path::PathBuf;

pub(super) fn options() -> UiOptions {
    UiOptions {
        prompt_slot: Default::default(),
        dialog_slot: Default::default(),
        pending_models: None,
        model_label: Arc::new(std::sync::Mutex::new("p/m".into())),
        context_window: Arc::new(std::sync::Mutex::new(0)),
        thinking: Arc::new(std::sync::Mutex::new(None)),
        theme: "auto".to_string(),
        theme_extra_dirs: Vec::new(),
        themes: crate::theme::THEMES.iter().map(|s| s.to_string()).collect(),
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
        slash_commands: vec!["/help".into(), "/model".into(), "/login".into()],
        models: vec![
            ("alpha".into(), "alpha".into()),
            ("beta".into(), "beta".into()),
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
        hooks: crate::state::UiHooks::default(),
        fullscreen: true,
    }
}

pub(super) fn chat() -> Chat {
    Chat::new(options(), Arc::new(KeybindingsManager::new()))
}

pub(super) fn strip(lines: &[String]) -> Vec<String> {
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

// Verifies: G2 (issue #3's safety half) - the `/model` completion menu
// shows the `model (provider)` label, but what Tab inserts is the raw id:
// a decorated insert would reach `/model <arg>` and be refused as an
// unknown model.
#[test]
fn model_completion_inserts_the_raw_id_and_shows_the_label() {
    let mut options = options();
    options.models = vec![(
        "deepseek-v4.1-flash".to_string(),
        "deepseek-v4.1-flash (openai-compatible)".to_string(),
    )];
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/model deep".chars() {
        chat.handle_key(&c.to_string());
    }
    let suggestions = chat.editor.suggestions().expect("the popup is open");
    assert_eq!(
        suggestions.items[0].label, "deepseek-v4.1-flash (openai-compatible)",
        "the menu row shows the label"
    );
    assert_eq!(
        suggestions.items[0].value, "deepseek-v4.1-flash",
        "the menu row's value is the raw id"
    );
    chat.handle_key("\t");
    assert_eq!(
        chat.editor.text(),
        "/model deepseek-v4.1-flash",
        "the insert is the raw id"
    );
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
    // R8: thinking is collapsed by default; expand to see it.
    chat.transcript.toggle_thinking_expanded();
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
        parent_call_id: None,
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

// ---------------------------------------------------------------------------
// R1/R6: the paste matrix. Every text surface takes a bracketed paste the
// way the editor does; the masked field masks it.
// ---------------------------------------------------------------------------

/// Wrap text as a real bracketed-paste event (what the engine emits).
fn bracketed(text: &str) -> String {
    format!("\x1b[200~{text}\x1b[201~")
}

// Verifies: R1/R6 - a paste into the masked secret field lands in the
// buffer and is masked in the frame; the secret bytes never appear.
#[test]
fn paste_into_the_masked_secret_field_is_masked() {
    let mut chat = chat();
    chat.apply_login_next(LoginNext::Secret {
        provider: "p".into(),
        label: "API key (input hidden)".into(),
        masked: true,
    });
    let secret = "sk-super-secret-value";
    chat.handle_key(&bracketed(secret));
    assert_eq!(chat.world.secret.as_ref().expect("prompt").input, secret);
    let text = strip(&chat.viewport(80, 24, 0)).join("\n");
    assert!(
        !text.contains(secret),
        "the secret never reaches the frame:\n{text}"
    );
    assert!(
        text.contains(&"*".repeat(secret.chars().count())),
        "the paste is masked, not dropped:\n{text}"
    );
}

// Verifies: R1 - a multi-line paste into a single-line field is flattened,
// not dropped and not broken across lines (pi's `input.ts` contract).
#[test]
fn paste_into_a_base_url_field_is_flattened() {
    let mut chat = chat();
    chat.apply_login_next(LoginNext::Secret {
        provider: "p".into(),
        label: "Base URL".into(),
        masked: false,
    });
    chat.handle_key(&bracketed("https://api.example.com/v1\r\nnext\ttab"));
    assert_eq!(
        chat.world.secret.as_ref().expect("prompt").input,
        "https://api.example.com/v1next    tab"
    );
}

// Verifies: R1 - the tmux CSI-u paste dialect reaches the same buffer,
// decoded.
#[test]
fn paste_into_the_secret_field_decodes_the_csi_u_dialect() {
    let mut chat = chat();
    chat.apply_login_next(LoginNext::Secret {
        provider: "p".into(),
        label: "API key (input hidden)".into(),
        masked: true,
    });
    chat.handle_key(&bracketed("sk-a\x1b[106;5ub"));
    assert_eq!(chat.world.secret.as_ref().expect("prompt").input, "sk-ab");
}

// Verifies: R1 - the model picker's search box takes a paste.
#[test]
fn paste_into_the_model_picker_search_filters_the_list() {
    let mut chat = chat();
    chat.model_picker = Some(ModelPicker::new(vec![
        ("alpha-1".into(), "alpha-1".into()),
        ("beta-2".into(), "beta-2".into()),
    ]));
    chat.handle_key(&bracketed("beta"));
    let picker = chat.model_picker.as_ref().expect("picker");
    assert_eq!(picker.query, "beta");
    assert_eq!(picker.matches, vec![1]);
}

// Verifies: R1 - the resume picker's search box takes a paste.
#[test]
fn paste_into_the_resume_picker_search_filters_the_list() {
    let mut chat = chat();
    chat.resume_picker = Some(crate::resume::ResumePicker::new(vec![
        crate::resume::SessionEntry {
            id: "a".into(),
            title: "alpha".into(),
            messages: 1,
            age: "now".into(),
        },
        crate::resume::SessionEntry {
            id: "b".into(),
            title: "beta".into(),
            messages: 2,
            age: "now".into(),
        },
    ]));
    chat.handle_key(&bracketed("beta"));
    let picker = chat.resume_picker.as_ref().expect("picker");
    assert_eq!(picker.query, "beta");
    assert_eq!(picker.matches, vec![1]);
}

// Verifies: R1 - the transcript search box takes a paste.
#[test]
fn paste_into_the_transcript_search_takes_the_query() {
    let mut chat = chat();
    chat.search = Some(String::new());
    chat.handle_key(&bracketed("the answer"));
    assert_eq!(chat.search.as_deref(), Some("the answer"));
}

#[test]
fn modals_composite_over_the_viewport() {
    let mut chat = chat();
    chat.world.show_permission("rm -rf /tmp/x".into());
    let viewport = chat.viewport(80, 24, 0);
    // In default scrollback mode, document length is preserved and overlays are bottom-anchored
    let text = strip(&viewport).join("\n");
    assert!(text.contains("rm -rf /tmp/x"));
}

// Verifies: S2 (issue #18) - dialogs are fully visible and centered in both
// scrollback (main-screen) and app-owned (alt-screen) modes, even at small terminal heights.
#[test]
fn dialogs_anchor_and_center_in_both_screen_modes() {
    let mut chat = chat();
    chat.world.show_permission("rm -rf /tmp/x".into());

    // 1. Main-screen (scrollback) mode at small height (12 rows)
    chat.screen_mode = false;
    let main_lines = chat.viewport(60, 12, 0);
    let main_text = strip(&main_lines).join("\n");
    assert!(
        main_text.contains("rm -rf /tmp/x"),
        "permission action is visible in main-screen mode"
    );
    assert!(
        main_text.contains("Allow this action?"),
        "modal title is visible in main-screen mode"
    );
    assert!(
        main_text.contains("╭─") && main_text.contains("╰─"),
        "modal frame is complete"
    );

    // 2. Alt-screen (fullscreen) mode at small height (12 rows)
    chat.screen_mode = true;
    let alt_lines = chat.viewport(60, 12, 0);
    let alt_text = strip(&alt_lines).join("\n");
    assert_eq!(
        alt_lines.len(),
        12,
        "alt screen sizes exactly to viewport height"
    );
    assert!(
        alt_text.contains("rm -rf /tmp/x"),
        "permission action is visible in alt-screen mode"
    );
    assert!(
        alt_text.contains("Allow this action?"),
        "modal title is visible in alt-screen mode"
    );
    assert!(
        alt_text.contains("╭─") && alt_text.contains("╰─"),
        "modal frame is complete"
    );
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
    options.hooks.run_shell = Some(Arc::new(
        |cmd: &str, excluded: bool, sink: std::sync::mpsc::SyncSender<crate::state::ShellEvent>| {
            let _ = sink.send(crate::state::ShellEvent::Chunk(format!(
                "ran {cmd} excluded={excluded}"
            )));
            let _ = sink.send(crate::state::ShellEvent::Done(Some(0)));
            Arc::new(|| {}) as crate::state::ShellHandle
        },
    ));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "!!echo hi".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert!(chat.shell_running());
    // R4: the output streams in on the loop's poll.
    assert!(chat.poll_shell());
    // Tool cards are collapsed by default (R8); expand to see the output.
    chat.transcript.toggle_tools_expanded();
    let text = strip(&chat.render(80)).join("\n");
    assert!(text.contains("bash"), "{text}");
    assert!(text.contains("echo hi"), "{text}");
    assert!(text.contains("excluded=true"), "{text}");
}

// Verifies: R4 - Escape cancels a running `!` command.
#[test]
fn escape_cancels_a_running_shell_command() {
    let mut options = options();
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = cancelled.clone();
    options.hooks.run_shell = Some(Arc::new(
        move |_cmd: &str,
              _excluded: bool,
              _sink: std::sync::mpsc::SyncSender<crate::state::ShellEvent>| {
            let flag = flag.clone();
            Arc::new(move || {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }) as crate::state::ShellHandle
        },
    ));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "!sleep 9".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.shell_running());
    chat.handle_key("\x1b");
    assert!(
        cancelled.load(std::sync::atomic::Ordering::SeqCst),
        "Escape called the cancel handle"
    );
}

// Verifies: R11 - modal focus: the modal owns keys while open, the editor
// regains them on close, and a key that only cancels the countdown keeps the
// responder alive.
#[test]
fn modal_focus_returns_to_the_editor_and_keeps_the_responder() {
    let mut chat = chat();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    chat.world.permission = Some(crate::state::PermissionModal {
        action: "rm -rf /tmp/x".into(),
        respond: Some(tx),
        deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
    });
    // A key that is neither a decision nor Escape only cancels the countdown.
    for c in "xyz".chars() {
        chat.handle_key(&c.to_string());
    }
    assert!(chat.world.permission.is_some(), "the modal stays open");
    assert_eq!(chat.editor.text(), "", "the modal owns the keys");
    // Deny closes the modal and reaches the waiting worker.
    chat.handle_key("d");
    assert!(chat.world.permission.is_none());
    assert_eq!(rx.try_recv().ok(), Some(lca_permissions::Decision::Denied));
    // The editor receives keys again.
    for c in "hi".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.editor.text(), "hi");
}

// Verifies: R20 - `!!` tells the shell runner the command is excluded from
// the model's context.
#[test]
fn a_bang_bang_command_is_marked_excluded() {
    let mut options = options();
    let seen = Arc::new(std::sync::Mutex::new(None));
    let cell = seen.clone();
    options.hooks.run_shell = Some(Arc::new(
        move |cmd: &str,
              excluded: bool,
              sink: std::sync::mpsc::SyncSender<crate::state::ShellEvent>| {
            *cell.lock().unwrap_or_else(|p| p.into_inner()) = Some((cmd.to_string(), excluded));
            let _ = sink.send(crate::state::ShellEvent::Done(Some(0)));
            Arc::new(|| {}) as crate::state::ShellHandle
        },
    ));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "!!ls".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        *seen.lock().unwrap_or_else(|p| p.into_inner()),
        Some(("ls".to_string(), true))
    );
}

// Verifies: R20 - an aborted turn returns its queued message to the editor.
#[test]
fn an_aborted_turn_returns_the_queue_to_the_editor() {
    let mut chat = chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("follow me".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(chat.pending.len(), 1);
    chat.restore_pending();
    assert!(chat.pending.is_empty());
    assert_eq!(chat.editor.text(), "follow me");
}

// Verifies: gh #9 (pi 1.0.0's `app.message.copy`) - Ctrl+X names a copy
// target only when the editor owns the keyboard, and the target is the
// sign-in URL on a waiting login screen, else the last assistant message
// as plain text. What actually reaches the clipboard is the loop's
// ladder (run.rs), tested there; this pins the target choice.
#[test]
fn ctrl_x_names_the_sign_in_url_or_the_last_assistant_message() {
    let mut chat = chat();
    assert!(
        chat.message_copy_key("\x18"),
        "Ctrl+X is pi's message-copy key"
    );
    assert!(
        !chat.message_copy_key("\x01"),
        "another key is not the copy key"
    );
    assert_eq!(chat.message_copy_text(), None, "nothing has been said yet");

    // The last assistant message, as plain text (the stored source, not
    // the rendered rows).
    chat.on_turn_event(lca_protocol::TurnEvent::TextDelta("the answer".into()));
    assert_eq!(
        chat.message_copy_text().as_deref(),
        Some("the answer"),
        "no selection involved: the last reply is the target"
    );

    // While a picker owns the keyboard, Ctrl+X is the picker's key.
    chat.model_picker = Some(crate::chat_pickers::ModelPicker::new(vec![(
        "m".to_string(),
        "m".to_string(),
    )]));
    assert!(
        !chat.message_copy_key("\x18"),
        "a picker owns the keyboard, so the key is not a copy"
    );
    chat.model_picker = None;

    // On a waiting login screen the sign-in URL wins over the reply:
    // that is what the person is waiting on.
    chat.apply_login_next(LoginNext::Waiting {
        label: "Waiting for the browser...\n\nhttps://example.test/device?code=42".to_string(),
    });
    assert_eq!(
        chat.message_copy_text().as_deref(),
        Some("https://example.test/device?code=42"),
        "the waiting screen's URL is the copy target"
    );
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
    options.fullscreen = false;
    options.hooks.persist_screen_mode = Some(Arc::new(move |fullscreen| {
        *sink.lock().unwrap() = Some(fullscreen);
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    assert!(!chat.screen_mode);
    for c in "/fullscreen".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.screen_mode);
    assert_eq!(*persisted.lock().unwrap(), Some(true));
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
    options.hooks.fork_at = Some(Arc::new(|n: usize| crate::state::ForkReport {
        id: Some("newbranch".to_string()),
        notice: format!("forked at {n}: newbranch"),
    }));
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
    // Requirement ids are for tests and docs, not for the person being
    // asked to approve something (manual tmux pass, 2026-10-01).
    assert!(!viewport.contains("FR-UI-18"), "{viewport}");
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

// Verifies: pain point #4 - `/model` reads the live model-list hook, so a
// login's post-grant discovery reaches the picker without a restart.
#[test]
fn the_live_model_hook_feeds_the_picker() {
    let mut options = options();
    options.hooks.models = Some(Arc::new(|| {
        vec![
            ("live-a".to_string(), "live-a".to_string()),
            ("live-b".to_string(), "live-b (some-provider)".to_string()),
        ]
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    assert_eq!(
        chat.model_rows(),
        vec![
            ("live-a".to_string(), "live-a".to_string()),
            ("live-b".to_string(), "live-b (some-provider)".to_string()),
        ]
    );
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.model_picker.is_some());
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(viewport.contains("live-a"), "{viewport}");
}

// Verifies: E3 - every picker overlay carries the shared hint row, so a
// swallowed slash command is no longer a surprise.
#[test]
fn pickers_show_the_hint_row() {
    let mut chat = chat();
    for c in "/theme".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(viewport.contains("enter apply"), "{viewport}");
    assert!(viewport.contains("esc restore"), "{viewport}");
    chat.handle_key("\x1b"); // close

    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(viewport.contains("type to filter"), "{viewport}");
}

// Verifies: R10 - the detected scheme sets the auto theme until an explicit
// pick wins.
#[test]
fn a_detected_scheme_sets_the_auto_theme() {
    let mut options = options();
    options.plain = false;
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    chat.apply_detected_scheme(lca_tui::engine::colors::ColorScheme::Light);
    assert_eq!(chat.theme_name, "light");
    chat.apply_detected_scheme(lca_tui::engine::colors::ColorScheme::Dark);
    assert_eq!(chat.theme_name, "dark");
    // An explicit pick wins over later detection.
    chat.set_theme("plain");
    chat.apply_detected_scheme(lca_tui::engine::colors::ColorScheme::Light);
    assert_eq!(chat.theme_name, "plain");
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

// Verifies: E2 - the `/thinking` pick persists to the config file, and
// `unset` removes the key.
#[test]
fn thinking_pick_persists_to_config() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    let mut options = options();
    options.hooks.persist_setting = Some(Arc::new(move |key: &str, value: Option<String>| {
        sink.lock().unwrap().push((key.to_string(), value));
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/thinking".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key("j"); // move to `off`
    chat.handle_key("\r");
    assert_eq!(
        seen.lock().unwrap().last().unwrap(),
        &("thinking".to_string(), Some("off".to_string()))
    );
    // Re-open and choose unset (row 0): the key is removed.
    for c in "/thinking".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key("k"); // back up to unset
    chat.handle_key("\r");
    assert_eq!(
        seen.lock().unwrap().last().unwrap(),
        &("thinking".to_string(), None)
    );
}

// Verifies: E2 - a committed `/theme` pick persists to the config file.
#[test]
fn theme_pick_persists_to_config() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let sink = seen.clone();
    let mut options = options();
    options.hooks.persist_setting = Some(Arc::new(move |key: &str, value: Option<String>| {
        *sink.lock().unwrap() = Some((key.to_string(), value));
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/theme".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key("\r"); // commit the highlighted row
    let seen = seen.lock().unwrap();
    let (key, value) = seen.as_ref().unwrap();
    assert_eq!(key, "ui.theme");
    assert!(value.is_some());
}

// Verifies: R2 - `/resume` opens a searchable session list and reports the
// resume command for the selected session.

// Verifies: the owner's "can we add tok/s to the footer?" ask - a finished
// turn reports its own generation speed from the provider's token count,
// and a tool run's latency is not part of it (the window closes at the
// tool call and reopens at the next delta).
#[test]
fn a_finished_turn_reports_tokens_per_second() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta("chunk".into()));
    chat.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "c1".into(),
        name: "shell".into(),
        arguments: "{}".into(),
        parent_call_id: None,
    }));
    // The tool's wall-clock time must not reach the reading: counting it
    // would cap 40 tokens at 40/0.15 = 266 tok/s.
    std::thread::sleep(std::time::Duration::from_millis(150));
    chat.on_turn_event(TurnEvent::ToolFinished(ToolResult::ok("c1", "out")));
    chat.on_turn_event(TurnEvent::Usage(Usage {
        output: 40,
        ..Default::default()
    }));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    let rate = chat.footer.tok_s.expect("the turn reports a rate");
    assert!(rate > 400, "rate {rate} includes tool time");
    let text = strip(&chat.render(120)).join("\n");
    assert!(text.contains("tok/s"), "{text}");
}

// Verifies: a turn with no streaming (tools only, or an immediate error)
// measures nothing and keeps the previous reading instead of inventing
// one or clearing a good number.
#[test]
fn a_turn_without_streaming_keeps_the_previous_rate() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta("chunk".into()));
    chat.on_turn_event(TurnEvent::Usage(Usage {
        output: 40,
        ..Default::default()
    }));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    let rate = chat.footer.tok_s.expect("the first turn reports a rate");

    chat.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "c2".into(),
        name: "shell".into(),
        arguments: "{}".into(),
        parent_call_id: None,
    }));
    chat.on_turn_event(TurnEvent::ToolFinished(ToolResult::ok("c2", "out")));
    chat.on_turn_event(TurnEvent::Usage(Usage {
        output: 5,
        ..Default::default()
    }));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    assert_eq!(chat.footer.tok_s, Some(rate), "no stream, no new reading");
}

// =============================================================================
// gh #27 (a): the prompt marker is two columns, so every editor row starts
// at the same visual column - the pad on the continuation rows, and the
// cursor marker's column following it.
// =============================================================================

/// Type `text` into the prompt, one key at a time.
fn type_prompt(chat: &mut Chat, text: &str) {
    for c in text.chars() {
        chat.handle_key(&c.to_string());
    }
}

/// The prompt's caret row and column, and that row's plain text.
fn caret_row(chat: &Chat, width: u16) -> (u16, String) {
    let (clean, pos) = lca_tui::engine::core::extract_cursor_position(&chat.render(width));
    let (row, col, _) = pos.expect("the prompt always carries the caret marker");
    (col, strip(&clean)[row as usize].clone())
}

// Verifies: gh #27 (a) - line 1 opens with `> ` and line 2 carries the same
// two columns as a plain pad, so no row steps back to column 0.
#[test]
fn every_editor_row_starts_at_the_marker_column() {
    let mut chat = chat();
    type_prompt(&mut chat, "alpha");
    chat.handle_key("\x1b[13;2u"); // shift+enter: a second prompt line
    type_prompt(&mut chat, "beta");

    chat.editor.cursor_up();
    chat.editor.cursor_line_start();
    let (column, row) = caret_row(&chat, 40);
    assert_eq!(column, 2, "the marker is two columns: {row:?}");
    assert_eq!(row, "> alpha", "line 1 carries the prompt marker");

    chat.editor.cursor_down();
    chat.editor.cursor_line_start();
    let (column, row) = caret_row(&chat, 40);
    assert_eq!(
        column, 2,
        "a continuation line starts at the marker column, not column 0: {row:?}"
    );
    assert_eq!(row, "  beta", "line 2 carries the marker-width pad");
}

// Verifies: gh #27 (a) - the cursor marker's column is uniform across every
// line of a multiline prompt: the same logical column reports the same raw
// cursor column on lines 1..n.
#[test]
fn the_cursor_column_is_uniform_across_every_editor_line() {
    let mut chat = chat();
    for (index, text) in ["alpha", "beta", "gamma"].iter().enumerate() {
        if index > 0 {
            chat.handle_key("\x1b[13;2u");
        }
        type_prompt(&mut chat, text);
    }

    // Start on the first line, so each `cursor_down` walks to the next one.
    chat.editor.cursor_line_start();
    chat.editor.cursor_up();
    chat.editor.cursor_up();

    let mut columns = Vec::new();
    for _ in 0..3 {
        chat.editor.cursor_line_start();
        for _ in 0..3 {
            chat.editor.cursor_right();
        }
        columns.push(caret_row(&chat, 40).0);
        chat.editor.cursor_down();
    }
    assert_eq!(
        columns,
        vec![5, 5, 5],
        "column 3 of the buffer, plus the two-column marker, on every line: {columns:?}"
    );
}

// Verifies: gh #43 (pi's `/skill:name` colon form): the colon routes
// like the space form, so both reach the host's `skill` command with
// the name first.
#[test]
fn skill_colon_form_routes_like_the_space_form() {
    use std::sync::Mutex;
    let seen: std::sync::Arc<Mutex<Vec<(String, String)>>> =
        std::sync::Arc::new(Mutex::new(Vec::new()));
    let capture = seen.clone();
    let mut options = options();
    options.slash_commands = vec!["/skill".into()];
    options.invoke_command = std::sync::Arc::new(move |name, argument| {
        capture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((name.to_string(), argument.to_string()));
        CommandEffect::None
    });
    let mut chat = Chat::new(options, std::sync::Arc::new(KeybindingsManager::new()));
    for c in "/skill:pdf-tools extract report.pdf".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    let seen = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        seen.as_slice(),
        &[(
            "skill".to_string(),
            "pdf-tools extract report.pdf".to_string()
        )],
        "colon form reaches the host as name + argument"
    );
}

// Verifies: gh #66 - `/hotkeys` prints the EFFECTIVE bindings: a rebound
// action shows its new key, not the default.
#[test]
fn hotkeys_shows_the_effective_rebound_key() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    use std::collections::BTreeMap;
    let mut user = BTreeMap::new();
    user.insert(
        "tui.editor.deleteCharBackward".to_string(),
        vec!["ctrl+q".to_string()],
    );
    let kb = std::sync::Arc::new(KeybindingsManager::with_user_bindings(user));
    let mut chat = Chat::new(options(), kb);
    for c in "/hotkeys".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let notice = chat.world.notice.clone().unwrap_or_default();
    assert!(
        notice.contains("ctrl+q - Delete character backward"),
        "the rebound key is listed: {notice}"
    );
    assert!(
        !notice.contains("backspace - Delete character backward"),
        "the replaced default is not: {notice}"
    );
}

// Verifies: gh #66 - a rebound key fires in the harness: with Backspace
// unbound and Ctrl+X bound to delete-backward, Ctrl+X deletes.
#[test]
fn a_rebound_key_fires() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    use std::collections::BTreeMap;
    let mut user = BTreeMap::new();
    user.insert(
        "tui.editor.deleteCharBackward".to_string(),
        vec!["ctrl+q".to_string()],
    );
    let kb = std::sync::Arc::new(KeybindingsManager::with_user_bindings(user));
    let mut chat = Chat::new(options(), kb);
    for c in "ab".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\x11");
    assert_eq!(chat.editor.text(), "a", "Ctrl+Q deleted backward");
}

#[cfg(test)]
#[path = "chat_command_tests.rs"]
mod command_tests;

#[cfg(test)]
#[path = "chat_dialog_tests.rs"]
mod dialog_tests;

#[cfg(test)]
#[path = "chat_drawer_tests.rs"]
mod drawer_tests;

#[cfg(test)]
#[path = "chat_tooltip_tests.rs"]
mod tooltip_tests;

#[cfg(test)]
#[path = "chat_overlay_tests.rs"]
mod overlay_tests;

#[cfg(test)]
#[path = "chat_picker_scroll_tests.rs"]
mod picker_scroll_tests;

#[cfg(test)]
#[path = "chat_viewport_tests.rs"]
mod viewport_tests;

// Verifies: gh #41 (a model without `high` thinking does not offer
// it): the picker hides unsupported levels instead of merely
// clamping after the fact.
#[test]
fn a_model_without_high_thinking_does_not_offer_it() {
    let mut chat = chat();
    chat.world.options.hooks.thinking_offered = Some(std::sync::Arc::new(|| {
        vec!["off".to_string(), "low".to_string()]
    }));
    chat.open_thinking_picker();
    let picker = chat.thinking_picker.expect("the picker opens");
    assert_eq!(picker.offered, vec!["off".to_string(), "low".to_string()]);
    assert!(
        !picker.offered.iter().any(|level| level == "high"),
        "high is hidden, not offered: {:?}",
        picker.offered
    );
}

// Verifies: gh #41 (no offered set means every level, the historical
// behavior for models without a configured set).
#[test]
fn a_model_without_a_set_offers_every_level() {
    let mut chat = chat();
    chat.open_thinking_picker();
    let picker = chat.thinking_picker.expect("the picker opens");
    assert_eq!(picker.offered.len(), 7, "unset + six levels");
    assert!(picker.offered.contains(&"high".to_string()));
}
