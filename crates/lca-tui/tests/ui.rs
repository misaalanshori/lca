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
        stats: std::sync::Arc::new(|| "stats go here".to_string()),
        workspace: std::path::PathBuf::new(),
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

// The status line shows the model, plus live turn state (SRDD status line).
#[test]
fn status_line_names_the_model_and_turn_state() {
    let mut state = UiState::new(options());
    state.on_turn_event(TurnEvent::Usage(lca_protocol::Usage {
        input: 1200,
        output: 40,
        cache_read: 1000,
        ..Default::default()
    }));
    let mut term = terminal(80, 24);
    render(&mut term, &state).expect("render");
    let text = buffer_text(&mut term);
    assert!(text.contains("fake/faux-1"), "model shown:\n{text}");
    assert!(text.contains("1200"), "token counts shown: {text}");
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
