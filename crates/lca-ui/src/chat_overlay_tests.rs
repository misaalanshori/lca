//! `Chat`'s overlay half: resume/tree/model/thinking/theme pickers, the
//! grants and trust views, the login wait, replay, and the cycle-9 rows
//! (theme repaint, separator, queue marker, background compaction).
//!
//! Split from `chat_tests.rs` so both halves stay under the workspace's
//! 1,200-line file ceiling. The fixtures (`options`, `chat`, `strip`) stay
//! in `chat_tests.rs` and are imported from it.

use super::tests::{chat, options, strip};
use super::*;
use lca_protocol::CommandEffect;

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
        (id == "s2").then(|| {
            vec![lca_protocol::Record::User {
                v: lca_protocol::FORMAT_VERSION,
                ts: 1,
                id: "r1".into(),
                content: "from s2".into(),
                attachments: Vec::new(),
                queue: None,
            }]
        })
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

// Verifies: FR-PERM-23 - the permission modal's `t` trusts the folder for
// the session without persisting a pattern.
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

// Verifies: FR-PERM-24 - the `/trust` picker applies the chosen scope.
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

// Verifies: FR-PERM-24 - the trust prompt opens at startup only when the
// host says the project needs a decision (Pi's resources rule).
#[test]
fn the_trust_prompt_opens_when_the_host_asks() {
    let mut opts = options();
    opts.hooks.trust_needed = Some(Arc::new(|| true));
    let chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    assert!(chat.trust_picker.is_some());
    let quiet = Chat::new(options(), Arc::new(KeybindingsManager::new()));
    assert!(quiet.trust_picker.is_none());
}

// Verifies: R4 - a background login step renders as a cancellable waiting
// state that owns the keyboard (so a 300-second OAuth callback can never
// freeze the app or leak keys into the editor behind it).
#[test]
fn the_login_wait_state_renders_and_owns_the_keyboard() {
    let mut chat = chat();
    chat.apply_login_next(LoginNext::Waiting {
        label: "waiting for browser sign-in… (esc cancels)".into(),
    });
    let text = strip(&chat.viewport(80, 24, 0)).join("\n");
    assert!(
        text.contains("waiting for browser sign-in"),
        "the waiting state is visible:\n{text}"
    );
    assert!(text.contains("esc cancels"), "the cancel is named:\n{text}");
    // The waiting state owns every key: typing cannot reach the editor.
    for c in "secretxyz".chars() {
        assert_eq!(
            chat.handle_key(&c.to_string()),
            Action::Continue,
            "keys are swallowed while waiting"
        );
    }
    assert_eq!(chat.editor.text(), "", "nothing leaked into the editor");
}

// Verifies: R4 - Escape cancels the background step through the host hook.
#[test]
fn escape_cancels_the_login_wait_through_the_hook() {
    let mut chat = chat();
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    chat.world.options.hooks.cancel_login = Some(Arc::new({
        let flag = cancelled.clone();
        move || flag.store(true, std::sync::atomic::Ordering::SeqCst)
    }));
    chat.apply_login_next(LoginNext::Waiting {
        label: "waiting…".into(),
    });
    chat.handle_key("\x1b");
    assert!(
        cancelled.load(std::sync::atomic::Ordering::SeqCst),
        "the host hook ran"
    );
    assert!(
        chat.world.login_waiting.is_none(),
        "the waiting state is gone"
    );
    assert!(
        chat.world
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("cancelled")),
        "the user is told: {:?}",
        chat.world.notice
    );
}

// Verifies: R4 - the poll hands the interface the finished step, which
// lands as an ordinary notice.
#[test]
fn a_finished_login_result_lands_when_the_poll_returns_it() {
    let mut chat = chat();
    let cell: Arc<std::sync::Mutex<Option<LoginNext>>> = Arc::new(std::sync::Mutex::new(Some(
        LoginNext::Message("signed in via `antigravity`".into()),
    )));
    chat.world.options.hooks.poll_login = Some(Arc::new({
        let cell = cell.clone();
        move || cell.lock().unwrap().take()
    }));
    let poll = chat.world.options.hooks.poll_login.clone().expect("hook");
    chat.apply_login_next(LoginNext::Waiting {
        label: "wait".into(),
    });
    if let Some(next) = poll() {
        chat.apply_login_next(next);
    }
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("signed in via `antigravity`")
    );
    assert!(chat.world.login_waiting.is_none(), "the wait is over");
    // A second poll with nothing pending changes nothing.
    let poll = chat.world.options.hooks.poll_login.clone().expect("hook");
    assert!(poll().is_none());
}

