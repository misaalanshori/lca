//! The agent interface's UI requirements, re-expressed against the new
//! engine after the legacy ratatui render tests were retired.
//!
//! Verifies: FR-UI-1, FR-UI-3, FR-UI-4, FR-UI-5, NFR-26, NFR-27, NFR-28.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::{CommandEffect, Widget, WidgetTree};
use lca_tui::engine::core::CURSOR_MARKER;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::{strip_terminal_sequences, visible_width};
use lca_ui::Chat;
use lca_ui::state::{UiOptions, widget_lines};
use lca_ui::theme::Theme;

fn options(plain: bool) -> UiOptions {
    UiOptions {
        prompt_slot: Default::default(),
        pending_models: None,
        model_label: Arc::new(Mutex::new("p/m".into())),
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
        plain,
        invoke_command: Arc::new(|_, _| CommandEffect::None),
        slash_commands: Vec::new(),
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
    }
}

fn chat(plain: bool) -> Chat {
    Chat::new(options(plain), Arc::new(KeybindingsManager::new()))
}

fn strip(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| strip_terminal_sequences(l)).collect()
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
    let lines = widget_lines(&tree.nodes, &Theme::colored());
    assert!(lines.iter().any(|l| l.contains("status")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("k") && l.contains("v")),
        "{lines:?}"
    );
}

// Verifies: FR-UI-3 - a resize re-renders without losing the transcript.
#[test]
fn resize_keeps_the_transcript() {
    let mut chat = chat(true);
    chat.transcript.push_user("an earlier turn");
    let before = strip(&chat.render(80));
    assert!(before.iter().any(|l| l.contains("an earlier turn")));
    chat.world.resize(120, 40);
    let after = strip(&chat.render(120));
    assert!(
        after.iter().any(|l| l.contains("an earlier turn")),
        "the transcript survived the resize"
    );
}

// Verifies: FR-UI-4 - the approval prompt shows the exact command or path.
#[test]
fn permission_modal_shows_the_exact_command() {
    let mut chat = chat(true);
    chat.world
        .show_permission("shell: rm -rf /tmp/x --force".into());
    let viewport = strip(&chat.viewport(100, 30, 0));
    assert!(
        viewport.iter().any(|l| l.contains("rm -rf /tmp/x --force")),
        "the exact command is shown:\n{}",
        viewport.join("\n")
    );
}

// Verifies: FR-UI-5 - a terminal without color renders plain text only.
#[test]
fn plain_mode_never_paints_color() {
    let mut chat = chat(true);
    chat.transcript.push_user("a question");
    chat.transcript.append_text("an answer");
    chat.world.notice = Some("a notice".into());
    for line in chat.render(80) {
        // The CURSOR_MARKER is the renderer's side channel, not color.
        let cleaned = line.replace(CURSOR_MARKER, "");
        assert!(
            !cleaned.contains('\x1b'),
            "plain mode emitted an escape: {cleaned:?}"
        );
    }
}

// Verifies: NFR-26 - the interface works at 80 columns.
#[test]
fn renders_at_eighty_columns() {
    let mut chat = chat(false);
    chat.transcript
        .push_user("a fairly long answer line that must wrap within the width");
    for line in chat.render(80) {
        assert!(visible_width(&line) <= 80, "over 80 columns: {line:?}");
    }
}

// Verifies: NFR-27 - the interface functions without mouse input (the
// keyboard path alone drives editing and submission).
#[test]
fn keyboard_alone_edits_and_submits() {
    let mut chat = chat(true);
    for c in "hello".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\x7f"); // backspace
    chat.handle_key("p");
    assert_eq!(chat.editor.text(), "hellp");
    let action = chat.handle_key("\r");
    assert_eq!(action, lca_ui::Action::Submit);
}

// Verifies: NFR-28 - color is never the only signal; every colored state
// carries a text cue too.
#[test]
fn state_carries_a_text_cue_not_only_color() {
    let mut chat = chat(false);
    chat.turn_running = true;
    chat.turn_status = Some(lca_ui::state::TurnStatusLine {
        text: "running...".into(),
    });
    let lines = strip(&chat.render(100));
    let status = lines
        .iter()
        .find(|l| l.contains("running..."))
        .expect("cue");
    assert!(status.contains("running..."));
}
