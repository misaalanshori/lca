//! Startup keybinding validation (gh #66), split from `run.rs` under the
//! 1,200-line ceiling when gh #400's early-input adoption grew it.
//!
//! The rule is that a bad `keybindings.toml` is loud from the first frame: an
//! unknown action, an invalid key, or a conflict keeps the defaults for that
//! binding and says so, rather than silently doing nothing.

/// Everything wrong with the loaded keybindings, as one notice, or `None`.
#[must_use]
pub(crate) fn keybinding_problems(
    load_error: Option<String>,
    unknown: &[String],
    invalid: &[(String, String)],
    conflicts: &[lca_tui::engine::keybindings::KeybindingConflict],
) -> Option<String> {
    let mut problems = Vec::new();
    if let Some(err) = load_error {
        problems.push(err);
    }
    if !unknown.is_empty() {
        problems.push(format!(
            "unknown action(s): {} - keeping defaults for those",
            unknown.join(", ")
        ));
    }
    for (action, key) in invalid {
        problems.push(format!(
            "invalid key {key} for {action} - keeping defaults for it"
        ));
    }
    for conflict in conflicts {
        problems.push(format!(
            "key {} claimed by {} - first match wins",
            conflict.key,
            conflict.keybindings.join(", ")
        ));
    }
    if problems.is_empty() {
        return None;
    }
    Some(format!("keybindings.toml: {}", problems.join("; ")))
}