// Verifies: FR-UI-7 - a session replayed through `/resume`, `/tree`, or a
// restart renders like the live one: the user band, markdown (a table
// stays a table), and one tool card. The plain `user:`/`assistant:` dump
// this replaced flattened every message to a line, so a resumed table came
// back as pipe characters (manual side-by-side against pi, 2026-10-01).
// FR-UI-20: the replayed usage feeds the footer's session totals.
#[test]
fn a_replayed_session_renders_like_the_live_one() {
    let mut chat = chat();
    let records = vec![
        lca_protocol::Record::User {
            v: lca_protocol::FORMAT_VERSION,
            ts: 1,
            id: "r1".into(),
            content: "show me a table".into(),
            attachments: Vec::new(),
            queue: None,
        },
        lca_protocol::Record::Assistant {
            v: lca_protocol::FORMAT_VERSION,
            ts: 2,
            id: "r2".into(),
            content: vec![lca_protocol::ContentBlock::Text {
                text: "Fruit prices\n\n| Fruit | Price |\n|---|---|\n| Apples | $1.99 |\n\nDone."
                    .into(),
            }],
            reasoning: Some("needs a table".into()),
            model: Some("m".into()),
            provider: Some("p".into()),
            usage: Some(lca_protocol::Usage {
                input: 100,
                output: 20,
                ..Default::default()
            }),
        },
        lca_protocol::Record::ToolCall {
            v: lca_protocol::FORMAT_VERSION,
            ts: 3,
            id: "r3".into(),
            call_id: "c1".into(),
            name: "read".into(),
            arguments: "{\"path\":\"a.txt\"}".into(),
            source: lca_protocol::ToolSource::Builtin,
        },
        lca_protocol::Record::ToolResult {
            v: lca_protocol::FORMAT_VERSION,
            ts: 4,
            id: "r4".into(),
            call_id: "c1".into(),
            status: lca_protocol::ToolResultStatus::Ok,
            content: Some("line one".into()),
            attachment: None,
            truncated: false,
            exit_code: None,
            full_output_path: None,
        },
    ];
    chat.load_records(&records, None);
    let text = strip(&chat.render(100)).join("\n");
    assert!(text.contains("show me a table"), "{text}");
    assert!(
        text.contains('┌'),
        "the markdown table renders as a table: {text}"
    );
    assert!(!text.contains("assistant:"), "no plain dump: {text}");
    assert!(!text.contains("user:"), "no plain dump: {text}");
    assert!(
        !text.contains("requested"),
        "the tool card does not duplicate the assistant's tool-call block: {text}"
    );
    assert!(text.contains("read"), "the tool card renders: {text}");
    assert_eq!(
        chat.usage.input, 100,
        "the replayed usage feeds the footer totals"
    );
    assert_eq!(chat.usage.output, 20, "the replayed usage feeds the footer");
}

// Verifies: FR-UI-13 - a replayed attachment renders as the image card when
// its bytes are still on disk (the record's own stub line then being a
// second copy of it, which the live transcript never showed), and stays as
// the named stub when the file is gone.
#[test]
fn a_replayed_attachment_renders_once_or_as_a_named_placeholder() {
    let hash = "0123456789abcdef".to_string();
    let record = lca_protocol::Record::User {
        v: lca_protocol::FORMAT_VERSION,
        ts: 1,
        id: "r1".into(),
        content: "look at this\n[image attachment 01234567, image/png, 3 bytes]".into(),
        attachments: vec![hash.clone()],
        queue: None,
    };
    let loader: crate::state::LoadAttachment =
        Arc::new(move |h: &str| (h == hash).then(|| ("image/png".to_string(), vec![1, 2, 3])));

    let mut with_file = chat();
    with_file.load_records(std::slice::from_ref(&record), Some(&loader));
    let text = strip(&with_file.render(100)).join("\n");
    assert!(text.contains("look at this"), "{text}");
    assert!(text.contains("image/png"), "the image card renders: {text}");
    assert!(
        !text.contains("[image attachment"),
        "the stub is not shown next to its own card: {text}"
    );

    let mut without_file = chat();
    without_file.load_records(&[record], None);
    let text = strip(&without_file.render(100)).join("\n");
    assert!(
        text.contains("[image attachment 01234567"),
        "a missing image keeps its named placeholder: {text}"
    );
}

