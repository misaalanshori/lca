//! The agent interface's UI requirements, re-expressed against the new
//! engine after the legacy ratatui render tests were retired.
//!
//! Verifies: FR-UI-1, FR-UI-3, FR-UI-4, FR-UI-5, NFR-26, NFR-27, NFR-28.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lca_protocol::{CommandEffect, Widget, WidgetTree};
use lca_ui::render_state;
use lca_ui::state::{PermissionModal, UiOptions, UiState, handle_key, widget_lines};

fn state(plain: bool) -> UiState {
    UiState::new(UiOptions {
        model_label: Arc::new(Mutex::new("p/m".into())),
        initial_lines: Vec::new(),
        plain,
        invoke_command: Arc::new(|_, _| CommandEffect::None),
        slash_commands: Vec::new(),
        models: Vec::new(),
        workspace: PathBuf::from("."),
        render_regions: None,
        ui_events: None,
        update_notice: None,
        login: None,
        complete_login: None,
        pick_login: None,
        confirm_login_grant: None,
    })
}

fn visible_width(s: &str) -> usize {
    lca_tui::engine::text::visible_width(s)
}

fn strip(s: &str) -> String {
    lca_tui::engine::text::strip_terminal_sequences(s)
}

// Verifies: FR-UI-1 - an extension's declarative widget tree renders to
// host-drawn lines (the host asks the registry, never the other way).
#[test]
fn extension_widget_trees_render_through_the_host() {
    let tree = WidgetTree {
        nodes: vec![
            Widget::Column(vec![1, 2]),
            Widget::Text {
                content: "status".into(),
                role: "accent".into(),
            },
            Widget::KeyValue(vec![("k".into(), "v".into())]),
        ],
    };
    let lines = widget_lines(&tree.nodes);
    assert!(lines.iter().any(|l| l.contains("status")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("k") && l.contains("v")),
        "{lines:?}"
    );
}

// Verifies: FR-UI-3 - a resize re-renders without losing scrollback.
#[test]
fn resize_keeps_the_scrollback() {
    let mut s = state(true);
    s.scrollback.push("an earlier turn".into());
    let before = render_state(&s, 80, 24);
    assert!(before.iter().any(|l| l.contains("an earlier turn")));
    s.resize(120, 40);
    let after = render_state(&s, 120, 40);
    assert!(
        after.iter().any(|l| l.contains("an earlier turn")),
        "the transcript survived the resize"
    );
}

// Verifies: FR-UI-4 - the approval prompt shows the exact command or path.
#[test]
fn permission_modal_shows_the_exact_command() {
    let mut s = state(true);
    s.permission = Some(PermissionModal {
        action: "shell: rm -rf /tmp/x --force".into(),
        respond: None,
    });
    let lines = render_state(&s, 100, 30);
    assert!(
        lines.iter().any(|l| l.contains("rm -rf /tmp/x --force")),
        "the exact command is shown:\n{}",
        lines.join("\n")
    );
}

// Verifies: FR-UI-5 - a terminal without color renders plain text only.
#[test]
fn plain_mode_never_paints_color() {
    let mut s = state(true);
    s.scrollback.push("an answer".into());
    s.notice = Some("a notice".into());
    let lines = render_state(&s, 80, 24);
    for line in &lines {
        // The CURSOR_MARKER is the renderer's side channel, not color.
        let cleaned = line.replace(lca_tui::engine::core::CURSOR_MARKER, "");
        assert!(
            !cleaned.contains('\x1b'),
            "plain mode emitted an escape: {cleaned:?}"
        );
    }
}

// Verifies: NFR-26 - the interface works at 80 columns.
#[test]
fn renders_at_eighty_columns() {
    let mut s = state(false);
    s.scrollback
        .push("a fairly long answer line that must wrap within the width".into());
    let lines = render_state(&s, 80, 24);
    for line in &lines {
        assert!(visible_width(line) <= 80, "over 80 columns: {line:?}");
    }
}

// Verifies: NFR-27 - the interface functions without mouse input (the
// keyboard path alone drives editing and submission).
#[test]
fn keyboard_alone_edits_and_submits() {
    let mut s = state(true);
    for c in "hello".chars() {
        handle_key(&mut s, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    handle_key(
        &mut s,
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
    );
    handle_key(
        &mut s,
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
    );
    assert_eq!(s.buffer, "hellp");
    let action = handle_key(&mut s, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(action, lca_ui::Action::Submit);
}

// Verifies: NFR-28 - color is never the only signal; every colored state
// carries a text cue too.
#[test]
fn state_carries_a_text_cue_not_only_color() {
    let mut s = state(false);
    s.turn_running = true;
    s.turn_status = Some(lca_ui::state::TurnStatusLine {
        text: "running...".into(),
    });
    let lines = render_state(&s, 100, 24);
    let status = lines
        .iter()
        .find(|l| l.contains("running..."))
        .expect("cue");
    // The cue is literal text, present with or without the color code.
    assert!(strip(status).contains("running..."));
}
