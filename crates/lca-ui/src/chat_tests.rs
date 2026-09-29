use super::*;
use lca_protocol::CommandEffect;
use lca_protocol::{ToolCall, ToolResult};
use lca_tui::engine::text::strip_terminal_sequences;
use std::path::PathBuf;

fn options() -> UiOptions {
    UiOptions {
        model_label: Arc::new(std::sync::Mutex::new("p/m".into())),
        context_window: Arc::new(std::sync::Mutex::new(0)),
        thinking: Arc::new(std::sync::Mutex::new(None)),
        theme: "auto".to_string(),
        theme_dir: std::path::PathBuf::new(),
        themes: crate::theme::THEMES.iter().map(|s| s.to_string()).collect(),
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

// Verifies: pain point #4 - `/model` reads the live model-list hook, so a
// login's post-grant discovery reaches the picker without a restart.
#[test]
fn the_live_model_hook_feeds_the_picker() {
    let mut options = options();
    options.hooks.models = Some(Arc::new(|| {
        vec!["live-a".to_string(), "live-b".to_string()]
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    assert_eq!(chat.model_ids(), vec!["live-a", "live-b"]);
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
#[test]
fn resume_picker_searches_sessions() {
    let mut chat = chat();
    chat.world.options.hooks.session_list = Some(Arc::new(|| {
        vec![
            crate::resume::SessionEntry {
                id: "a".into(),
                title: "parser fix".into(),
                messages: 3,
                age: "5m".into(),
            },
            crate::resume::SessionEntry {
                id: "b".into(),
                title: "docs pass".into(),
                messages: 1,
                age: "2d".into(),
            },
        ]
    }));
    for c in "/resume".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.resume_picker.is_some());
    for c in "docs".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.resume_picker.as_ref().unwrap().matches, vec![1]);
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(viewport.contains("docs pass"), "{viewport}");
    chat.handle_key("\r");
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap_or("")
            .contains("lca --resume b"),
        "{:?}",
        chat.world.notice
    );
    chat.handle_key("\x1b");
    assert!(chat.resume_picker.is_none());
}

// Verifies: R3 - selecting a branch in `/tree` switches the session in
// place when the host supports it, and rebuilds the transcript.
#[test]
fn tree_selection_switches_the_session() {
    let mut chat = chat();
    chat.world.options.hooks.session_tree = Some(Arc::new(|| {
        vec![("s1".into(), "s1 *".into()), ("s2".into(), "s2".into())]
    }));
    chat.world.options.hooks.switch_session = Some(Arc::new(|id: &str| {
        (id == "s2").then(|| vec!["user: from s2".to_string()])
    }));
    for c in "/tree".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key("j"); // move to s2
    chat.handle_key("\r");
    assert!(chat.tree_picker.is_none());
    let text = strip(&chat.render(80)).join("\n");
    assert!(text.contains("from s2"), "{text}");
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap_or("")
            .contains("switched to session s2"),
        "{:?}",
        chat.world.notice
    );
}

// Verifies: R9 - `/model` opens a searchable picker and Enter selects.
#[test]
fn model_picker_searches_and_selects() {
    let mut chat = chat();
    let selected = Arc::new(std::sync::Mutex::new(String::new()));
    let cell = selected.clone();
    chat.world.options.invoke_command = Arc::new(move |name: &str, arg: &str| {
        *cell.lock().unwrap_or_else(|p| p.into_inner()) = format!("{name}:{arg}");
        CommandEffect::ShowWidget(format!("model for this session: {arg}"))
    });
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.model_picker.is_some());
    for c in "beta".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.model_picker.as_ref().unwrap().matches, vec![1]);
    chat.handle_key("\r");
    assert!(chat.model_picker.is_none());
    assert_eq!(
        *selected.lock().unwrap_or_else(|p| p.into_inner()),
        "model:beta"
    );
}

// Verifies: FR-UI-20 - the footer shows the context use from the last call.
#[test]
fn the_footer_shows_context_use() {
    let mut chat = chat();
    *chat
        .world
        .options
        .context_window
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = 1000;
    chat.on_turn_event(TurnEvent::Usage(lca_protocol::Usage {
        input: 100,
        cache_read: 200,
        cache_write: 50,
        output: 10,
        ..Default::default()
    }));
    let text = strip(&chat.render(120)).join("\n");
    assert!(text.contains("ctx 35%"), "{text}");
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

// Verifies: S8 - the grants view composites the two groups over the
// viewport, with the revocable rows called out.
#[test]
fn the_grants_view_composites_both_groups() {
    let mut chat = chat();
    chat.grants_picker = Some(GrantPicker {
        entries: vec![
            lca_ui_entry(true, "openai-compatible", "enabled"),
            lca_ui_entry(false, "ad hoc", "echo tool-done"),
        ],
        selected: 1,
    });
    let text = strip(&chat.viewport(100, 24, 0)).join("\n");
    assert!(text.contains("Grants for this project"), "{text}");
    assert!(text.contains("Install consent"), "{text}");
    assert!(text.contains("Ad hoc"), "{text}");
    assert!(text.contains("echo tool-done"), "{text}");
}

fn lca_ui_entry(install_consent: bool, subject: &str, detail: &str) -> crate::state::GrantEntry {
    crate::state::GrantEntry {
        install_consent,
        subject: subject.to_string(),
        detail: detail.to_string(),
        revocable: !install_consent,
    }
}

// Verifies: ADR-0039 - the permission modal's `t` trusts the folder for the
// session without persisting a pattern.
#[test]
fn the_permission_modal_can_trust_the_folder() {
    let mut chat = chat();
    let (respond, response) = std::sync::mpsc::sync_channel(1);
    chat.world.permission = Some(crate::state::PermissionModal {
        action: "cargo build".into(),
        respond: Some(respond),
        deadline: None,
    });
    chat.handle_key("t");
    assert_eq!(
        response.try_recv().ok(),
        Some(lca_permissions::Decision::TrustFolder)
    );
}

// Verifies: ADR-0039 - the `/trust` picker applies the chosen trust scope.
#[test]
fn the_trust_picker_applies_a_choice() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let sink = seen.clone();
    let mut options = options();
    options.hooks.trust_apply = Some(Arc::new(move |choice| {
        *sink.lock().unwrap() = Some(choice);
        "ok".to_string()
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/trust".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.trust_picker.is_some());
    chat.handle_key("j"); // move to "this session only"
    chat.handle_key("\r");
    assert_eq!(
        *seen.lock().unwrap(),
        Some(crate::state::TrustChoice::Session(true))
    );
    assert!(chat.trust_picker.is_none());
}

// Verifies: ADR-0039 - the trust prompt opens at startup only when the host
// says the project needs a decision (Pi's trust-requiring-resources rule).
#[test]
fn the_trust_prompt_opens_when_the_host_asks() {
    let mut opts = options();
    opts.hooks.trust_needed = Some(Arc::new(|| true));
    let chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    assert!(chat.trust_picker.is_some());
    let quiet = Chat::new(options(), Arc::new(KeybindingsManager::new()));
    assert!(quiet.trust_picker.is_none());
}