// Verifies: R1 - a theme change repaints the transcript. The render cache
// is keyed by width only, so switching palettes has to drop it or the
// bands keep the previous theme's bytes (this is also what made a live
// `/theme` preview lie about the user band).
#[test]
fn a_theme_change_repaints_the_cached_transcript() {
    let mut opts = options();
    opts.plain = false;
    opts.theme = "dark".to_string();
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.transcript.push_user("hello");
    let painted = chat.render(60);
    assert!(
        painted.iter().any(|l| l.contains("\x1b[48;2;52;53;65m")),
        "the user band is painted in the dark theme: {painted:?}"
    );
    chat.set_theme("plain");
    let plain = chat.render(60);
    assert!(
        plain
            .iter()
            .all(|l| !l.contains("\x1b[48;2;") && !l.contains("\x1b[38;2;")),
        "the cached rows were dropped: {plain:?}"
    );
    assert!(
        plain.iter().any(|l| l.contains("› hello")),
        "the band still renders, now as text: {plain:?}"
    );
}

// Verifies: R2 - the separator follows the turn: rest when idle, pi's
// spinner row while a turn runs, exactly one full row, and back to rest
// when the turn ends.
#[test]
fn the_separator_follows_the_turn() {
    let mut chat = chat();
    assert_eq!(chat.separator.state(), &crate::SeparatorState::Idle);
    chat.begin_turn(lca_protocol::steer_queue());
    assert_eq!(chat.separator.state(), &crate::SeparatorState::Working);
    let working: Vec<String> = chat.render(60);
    let row = working
        .iter()
        .find(|l| l.contains("Working"))
        .expect("the working separator is on screen")
        .clone();
    assert_eq!(lca_tui::engine::text::visible_width(&row), 60);
    assert_eq!(
        working.iter().filter(|l| l.contains("Working")).count(),
        1,
        "exactly one separator row: {working:?}"
    );
    chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
        status: lca_protocol::TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
    });
    assert_eq!(chat.separator.state(), &crate::SeparatorState::Idle);
    assert!(
        !chat.render(60).iter().any(|l| l.contains("Working")),
        "the indicator clears when the turn ends"
    );
}

// Verifies: ADR-0038 - a queued message that becomes the next turn keeps
// the marker it was queued with, and an ordinary submit carries none. The
// flush used to hand over only the text, so the record this turn wrote
// called a queued message an ordinary prompt.
#[test]
fn a_flushed_queue_message_keeps_its_marker() {
    let mut chat = chat();
    chat.queue_submit("later".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(chat.take_next_pending().as_deref(), Some("later"));
    assert_eq!(
        chat.take_submitted_queue(),
        Some(lca_protocol::SubmitMode::FollowUp),
        "the flush hands the marker to the turn"
    );

    // An ordinary submit carries none (the queued flag is cleared with it).
    assert_eq!(chat.on_submit("plain prompt".to_string()), Action::Submit);
    assert_eq!(chat.take_submitted().as_deref(), Some("plain prompt"));
    assert_eq!(chat.take_submitted_queue(), None);
}

// Verifies: the background `/compact` handshake. The summarization call is
// a model round-trip; running it on the interface's thread froze the pane
// for its whole duration (measured live: a typed character only appeared
// 3.5 s later, together with the notice). The command now reports through
// `poll_compact`, which raises the working state, keeps the loop
// repainting, and posts the result - the shape pi gives the same moment
// with `CompactionStatusIndicator`.
#[test]
fn a_background_compaction_drives_the_working_state() {
    use crate::state::{CompactPoll, CompactState};

    let state = Arc::new(std::sync::Mutex::new(CompactState::Idle));
    let mut opts = options();
    let slot = state.clone();
    opts.hooks.poll_compact = Some(Arc::new(move || {
        let mut guard = slot.lock().unwrap_or_else(|p| p.into_inner());
        // The hook consumes `Done`, as the contract requires; `Running`
        // stays put until there is something to hand over.
        match guard.clone() {
            CompactState::Done(notice) => {
                *guard = CompactState::Idle;
                CompactState::Done(notice)
            }
            ongoing => ongoing,
        }
    }) as CompactPoll);
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));

    // Nothing in flight: no state change, no repaint.
    assert!(!chat.poll_compact());
    assert_eq!(chat.separator.state(), &crate::SeparatorState::Idle);

    // The command starts summarizing.
    *state.lock().unwrap() = CompactState::Running;
    assert!(chat.poll_compact(), "the start is a repaint");
    assert_eq!(chat.separator.state(), &crate::SeparatorState::Working);
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("compacting"),
        "the interface says what is happening: {:?}",
        chat.world.notice
    );
    assert!(!chat.poll_compact(), "still running: no repaint storm");

    // The summary lands.
    *state.lock().unwrap() = CompactState::Done("compacted: 42 tokens".into());
    assert!(chat.poll_compact());
    assert_eq!(chat.separator.state(), &crate::SeparatorState::Idle);
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("compacted: 42 tokens"),
        "{:?}",
        chat.world.notice
    );
    assert!(!chat.poll_compact(), "the result is consumed");
}

