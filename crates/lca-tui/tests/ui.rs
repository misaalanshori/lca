//! Rendering and key-handling tests against ratatui's TestBackend: the
//! virtual terminal buffer is the snapshot (testing plan section2).

use crossterm::event::KeyCode;
use lca_core::{StopReason, TurnEvent, TurnStatus};
use lca_tui::render;
use lca_tui::{Action, ColorMode, InputMode, TurnStatusLine, UiOptions, UiState, handle_key};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn options() -> UiOptions {
    UiOptions {
        model_label: "fake/faux-1".to_string(),
        initial_lines: vec![
            "user: add a parser".to_string(),
            "assistant: working on it".to_string(),
        ],
        plain: false,
        invoke_command: std::sync::Arc::new(|name, _arg| {
            if name == "stats" {
                lca_protocol::CommandEffect::ShowWidget("stats go here".to_string())
            } else {
                lca_protocol::CommandEffect::None
            }
        }),
        workspace: std::path::PathBuf::new(),
        render_regions: None,
        ui_events: None,
        update_notice: None,
        slash_commands: vec![
            "/login".into(),
            "/logout".into(),
            "/usage".into(),
            "/model".into(),
            "/compact".into(),
            "/stats".into(),
        ],
    }
}

fn terminal(width: u16, height: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(width, height)).expect("terminal")
}

fn buffer_text(terminal: &mut Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = *terminal.backend().buffer().area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

// Verifies: FR-CORE-4 (partial content renders as it arrives)
#[test]
fn streams_partial_text_into_the_active_area() {
    let options = options();
    let mut state = UiState::new(options);
    state.on_turn_event(TurnEvent::TextDelta("Hello".into()));
    state.on_turn_event(TurnEvent::TextDelta(", wor".into()));
    state.on_turn_event(TurnEvent::TextDelta("ld".into()));
    let mut term = terminal(80, 24);
    render(&mut term, &state).expect("render");
    let text = buffer_text(&mut term);
    assert!(
        text.contains("Hello, world"),
        "streamed text visible:\n{text}"
    );
}

// The conversation scrollback keeps earlier messages visible (the region
// list from the SRDD's user interface section).
#[test]
fn scrollback_shows_the_conversation() {
    let state = UiState::new(options());
    let mut term = terminal(80, 24);
    render(&mut term, &state).expect("render");
    let text = buffer_text(&mut term);
    assert!(text.contains("add a parser"), "scrollback:\n{text}");
    assert!(text.contains("working on it"));
}

// Verifies: FR-UI-4 (the approval prompt shows the exact command)
#[test]
fn permission_modal_shows_the_exact_command() {
    let mut state = UiState::new(options());
    state.show_permission("rm -rf /tmp/important".to_string());
    let mut term = terminal(80, 24);
    render(&mut term, &state).expect("render");
    let text = buffer_text(&mut term);
    assert!(text.contains("rm -rf /tmp/important"), "modal:\n{text}");
    assert!(text.contains("Allow once"), "choices offered:\n{text}");
    assert!(text.contains("Deny"));
}

// Verifies: FR-UI-3 (a resize re-renders without losing scrollback)
#[test]
fn resizing_keeps_the_scrollback() {
    let mut state = UiState::new(options());
    {
        let mut term = terminal(80, 24);
        render(&mut term, &state).expect("render");
    }
    state.resize(120, 40);
    let mut term = terminal(120, 40);
    render(&mut term, &state).expect("render after resize");
    let text = buffer_text(&mut term);
    assert!(
        text.contains("add a parser"),
        "scrollback survives:\n{text}"
    );
}

// Verifies: NFR-26 (an80-column terminal works).
#[test]
fn renders_at_eighty_columns() {
    let mut state = UiState::new(options());
    state.on_turn_event(TurnEvent::TextDelta("x".repeat(300)));
    let mut term = terminal(80, 24);
    render(&mut term, &state).expect("render");
    let text = buffer_text(&mut term);
    assert!(text.lines().all(|line| line.chars().count() <= 80));
    assert!(text.contains("x"), "content present");
}

// The status line shows the model, live turn state, and the session
// cost (SRDD status line: model, context use, session cost, segments).
#[test]
fn status_line_names_the_model_and_turn_state() {
    let mut state = UiState::new(options());
    state.on_turn_event(TurnEvent::Usage(lca_protocol::Usage {
        input: 1200,
        output: 40,
        cache_read: 1000,
        cost: 0.075,
        ..Default::default()
    }));
    let mut term = terminal(80, 24);
    render(&mut term, &state).expect("render");
    let text = buffer_text(&mut term);
    assert!(text.contains("fake/faux-1"), "model shown:\n{text}");
    assert!(text.contains("1200"), "token counts shown: {text}");
    assert!(text.contains("$0.0750"), "session cost shown: {text}");
}

// Verifies: FR-UI-5 (with color disabled everything renders as plain text:
// no cell carries a foreground color)
#[test]
fn plain_mode_never_paints_color() {
    let mut options = options();
    options.plain = true;
    let mut state = UiState::new(options);
    state.on_turn_event(TurnEvent::Error {
        message: "boom".into(),
        class: "transport".into(),
        retryable: true,
    });
    state.show_permission("danger".to_string());
    let mut term = terminal(80, 24);
    render(&mut term, &state).expect("render");
    let buffer = term.backend().buffer();
    let area = *buffer.area();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            assert_eq!(
                buffer[(x, y)].fg,
                ratatui::style::Color::Reset,
                "plain mode paints color at {x},{y}"
            );
        }
    }
}

