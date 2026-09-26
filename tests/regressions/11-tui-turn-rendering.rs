//! Released TUI rendering defects (0.1.x): a reasoning model's reasoning was
//! streamed into the answer buffer with no separator, and a finished tool
//! call printed the provider's opaque call id instead of the tool name and
//! argument. Both are visible on every reasoning-model turn.
//!
//! The same rendering is also guarded at the buffer level by
//! `crates/lca-tui/tests/ui.rs::reasoning_is_separated_from_the_answer` and
//! `::tool_lines_name_the_tool_not_the_call_id`; this file keeps the
//! released-defect guard in the named regression set.
//!
//! Verifies: FR-UI-2.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::{ToolCall, ToolResult, TurnEvent};
use lca_tui::{UiOptions, UiState};

fn state() -> UiState {
    UiState::new(UiOptions {
        model_label: Arc::new(Mutex::new("fake/faux-1".to_string())),
        initial_lines: Vec::new(),
        plain: false,
        invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
        slash_commands: Vec::new(),
        workspace: PathBuf::new(),
        render_regions: None,
        ui_events: None,
        update_notice: None,
        login: None,
        complete_login: None,
        pick_login: None,
        confirm_login_grant: None,
    })
}

fn transcript(state: &UiState) -> String {
    let mut text = state.scrollback.join("\n");
    if !text.is_empty() && !state.active.is_empty() {
        text.push('\n');
    }
    text.push_str(&state.active);
    text
}

#[test]
fn reasoning_is_set_off_from_the_answer() {
    let mut state = state();
    state.on_turn_event(TurnEvent::ReasoningDelta("Let me think about it.".into()));
    state.on_turn_event(TurnEvent::TextDelta("The answer is 42.".into()));
    let text = transcript(&state);
    assert!(text.contains("The answer is 42."), "{text}");
    assert!(
        !text.contains("about it.The answer"),
        "reasoning glued to the answer:\n{text}"
    );
}

#[test]
fn a_finished_tool_call_names_the_tool_not_the_call_id() {
    let mut state = state();
    state.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "call-abc123".into(),
        name: "read".into(),
        arguments: "{\"path\":\"stats.py\"}".into(),
    }));
    state.on_turn_event(TurnEvent::ToolFinished(ToolResult::ok(
        "call-abc123",
        "body",
    )));
    let text = transcript(&state);
    assert!(text.contains("read"), "tool name shown:\n{text}");
    assert!(text.contains("stats.py"), "argument shown:\n{text}");
    assert!(!text.contains("abc123"), "opaque call id:\n{text}");
}