// Verifies: pi's model-selector row count (`(1/126)` in
// `model-selector.ts`) - the /model list says where you are in it and how
// many rows it has.
#[test]
fn the_model_picker_shows_its_position_and_size() {
    let mut chat = chat();
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.model_picker.is_some());
    let viewport = strip(&chat.viewport(120, 30, 0)).join("\n");
    assert!(viewport.contains("(1/2)"), "{viewport}");
}

// Verifies: issue #1, kept honest by gh #25 - the namespaced `<ext>.login`
// reaches the host's command dispatch, which runs the provider's identity
// `login` export (FR-PROV-10) instead of answering with the raw "no API key
// is configured" text issue #1 hit. The preset-picker seam belongs to
// `/login` alone: issue #1's fix intercepted every `*.login` here, and that
// interception is what issue #25 moved back to the bare command.
#[test]
fn the_namespaced_login_reaches_the_host_command_dispatch() {
    let targets: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let invoked = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut options = options();
    // What the real host registers: the auto-namespaced identity export is
    // one of the extension's own commands (FR-PROV-10).
    options.slash_commands = vec!["/login".to_string(), "/antigravity.login".to_string()];
    let seen = targets.clone();
    options.login = Some(Arc::new(move |target: &str| {
        seen.lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(target.to_string());
        LoginNext::Message("picker opened".to_string())
    }));
    let hit = invoked.clone();
    options.invoke_command = Arc::new(move |_, _| {
        hit.store(true, std::sync::atomic::Ordering::SeqCst);
        CommandEffect::ShowWidget("identity flow started".to_string())
    });

    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/antigravity.login".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        invoked.load(std::sync::atomic::Ordering::SeqCst),
        "the host's dispatch owns the identity export"
    );
    assert!(
        targets.lock().unwrap_or_else(|p| p.into_inner()).is_empty(),
        "the preset-picker seam never sees a namespaced login: {:?}",
        targets.lock().unwrap_or_else(|p| p.into_inner())
    );
    assert_eq!(chat.world.notice.as_deref(), Some("identity flow started"));
}

// Verifies: gh #178 - the OAuth receipt case: a 200+ character sign-in
// URL in the login modal wraps across rows with every row re-opening
// the full OSC 8 link, so any wrapped row clicks open the whole URL.
#[test]
fn a_wrapped_modal_url_clicks_open_whole() {
    let mut chat = chat();
    let url = format!(
        "https://accounts.example.com/o/oauth2/v2/auth?response_type=code&client_id={}&redirect_uri=https%3A%2F%2Flocalhost%3A8080%2Fcallback&scope=email%20profile",
        "7".repeat(120)
    );
    chat.apply_login_next(LoginNext::Waiting {
        label: format!("waiting for browser sign-in… (esc cancels)\n\n{url}"),
    });
    let frame = chat.viewport(80, 24, 0);
    let rows: Vec<&String> = frame
        .iter()
        .filter(|row| {
            lca_tui::engine::text::strip_terminal_sequences(row).contains("accounts.example.com")
                || row.contains("accounts.example.com")
        })
        .collect();
    assert!(rows.len() >= 3, "the URL wraps: {}", rows.len());
    for row in &rows {
        assert!(
            row.contains(&format!("\x1b]8;;{url}\x07")),
            "every wrapped row re-opens the full URL: {row:?}"
        );
    }
    let middle = rows[rows.len() / 2];
    let plain = lca_tui::engine::text::strip_terminal_sequences(middle);
    // A column inside the link text (the row's midpoint is URL on every
    // wrapped segment, never the box border).
    let col = lca_tui::engine::text::visible_width(&plain) / 2;
    assert_eq!(
        lca_tui::engine::text::get_osc8_link_at_column(middle, col),
        Some(url),
        "hit-testing a wrapped modal row resolves the link"
    );
}
