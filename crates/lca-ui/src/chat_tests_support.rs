//! The fixtures `lca-ui`'s `Chat` rows share (gh #574's leftover, #575b).
//!
//! `chat_tests.rs` sat three lines from gate 11's ceiling as a *declared test
//! module* (`chat.rs` holds `#[path = "chat_tests.rs"] pub(crate) mod tests`),
//! so its rows report as `chat::tests::<row>` and neither a `#[path]` sibling
//! nor a nested `mod` can move a row without renaming it (#574's lesson). The
//! fixtures are not rows: nothing in the reported test set names them, so they
//! live here and `chat_tests.rs` re-exports them under the paths every sibling
//! file already imports (`use super::{chat, options, strip, tree_row}`).
//!
//! Only fixtures that a *sibling* file imports are here; the two helpers used
//! by this file alone (`type_prompt`, `caret_row`) stay where they are.

use super::*;
use lca_protocol::CommandEffect;
use lca_tui::engine::text::strip_terminal_sequences;
use std::path::PathBuf;

/// One structured tree row for navigator tests (gh #231): a user
/// row at `depth` unless the kind says otherwise.
pub(super) fn tree_row(id: &str, depth: usize, text: &str) -> crate::state::TreeRow {
    crate::state::TreeRow {
        id: id.to_string(),
        depth,
        kind: crate::state::TreeRowKind::User,
        text: text.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    }
}

pub(crate) fn options() -> UiOptions {
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
        initial_header: (Vec::new(), Vec::new()),
        initial_records: Vec::new(),
        initial_tail_lines: Vec::new(),
        initial_messages: Vec::new(),
        open_resume_picker: false,
        yolo: false,
        early_input: None,
        thinking_visibility: Default::default(),
        codeblock_border: Default::default(),
        plain: true,
        invoke_command: Arc::new(|_, _| CommandEffect::None),
        slash_commands: vec!["/help".into(), "/model".into(), "/login".into()],
        models: vec![
            crate::ModelRow::from(("alpha".into(), "alpha".into())),
            crate::ModelRow::from(("beta".into(), "beta".into())),
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
        follow_up_mode: lca_protocol::QueueMode::OneAtATime,
    }
}

pub(super) fn chat() -> Chat {
    Chat::new(options(), Arc::new(KeybindingsManager::new()))
}

/// A chat whose slash commands reach `drained`, for rows that assert
/// what *dispatched* rather than what rendered (gh #401's compaction
/// flush, which runs a leading queued command before the turn).
pub(super) fn chat_with_commands(
    drained: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
) -> Chat {
    let mut options = options();
    options.slash_commands = vec!["/inspect".into()];
    options.invoke_command = std::sync::Arc::new(move |name, argument| {
        drained
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((name.to_string(), argument.to_string()));
        CommandEffect::None
    });
    Chat::new(options, Arc::new(KeybindingsManager::new()))
}

pub(super) fn strip(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| strip_terminal_sequences(l)).collect()
}
