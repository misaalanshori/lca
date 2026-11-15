//! Gh #601: the branch-summary verb's interface half - the poll that paints the
//! separator and posts the outcome.
//!
//! Split out of `chat_session_commands.rs` because it is a *background verb's*
//! lifecycle (start, running state, result), not session navigation: the same
//! reason `poll_compact` lives beside the command that starts it.
//!
//! # Why the interface polls instead of receiving an event
//!
//! The `/tree` navigator runs on the interface thread, and the summary is one
//! provider call on a host thread. A `TurnEvent` pair cannot carry it: events
//! are the *turn* loop's vocabulary, and the tree key never enters a turn. So the
//! shape is `/compact`'s, verbatim - the host hook spawns and returns, the
//! interface paints the running state, and a poll hands the result over once.

use super::chat::Chat;

impl Chat {
    /// Start a branch summary for the abandoned tip `record_id` (gh #601).
    ///
    /// Returns **`true` when the work actually started** (the caller's "then
    /// leave" must not run otherwise). Every refusal posts its own line, so the
    /// frame still changes; the loop repaints on the notice either way.
    ///
    /// - the host refuses to start (no provider, no strategy): the reason is
    ///   posted **now**, and nothing claims to be running;
    /// - the host starts it: the separator says `Summarizing branch` before the
    ///   answer exists, which is the whole point of the poll;
    /// - the host cannot do it at all (no hook): the interface says so.
    pub(crate) fn start_branch_summary(&mut self, record_id: &str) -> bool {
        if self.summarizing_branch {
            self.say("a branch summary is already running".to_string());
            return false;
        }
        let Some(start) = self.world.options.hooks.start_branch_summary.clone() else {
            self.say("branch summaries are not available in this host".to_string());
            return false;
        };
        if let Some(reason) = start(record_id) {
            // Could not even start: the fault line names it, and no running
            // state was ever raised (so nothing has to be rested).
            self.say(format!("cannot summarize this branch: {reason}"));
            return false;
        }
        self.summarizing_branch = true;
        self.separator.branch_summarizing();
        true
    }

    /// Poll a background branch summary (gh #601): `Running` keeps the
    /// separator's state, `Done` posts the notice (or the named provider error)
    /// and rests it.
    ///
    /// `Done` is **consumed on read** by the host's hook, the `poll_compact`
    /// handoff rule, so a finished summary is reported once rather than on
    /// every tick. Unlike a compaction this owes the queue nothing: the user
    /// is still holding the `/tree` navigator, and the branch it points at is
    /// only taken when they press Enter afterwards.
    pub fn poll_branch_summary(&mut self) -> bool {
        if !self.summarizing_branch {
            return false;
        }
        let Some(poll) = self.world.options.hooks.poll_branch_summary.clone() else {
            return false;
        };
        match poll() {
            crate::state::BranchSummaryState::Idle | crate::state::BranchSummaryState::Running => {
                false
            }
            crate::state::BranchSummaryState::Done(notice) => {
                self.summarizing_branch = false;
                self.separator.idle();
                self.say(notice);
                true
            }
        }
    }

    /// The `/tree` key's whole job (gh #601): **summarize the branch being
    /// left, then leave it.**
    ///
    /// The order matters and is the design: the summary is written by the host's
    /// closure *after* the navigator closes, so the running separator is the only
    /// thing on screen while the provider works, and the branch itself is taken
    /// only when the poll says the record landed. A summary the user abandoned
    /// mid-flight therefore never moves them; the log's own record is what a
    /// replay shows either way.
    pub(crate) fn summarize_branch_and_replay(&mut self, record_id: &str) {
        // Close the navigator: the poll needs the separator, and the dock is
        // what paints it.
        self.tree_picker = None;
        if !self.start_branch_summary(record_id) {
            // A refusal (`cannot summarize …`) already posted its line; the
            // branch stays where it is, which is the safe half of "then leave".
            return;
        }
        self.branch_and_replay(record_id);
    }
}

#[cfg(test)]
#[path = "chat_branch_summary_tests.rs"]
mod tests;