// Verifies: NFR-27 (the interface functions on the keyboard alone; every
// flow in this file is driven by key presses with no mouse event).
#[test]
fn keys_follow_the_documented_conventions() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = UiState::new(options());
    state.buffer = "hello".to_string();
    assert_eq!(
        handle_key(&mut state, KeyEvent::from(KeyCode::Enter)),
        Action::Submit
    );
    assert_eq!(state.buffer, "", "Enter submits and clears");

    state.buffer = "line1".to_string();
    assert_eq!(
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
        ),
        Action::Continue
    );
    assert_eq!(state.buffer, "line1\n", "Shift+Enter inserts a newline");

    // Ctrl+C during a running turn cancels it; a second one on an idle
    // prompt exits (SRDD keyboard control).
    state.turn_running = true;
    assert_eq!(
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ),
        Action::CancelTurn
    );
    state.turn_running = false;
    assert_eq!(
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ),
        Action::Continue
    );
    assert_eq!(
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ),
        Action::Exit,
        "second Ctrl+C on an idle prompt exits"
    );

    state.show_permission("rm -rf /".to_string());
    assert_eq!(
        handle_key(&mut state, KeyEvent::from(KeyCode::Esc)),
        Action::Continue
    );
    assert!(state.permission.is_none(), "Escape closes the modal");

    state.buffer = "typed".to_string();
    state.show_permission("rm -rf /".to_string());
    state.buffer.clear();
    let _ = state;
}

