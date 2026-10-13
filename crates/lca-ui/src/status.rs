//! Tab-badge sync (gh #213, OSC 7501): the lifecycle mirror TurnState
//! drives every frame, split from `run.rs` for the file ceiling.
//!
//! Behavior unchanged: a waiting modal reports blocked, a running turn
//! working, otherwise the last outcome rests (done/error) or the editor
//! idles. Emits on change only; the terminal suppresses when unsupported.

use lca_tui::engine::program_status::ProgramState;
use lca_tui::engine::terminal::Terminal;

use crate::chat::Chat;

/// Whether a modal waits for the user: any open prompt, picker,
/// confirm, dialog, panel modal, or background login step.
pub fn world_blocked(chat: &Chat) -> bool {
    let world = &chat.world;
    world.permission.is_some()
        || world.secret.is_some()
        || world.picker.is_some()
        || world.grant.is_some()
        || world.switch_confirm.is_some()
        || world.dialog.is_some()
        || world.modal_open
        || world.login_waiting.is_some()
}

/// The badge state for this frame: blocked beats running beats resting.
pub fn badge_state(blocked: bool, turn_running: bool, last_ok: Option<bool>) -> ProgramState {
    if blocked {
        ProgramState::Blocked
    } else if turn_running {
        ProgramState::Working
    } else {
        match last_ok {
            Some(true) => ProgramState::Done,
            Some(false) => ProgramState::Error,
            None => ProgramState::Idle,
        }
    }
}

/// Mirror the lifecycle to the tab badge, emitting on change only.
/// `last` is the previously reported state and updates in place.
pub fn sync_badge(
    chat: &Chat,
    turn_running: bool,
    last_ok: Option<bool>,
    last: &mut Option<ProgramState>,
    terminal: &mut dyn Terminal,
) {
    let state = badge_state(world_blocked(chat), turn_running, last_ok);
    if *last != Some(state) {
        terminal.set_program_status(state);
        *last = Some(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #213 - blocked beats running beats resting.
    #[test]
    fn badge_precedence_is_blocked_running_resting() {
        assert_eq!(badge_state(true, true, Some(true)), ProgramState::Blocked);
        assert_eq!(badge_state(false, true, Some(true)), ProgramState::Working);
        assert_eq!(badge_state(false, false, Some(true)), ProgramState::Done);
        assert_eq!(badge_state(false, false, Some(false)), ProgramState::Error);
        assert_eq!(badge_state(false, false, None), ProgramState::Idle);
    }
}
