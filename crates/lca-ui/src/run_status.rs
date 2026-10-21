//! `TurnState`'s lifecycle → ambient-channel sync (gh #213, gh #369),
//! split from `run.rs` under the workspace's 1,200-line ceiling (gate 11).
//!
//! One `WorkKind` feeds both channels so the tab badge and the taskbar
//! spinner cannot disagree about what "busy" means: a turn, a background
//! `/compact`, and an inline `!`/`!!` run are all work the user is waiting
//! on.

use lca_tui::engine::terminal::Terminal;

use crate::chat::Chat;
use crate::run::TurnState;

impl TurnState {
    /// Mirror the lifecycle to the tab badge and the taskbar indicator
    /// (gh #213, gh #369).
    ///
    /// Both channels read one [`crate::status::WorkKind`], so a background
    /// `/compact` and a `!`/`!!` run are `working` and drive `OSC 9;4`
    /// exactly as a turn does. The progress gate is the existing
    /// `show_progress` tuning key.
    pub(crate) fn sync_program_status(&mut self, chat: &Chat, terminal: &mut dyn Terminal) {
        let work =
            crate::status::work_state(chat.turn_running, chat.compacting, chat.shell_running());
        crate::status::sync_badge(
            chat,
            work.is_working(),
            self.last_ok,
            &mut self.program_status,
            terminal,
        );
        self.sync_progress(work, chat, terminal);
    }

    /// Drive the taskbar spinner from the same [`crate::status::WorkKind`]
    /// (gh #369).
    ///
    /// `set_progress` is only written on a change of the boolean, so the
    /// keepalive (the terminal's own tick) is what keeps it alive; this
    /// method must not re-emit per frame.
    fn sync_progress(
        &mut self,
        work: crate::status::WorkKind,
        chat: &Chat,
        terminal: &mut dyn Terminal,
    ) {
        let active = work.is_working() && chat.display_tuning().show_progress;
        if self.progress_active != active {
            terminal.set_progress(active);
            self.progress_active = active;
        }
    }
}
