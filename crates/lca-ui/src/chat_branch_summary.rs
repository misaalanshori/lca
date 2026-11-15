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
        self.branch_summary_target = Some(record_id.to_string());
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
                // The notice lands **after** the replay, never before: branching
                // clears the transcript (`replay_records`), so a notice posted
                // first would be wiped by the very step that follows it - found
                // in a real pane, where the summary finished and the user saw
                // only `branched at …`.
                // The **replay** is the "then leave", and it is a re-read, not a
                // second navigation. `store.summarize_branch` already appended the
                // branch-point: navigating again would write a *second* one, which
                // orphans the summary the first one's chain carried - measured in a
                // real pane, where the summary finished and the next turn's request
                // carried no `abandoned branch summary` at all. So this re-reads the
                // display view, which is what `switch_session` does for the current
                // session (no append, no new id).
                if let Some(target) = self.branch_summary_target.take()
                    && let (Some(current), Some(switch)) = (
                        self.world.options.hooks.current_session_id.as_ref(),
                        self.world.options.hooks.switch_session.as_ref(),
                    )
                    && let Some(records) = switch(&current())
                {
                    self.replay_records(&records);
                    self.say(format!("branched at {target}; type to continue"));
                }
                self.say(notice);
                true
            }
        }
    }

    /// The `/tree` key's whole job (gh #601): **summarize the branch being
    /// left, then leave it** - in that order, which is why only the navigator
    /// closes here and the branch waits for [`Self::poll_branch_summary`].
    ///
    /// Branching first was the first cut of this code and it was wrong in a way
    /// only a real pane showed: `branch_at` cuts the display view at the
    /// navigation point, so the summary - computed milliseconds later on that
    /// same view - found nothing after it and every summary failed with
    /// `nothing to summarize`.
    pub(crate) fn summarize_branch_and_replay(&mut self, record_id: &str) {
        // Close the navigator: the poll needs the separator's row, and the dock
        // is what paints it.
        self.tree_picker = None;
        // A refusal (`cannot summarize …`) posts its own line, and the branch
        // stays where it is - the safe half of "then leave". There is nothing to
        // do with the boolean here: the branch happens in the poll, so a failure
        // simply leaves the target unset.
        let _ = self.start_branch_summary(record_id);
    }
}

#[cfg(test)]
#[path = "chat_branch_summary_tests.rs"]
mod tests;
