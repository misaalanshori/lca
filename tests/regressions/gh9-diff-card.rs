//! gh #9 (EFG-016's headline, EFG-014's display half): an `edit` tool
//! result rendered as an ordinary tool card - the diff the tool now
//! returns was carried in `extras` and dropped before it reached the
//! transcript. This guards the whole wire: the structured `diff` in the
//! `ToolResult`, through `Chat`'s turn-event handling, into the framed
//! card with pi's two diff roles.
//!
//! Verifies: FR-UI-2 (the transcript renders what a turn produced).

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
            dialog_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("openai-compatible/mimo".to_string())),
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
            // Colored on purpose: the rows below assert the diff roles
            // through their raw SGR (the plain theme emits none).
            plain: false,
            invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::new(),
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
        },
        Arc::new(KeybindingsManager::new()),
    )
}

fn call() -> ToolCall {
    ToolCall {
        call_id: "c1".to_string(),
        name: "edit".to_string(),
        arguments: r#"{"path":"src/main.rs"}"#.to_string(),
    }
}

// The card an edit result draws: title row, then the diff's own lines in
// the two roles - carried by the result's structured `extras`, never by
// re-parsing display text.
#[test]
fn an_edit_results_structured_diff_reaches_the_card() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ToolStarted(call()));
    let mut result = ToolResult::ok("c1", "Successfully replaced 1 block(s) in src/main.rs.");
    result.extras.insert(
        "diff".to_string(),
        "--- src/main.rs\n+++ src/main.rs\n@@ -1,3 +1,3 @@\n fn a() {}\n-fn b() {}\n+fn bb() {}\n fn c() {}\n"
            .to_string(),
    );
    chat.on_turn_event(TurnEvent::ToolFinished(result));

    let rows = chat.render(100);
    let joined = rows.join("\n");
    let plain = strip_terminal_sequences(&joined);
    assert!(
        plain.contains("-fn b() {}") && plain.contains("+fn bb() {}"),
        "both changed lines render:\n{plain}"
    );
    assert!(
        plain.contains("Successfully replaced 1 block(s) in src/main.rs."),
        "the human summary stays on the card:\n{plain}"
    );
    assert!(
        rows.iter().any(|row| row.contains("38;2;204;102;102")),
        "the removed line carries toolDiffRemoved (raw SGR):\n{joined}"
    );
    assert!(
        rows.iter().any(|row| row.contains("38;2;181;189;104")),
        "the added line carries toolDiffAdded (raw SGR):\n{joined}"
    );
}

// No diff, no diff card: a result without the structured entry renders
// exactly as every other tool result does (the no-regression half).
#[test]
fn a_tool_result_without_a_diff_renders_as_an_ordinary_card() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ToolStarted(call()));
    chat.on_turn_event(TurnEvent::ToolFinished(ToolResult::ok(
        "c1",
        "Successfully replaced 2 block(s) in src/main.rs.",
    )));
    let joined = chat.render(100).join("\n");
    let plain = strip_terminal_sequences(&joined);
    assert!(
        plain.contains("edit src/main.rs") && plain.contains("ok"),
        "the ordinary card renders as it always did:\n{plain}"
    );
    // No diff text, and no removed-role row: the card's own green is
    // the `ok` status (the same value as toolDiffAdded), so the check
    // is on what the card shows, not on a color it shares.
    assert!(
        !plain.contains("--- src/main.rs") && !plain.contains("-fn b() {}"),
        "a result with no structured diff renders no diff:\n{plain}"
    );
    assert!(
        !joined.contains("38;2;204;102;102"),
        "no toolDiffRemoved row on a card with no diff:\n{joined}"
    );
}
