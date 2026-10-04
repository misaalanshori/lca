//! Released TUI rendering defects (0.1.x): a reasoning model's reasoning was
//! streamed into the answer buffer with no separator, and a finished tool
//! call printed the provider's opaque call id instead of the tool name and
//! argument. Both are visible on every reasoning-model turn.
//!
//! The same rendering is also guarded at the buffer level by
//! `crates/lca-ui/src/transcript.rs` and `crates/lca-ui/src/chat.rs`; this
//! file keeps the released-defect guard in the named regression set.
//!
//! Verifies: FR-UI-2.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::{ToolCall, ToolResult, TurnEvent};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::{Chat, UiOptions};

fn chat() -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("fake/faux-1".to_string())),
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
            yolo: false,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
            plain: true,
            invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::new(),
            render_regions: None,
            ui_events: None,
            update_notice: None,
            login: None,
            complete_login: None,
            pick_login: None,
            confirm_login_grant: None,
            hooks: lca_ui::UiHooks::default(),
            fullscreen: true,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

fn transcript(chat: &Chat) -> String {
    chat.render(100)
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn reasoning_is_set_off_from_the_answer() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ReasoningDelta("Let me think about it.".into()));
    chat.on_turn_event(TurnEvent::TextDelta("The answer is 42.".into()));
    let text = transcript(&chat);
    assert!(text.contains("The answer is 42."), "{text}");
    assert!(
        !text.contains("about it.The answer"),
        "reasoning glued to the answer:\n{text}"
    );
}

#[test]
fn a_finished_tool_call_names_the_tool_not_the_call_id() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "call-abc123".into(),
        name: "read".into(),
        arguments: "{\"path\":\"stats.py\"}".into(),
    }));
    chat.on_turn_event(TurnEvent::ToolFinished(ToolResult::ok(
        "call-abc123",
        "body",
    )));
    let text = transcript(&chat);
    assert!(text.contains("read"), "tool name shown:\n{text}");
    assert!(text.contains("stats.py"), "argument shown:\n{text}");
    assert!(!text.contains("abc123"), "opaque call id:\n{text}");
}
