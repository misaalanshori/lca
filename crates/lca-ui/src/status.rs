//! Tab-badge sync (gh #213, OSC 7501): the lifecycle mirror TurnState
//! drives every frame, split from `run.rs` for the file ceiling.
//!
//! Behavior unchanged: a waiting modal reports blocked, a running turn
//! working, otherwise the last outcome rests (done/error) or the editor
//! idles. Emits on change only; the terminal suppresses when unsupported.

use lca_tui::engine::program_status::ProgramState;
use lca_tui::engine::terminal::Terminal;

use crate::chat::Chat;

/// Every modal that means "the agent is waiting on you" (gh #369).
///
/// One table, so the badge rule and any surface that lists these
/// (docs, a `/hotkeys`-style summary) agree by construction rather than by
/// two hand-kept lists drifting apart — the house lesson from the earlier
/// vocabulary work. `world_blocked` is checked against this list by a test,
/// so adding a modal without adding it here fails.
pub const BLOCKED_MODALS: &[&str] = &[
    "permission",
    "secret",
    "picker",
    "grant",
    "switch_confirm",
    // Gh #411: the `/bug` inspection screen. Nothing is written until the user
    // confirms, so it is a wait on the user like any other prompt.
    "bug_review",
    "dialog",
    "modal_open",
    "login_waiting",
    // Gh #369: the `/extensions` install-consent prompt. It is a wait on the
    // user exactly like a permission prompt, and was the one omission.
    "install_consent",
];

/// Which kind of work the agent is doing (gh #369). All three map to
/// `working`, but the distinction is what lets a caller/tests name the
/// source, and it documents that compaction and shell runs are work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkKind {
    /// Nothing running.
    Idle,
    /// A submitted turn (generation or a tool).
    Turn,
    /// A background `/compact` summarization.
    Compact,
    /// An inline `!`/`!!` shell command.
    Shell,
}

impl WorkKind {
    /// Whether anything is running.
    #[must_use]
    pub fn is_working(self) -> bool {
        !matches!(self, WorkKind::Idle)
    }
}

/// Classify what is running, with a turn outranking a concurrent compaction
/// (both read `working`; the turn is what the user is watching).
#[must_use]
pub fn work_state(turn_running: bool, compacting: bool, shell_running: bool) -> WorkKind {
    if turn_running {
        WorkKind::Turn
    } else if compacting {
        WorkKind::Compact
    } else if shell_running {
        WorkKind::Shell
    } else {
        WorkKind::Idle
    }
}

/// Whether a modal waits for the user: any open prompt, picker,
/// confirm, dialog, panel modal, background login step, or the extension
/// install-consent prompt (gh #369). Each clause is named in
/// [`BLOCKED_MODALS`].
pub fn world_blocked(chat: &Chat) -> bool {
    worlds(chat).iter().any(|(_, open)| *open)
}

/// One `(name, open)` row per entry in [`BLOCKED_MODALS`], in table order.
///
/// The table drives the loop, so a modal added to the vocabulary without a
/// reader here is caught by `every_blocked_modal_has_a_reader` rather than
/// silently not counting. Each arm reads one field of the chat state.
#[must_use]
pub fn worlds(chat: &Chat) -> Vec<(&'static str, bool)> {
    BLOCKED_MODALS
        .iter()
        .map(|&name| {
            let open = match name {
                "permission" => chat.world.permission.is_some(),
                "secret" => chat.world.secret.is_some(),
                "picker" => chat.world.picker.is_some(),
                "grant" => chat.world.grant.is_some(),
                "switch_confirm" => chat.world.switch_confirm.is_some(),
                "bug_review" => chat.world.bug_review.is_some(),
                "dialog" => chat.world.dialog.is_some(),
                "modal_open" => chat.world.modal_open,
                "login_waiting" => chat.world.login_waiting.is_some(),
                "install_consent" => chat.install_consent.is_some(),
                // Unreachable while the table and this match agree, which a
                // test enforces; an unknown name reads as "not open" so a
                // future addition cannot panic a render.
                _ => false,
            };
            (name, open)
        })
        .collect()
}

/// The badge state for this frame: blocked beats working beats resting.
pub fn badge_state(blocked: bool, working: bool, last_ok: Option<bool>) -> ProgramState {
    if blocked {
        ProgramState::Blocked
    } else if working {
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
///
/// Gh #369: `working` is the unified condition, so a background `/compact`
/// and a `!`/`!!` run badge `working` like a turn does.
pub fn sync_badge(
    chat: &Chat,
    working: bool,
    last_ok: Option<bool>,
    last: &mut Option<ProgramState>,
    terminal: &mut dyn Terminal,
) {
    let state = badge_state(world_blocked(chat), working, last_ok);
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

    // Verifies: gh #369 - the unified "the agent is busy" condition. A
    // background `/compact` and a `!`/`!!` shell run are work the user
    // started and is waiting on, so a tab bar must badge them `working` too;
    // before this, only a turn did.
    #[test]
    fn compaction_and_shell_runs_count_as_working() {
        assert_eq!(work_state(true, false, false), WorkKind::Turn);
        assert_eq!(work_state(false, true, false), WorkKind::Compact);
        assert_eq!(work_state(false, false, true), WorkKind::Shell);
        assert_eq!(work_state(false, false, false), WorkKind::Idle);
        // Blocked is decided separately and outranks all of these.
        assert_eq!(badge_state(true, true, None), ProgramState::Blocked);
    }

    // Verifies: gh #369 - a turn outranks a concurrent compaction for the
    // badge, because both map to `working` and the turn is the thing the
    // user is actually watching.
    #[test]
    fn a_turn_outranks_a_concurrent_compaction() {
        assert_eq!(work_state(true, true, true), WorkKind::Turn);
    }

    // Verifies: gh #369 - the extension install-consent modal is a wait on
    // the user, so the tab must badge `blocked`. It was the one modal the
    // `world_blocked` list omitted, which is why an install prompt in
    // another window looked like the agent was still working.
    #[test]
    fn install_consent_counts_as_blocked() {
        assert!(
            BLOCKED_MODALS.contains(&"install_consent"),
            "the vocabulary names the install-consent modal"
        );
        // Behavioural, not just vocabulary: open the modal and assert the
        // badge rule sees it. This is what the issue's gap 3 is about - an
        // install prompt in another window read as "still working".
        let mut chat = crate::run::tests::chat();
        assert!(!world_blocked(&chat), "nothing open, nothing blocked");
        chat.install_consent = Some(crate::chat_hub::InstallConsent {
            name: "demo".to_string(),
            version: "1.0.0".to_string(),
            publisher: "Official (misaalanshori/lca)".to_string(),
            source_url: String::new(),
            artifact_ref: String::new(),
            capabilities: Vec::new(),
        });
        assert!(
            world_blocked(&chat),
            "the install-consent modal waits on the user"
        );
        assert_eq!(
            badge_state(world_blocked(&chat), true, None),
            ProgramState::Blocked,
            "and it outranks a running turn"
        );
    }

    // Verifies: gh #369 - the table is the single source. Every name in it
    // has a reader in `worlds`, so adding a modal to the vocabulary without
    // wiring it fails here instead of silently never counting as blocked.
    // Pinned at the source level because a "no arm matched" case is
    // indistinguishable from "the modal is closed" at runtime.
    #[test]
    fn every_blocked_modal_has_a_reader() {
        let source = include_str!("status.rs");
        for modal in BLOCKED_MODALS {
            assert!(
                source.contains(&format!("\"{modal}\" =>")),
                "`{modal}` is in BLOCKED_MODALS but has no reader in `worlds`"
            );
        }
    }
}