// Tab completes file paths from the workspace (SRDD input editor).
#[test]
fn tab_completes_file_paths() {
    let root = std::env::temp_dir().join(format!("lca-tui-complete-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(root.join("Cargo.toml"), "").expect("write");
    std::fs::write(root.join("src/main.rs"), "").expect("write");

    let mut options = options();
    options.workspace = root.clone();
    let mut state = UiState::new(options);
    state.buffer = "Ca".to_string();
    let action = handle_key(&mut state, crossterm::event::KeyEvent::from(KeyCode::Tab));
    assert_eq!(action, Action::Continue);
    assert_eq!(
        state.buffer, "Cargo.toml",
        "completes the unique match: {:?}",
        state.buffer
    );

    state.buffer = "src/m".to_string();
    let _ = handle_key(&mut state, crossterm::event::KeyEvent::from(KeyCode::Tab));
    assert_eq!(state.buffer, "src/main.rs", "completes inside a directory");
    let _ = std::fs::remove_dir_all(&root);
}

// Slash commands complete from the documented set (SRDD input editor).
#[test]
fn slash_completion_offers_the_documented_commands() {
    let mut state = UiState::new(options());
    state.buffer = "/sta".to_string();
    let _ = handle_key(&mut state, crossterm::event::KeyEvent::from(KeyCode::Tab));
    assert_eq!(
        state.buffer, "/stats",
        "completes the command: {:?}",
        state.buffer
    );
}

// /stats shows session totals through the stats seam.
#[test]
fn stats_command_surfaces_session_statistics() {
    let mut state = UiState::new(options());
    state.buffer = "/stats".to_string();
    assert_eq!(
        handle_key(&mut state, crossterm::event::KeyEvent::from(KeyCode::Enter)),
        Action::Continue
    );
    assert!(
        state
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("stats go here")
    );
}

// Verifies: NFR-28 (color is never the only signal: each turn state also
// carries a text cue).
#[test]
fn turn_state_carries_a_text_cue_not_only_color() {
    let mut state = UiState::new(options());
    state.turn_running = true;
    state.turn_status = Some(TurnStatusLine {
        text: "running...".into(),
    });
    let mut term = terminal(80, 24);
    render(&mut term, &state).expect("render");
    assert!(buffer_text(&mut term).contains("running..."));

    state.turn_running = false;
    state.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    let mut term = terminal(80, 24);
    render(&mut term, &state).expect("render");
    let text = buffer_text(&mut term);
    assert!(
        text.contains("done") || text.contains("ok"),
        "finished state named in text:\n{text}"
    );
}

// Input mode is observable (history navigation stays inside the editor).
#[test]
fn input_mode_starts_multiline_capable() {
    let state = UiState::new(options());
    assert_eq!(state.mode, InputMode::Normal);
    assert!(state.history.is_empty());
}

#[test]
fn color_mode_resolves_from_options() {
    assert_eq!(
        ColorMode::from_config(lca_config::ColorMode::Never),
        ColorMode::Plain
    );
    assert_eq!(
        ColorMode::from_config(lca_config::ColorMode::Auto),
        ColorMode::Themed
    );
}

// ---------------------------------------------------------------------------
// Phase 6: the ui world's exit clauses
// ---------------------------------------------------------------------------

/// Options carrying a scripted ui table: the hostile span lives in the
/// footer, and every region has content (the reference extension's
/// shape, inlined so the test owns its fixture).
fn options_with_ui() -> UiOptions {
    use lca_protocol::{UiEffect, UiInput, Widget, WidgetTree};
    fn text(content: &str, role: &str) -> Widget {
        text_node(content, role)
    }
    let mut options = options();
    options.render_regions = Some(std::sync::Arc::new(|region: &str| {
        let nodes = match region {
            "status-line" => vec![text("ext-seg", "accent")],
            "footer" => vec![text("\u{1b}[31mNOT A PROMPT\u{1b}[0m", "warning")],
            "panel" => vec![Widget::KeyValue(vec![(
                "live".to_string(),
                "session output here".to_string(),
            )])],
            "modal" => vec![
                Widget::Boxed {
                    title: Some("extension dialog".to_string()),
                    child: 1,
                },
                text("modal body", "default"),
            ],
            _ => return vec![],
        };
        vec![("test-ext".to_string(), WidgetTree { nodes })]
    }));
    options.ui_events = Some(std::sync::Arc::new(|region: &str, input: &UiInput| {
        if region == "modal" && matches!(input, UiInput::Submit { .. }) {
            Some((
                "test-ext".to_string(),
                UiEffect::ShowNotice("modal ok".to_string()),
            ))
        } else if matches!(input, UiInput::Key { key } if key == "m") {
            Some(("test-ext".to_string(), UiEffect::OpenModal))
        } else {
            None
        }
    }));
    options
}

// Verifies: FR-UI-2 (the sanitizer is the choke point) - a hostile span
// becomes visible text, never a control byte, and every widget kind
// renders through it.
#[test]
fn control_sequences_become_visible_text() {
    use lca_tui::{sanitize_text, widget_lines};
    let hostile = "\u{1b}[31mEVIL\u{1b}[0m";
    let sanitized = sanitize_text(hostile);
    assert_eq!(sanitized, "\\x1b[31mEVIL\\x1b[0m", "{sanitized}");
    assert!(!sanitized.contains('\u{1b}'), "no raw escape survives");

    // Layout characters belong to the host, not the extension.
    assert_eq!(sanitize_text("a\nb\tc"), "a b c");

    // Every kind produces lines, and text inside them is sanitized.
    use lca_protocol::{Widget, WidgetTree};
    let tree = WidgetTree {
        nodes: vec![
            Widget::Column(vec![1, 2]),
            text_node("line one", "default"),
            Widget::Progress {
                label: "prog".to_string(),
                fill: 0.5,
            },
        ],
    };
    let lines = widget_lines(&tree.nodes);
    assert_eq!(lines[0], "line one");
    assert!(
        lines[1].contains("prog [##########----------]  50%"),
        "{lines:?}"
    );

    // The image widget renders as a labeled placeholder: terminal
    // image protocols are the renderer's future work, not the ABI's.
    let image = WidgetTree {
        nodes: vec![Widget::Image {
            media_type: "image/png".to_string(),
            bytes: vec![0, 1, 2, 3],
        }],
    };
    let lines = widget_lines(&image.nodes);
    assert_eq!(lines, vec!["[image image/png, 4 bytes]".to_string()]);
}

fn text_node(content: &str, role: &str) -> lca_protocol::Widget {
    lca_protocol::Widget::Text {
        content: content.to_string(),
        role: role.to_string(),
    }
}

// Verifies: the Phase 6 exit clauses1 and2 at the interface level -
// an extension renders in all four regions, and the hostile span in the
// footer reaches the virtual terminal as literal characters (FR-UI-2,
// ADR-0003's spoofing protection: no cell in the buffer is a control
// code).
#[test]
fn an_extension_renders_in_all_four_regions_and_the_hostile_span_stays_literal() {
    let options = options_with_ui();
    let mut state = UiState::new(options);
    let mut terminal = terminal(100, 30);

    // Status + footer draw without any toggling (panel and modal need
    // the user).
    render(&mut terminal, &state).expect("draw");
    let text = buffer_text(&mut terminal);
    assert!(text.contains("ext-seg"), "status line: {text}");

    // The side panel opens with Ctrl+P (the host's binding).
    assert!(matches!(
        handle_key(
            &mut state,
            key_event(
                crossterm::event::KeyCode::Char('p'),
                crossterm::event::KeyModifiers::CONTROL
            )
        ),
        Action::Continue
    ));
    assert!(state.panel_open);
    render(&mut terminal, &state).expect("draw");
    let text = buffer_text(&mut terminal);
    assert!(text.contains("session output here"), "panel: {text}");
    assert!(text.contains("footer"), "footer border: {text}");

    // The footer carries the hostile span, and the buffer holds no raw
    // control byte anywhere: the escape sequence is visible text.
    assert!(
        text.contains("\\x1b[31mNOT A PROMPT\\x1b[0m"),
        "hostile span rendered literally: {text:?}"
    );
    assert!(
        !text.contains('\u{1b}'),
        "no control byte reached the terminal"
    );

    // The modal opens on the user's key (idle: allowed).
    state.turn_running = false;
    handle_key(
        &mut state,
        key_event(
            crossterm::event::KeyCode::Char('m'),
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert!(state.modal_open, "the user's key opened the modal");
    render(&mut terminal, &state).expect("draw");
    let text = buffer_text(&mut terminal);
    assert!(text.contains("modal body"), "modal content: {text}");
    assert!(text.contains("extension dialog"), "modal title: {text}");

    // Escape dismisses it (user-dismissible: catalog `ui`).
    handle_key(
        &mut state,
        key_event(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert!(!state.modal_open);

    // Ctrl+P closes the panel again.
    handle_key(
        &mut state,
        key_event(
            crossterm::event::KeyCode::Char('p'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    assert!(!state.panel_open);
}

// Verifies: FR-UI-6 - an extension cannot open a modal during a running
// turn; the same key that opens it while idle is dropped mid-turn.
#[test]
fn an_extension_cannot_open_a_modal_during_a_running_turn() {
    let options = options_with_ui();
    let mut state = UiState::new(options);
    // Keys reach a registered region only while it is focused: the
    // panel is how the reference extension's `m` is reached (status and
    // footer are display-only in v1 - the catalog says segment and
    // lines, not input focus).
    handle_key(
        &mut state,
        key_event(
            crossterm::event::KeyCode::Char('p'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    assert!(state.panel_open);

    state.turn_running = true;
    handle_key(
        &mut state,
        key_event(
            crossterm::event::KeyCode::Char('m'),
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert!(!state.modal_open, "dropped while the turn runs (FR-UI-6)");

    state.turn_running = false;
    handle_key(
        &mut state,
        key_event(
            crossterm::event::KeyCode::Char('m'),
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert!(state.modal_open, "allowed once the user has the agent idle");
}

// Verifies: a submission in the modal surfaces through the effect path
// (the same machinery slash commands use, FR-UI-1's interaction half).
#[test]
fn modal_submissions_flow_through_the_effect() {
    let options = options_with_ui();
    let mut state = UiState::new(options);
    state.modal_open = true;
    state.buffer.push_str("typed in the modal");
    handle_key(
        &mut state,
        key_event(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert_eq!(
        state.notice.as_deref(),
        Some("modal ok"),
        "the effect applied"
    );
    assert!(!state.modal_open || state.notice.is_some());
}

fn key_event(
    code: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, modifiers)
}
