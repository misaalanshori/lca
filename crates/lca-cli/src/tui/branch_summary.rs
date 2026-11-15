//! Gh #601: the host's branch-summary verb - the `/tree` `s` key's worker.
//!
//! # The shape, and why it is not an event
//!
//! The `/tree` navigator runs on the **interface** thread; the summary is one
//! provider call and must not run there (it would freeze the pane for its whole
//! duration, which is exactly the defect `/compact` was moved off that thread
//! for). So this is `tui/commands.rs`'s `/compact` slot, verbatim:
//!
//! - [`start_branch_summary`] **spawns and returns**, having set the shared cell
//!   to `Running` first, so the interface paints `Summarizing branch` before the
//!   model has answered;
//! - [`poll_branch_summary`] hands the result over **once** (the `Done` is taken
//!   on read, the `poll_compact` rule).
//!
//! A `TurnEvent` pair was the issue's own first idea and is the wrong carrier:
//! events are the turn loop's vocabulary, and this key never enters a turn.
//!
//! # The budget
//!
//! `lca_core::summarize_branch_now` asks the session's **compaction backend**
//! (`live.completion_backend`), the same object `/compact` uses, whose
//! `extras["max-tokens"]` is stamped from
//! [`lca_core::ext_provider::summarization_budget`] by
//! [`crate::tui::cells::apply_summarization_budget`] on every model change. So
//! there is one budget for both verbs and no second derivation to drift
//! (gh #594's lesson, one verb over).

use std::sync::Arc;

use crate::tui::Ui;

impl Ui {
    /// Start a branch summary for the abandoned tip `record_id` (gh #601).
    ///
    /// `Some(reason)` means the work could not even start - the interface posts
    /// that line and claims nothing running. `None` means it started; the answer
    /// arrives through the poll.
    pub(super) fn start_branch_summary(&self, record_id: &str) -> Option<String> {
        let mut state = self
            .branch_summary_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(*state, lca_ui::BranchSummaryState::Running) {
            return Some("a branch summary is already running".to_string());
        }
        // The same live handles `/compact` reads, taken here rather than inside
        // the thread so a `/model` switch during the call cannot swap the
        // backend out from under a request that already started.
        let live = crate::lock(&self.live).agent_config.clone();
        let Some(backend) = live.completion_backend.clone() else {
            return Some("no provider is available to summarize the branch".to_string());
        };
        let extensions = live.extensions.clone();
        let store = self.store.clone();
        let session = self.session();
        let record_id = record_id.to_string();
        // Raise the running state **before** the spawn: the interface polls the
        // same instant the key returns, and a state set after a spawn race is a
        // separator that never says anything on a fast failure.
        *state = lca_ui::BranchSummaryState::Running;
        drop(state);
        let state = self.branch_summary_state.clone();
        std::thread::spawn(move || {
            let notice = match lca_core::summarize_branch_now(
                store,
                session,
                extensions,
                Some(backend),
                &record_id,
            ) {
                // The notice is the user-visible outcome; the record itself is
                // what a replay renders as the card.
                Ok(summary) => format!(
                    "summarized branch at {record_id} · {} chars",
                    summary.chars().count()
                ),
                // Gh #594/#169: a failure names the error, so a summary that did
                // not happen is a visible line rather than an absent record.
                Err(reason) => format!("branch summary failed: {reason}"),
            };
            *state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                lca_ui::BranchSummaryState::Done(notice);
        });
        None
    }

    /// Poll the branch summary (gh #601): `Running` keeps the separator's state,
    /// a finished one is handed over exactly once.
    pub(super) fn poll_branch_summary(&self) -> lca_ui::BranchSummaryState {
        let mut guard = self
            .branch_summary_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.clone() {
            lca_ui::BranchSummaryState::Done(notice) => {
                *guard = lca_ui::BranchSummaryState::Idle;
                lca_ui::BranchSummaryState::Done(notice)
            }
            ongoing => ongoing,
        }
    }

    /// The hook the interface holds: the same pair as `Arc`s.
    pub(super) fn branch_summary_hooks(
        self: &Arc<Self>,
    ) -> (lca_ui::BranchSummaryStart, lca_ui::BranchSummaryPoll) {
        let start = self.clone();
        let poll = self.clone();
        (
            Arc::new(move |record_id: &str| start.start_branch_summary(record_id)),
            Arc::new(move || poll.poll_branch_summary()),
        )
    }
}
