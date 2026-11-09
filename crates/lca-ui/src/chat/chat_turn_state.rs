//! The turn lifecycle: what starts a turn, what a turn's stream and its usage
//! counters do, and what the loop takes back out of it.
//!
//! Split from `chat.rs` at the 1,200-line ceiling (gh #574). This is the
//! *lifecycle* seam, not the event seam: per-event rendering still lives in
//! `chat_turn_events.rs`, which `on_turn_event` routes to once it has decided
//! which tab owns the event. An inherent `impl Chat` may live in a sibling
//! file, so this split moves code and not ownership - every method below is
//! still `Chat::<name>` at the same path, and the tests that call it keep
//! their names.

use lca_protocol::{TurnEvent, Usage};

use super::Chat;
use crate::state::TurnStatusLine;

impl Chat {
    /// Start a turn's usage view (gh #327): called at the turn boundary so
    /// a later turn's stream never inherits a previous one's numbers.
    pub fn begin_turn_usage(&mut self) {
        self.turn_usage = Usage::default();
    }

    /// Note one *tool step's* complete usage report inside a turn (gh
    /// #327): distinct calls sum, which is what core's per-turn
    /// `accumulate_usage` does, so the live turn view matches the total
    /// `reap` later folds.
    ///
    /// This is deliberately distinct from [`Usage::absorb`], which the
    /// stream handler uses: an `absorb` is a *refinement* within one call
    /// (gh #276 — a later non-zero value wins), while a step is a new call
    /// whose numbers add. Conflating them would either double-count a
    /// refinement or lose a step's spend.
    pub fn note_turn_step(&mut self, usage: &Usage) {
        self.turn_usage.input = self.turn_usage.input.saturating_add(usage.input);
        self.turn_usage.output = self.turn_usage.output.saturating_add(usage.output);
        self.turn_usage.cache_read = self.turn_usage.cache_read.saturating_add(usage.cache_read);
        self.turn_usage.cache_write = self
            .turn_usage
            .cache_write
            .saturating_add(usage.cache_write);
        self.turn_usage.cache_write_1h = self
            .turn_usage
            .cache_write_1h
            .saturating_add(usage.cache_write_1h);
        self.turn_usage.cost += usage.cost;
    }

    /// Fold one completed turn's total into the session totals (gh #327).
    ///
    /// This is the ONE implementation of "turn total into session total":
    /// `TurnState::reap` calls it with core's authoritative
    /// [`lca_protocol::TurnOutcome::usage`], and the record-replay path
    /// calls it per historical record. Both add exactly once, so the
    /// footer, `/session` and `/usage` report the true sum.
    pub fn fold_turn_usage(&mut self, usage: &Usage) {
        self.usage.input = self.usage.input.saturating_add(usage.input);
        self.usage.output = self.usage.output.saturating_add(usage.output);
        self.usage.cache_read = self.usage.cache_read.saturating_add(usage.cache_read);
        self.usage.cache_write = self.usage.cache_write.saturating_add(usage.cache_write);
        self.usage.cache_write_1h = self
            .usage
            .cache_write_1h
            .saturating_add(usage.cache_write_1h);
        self.usage.cost += usage.cost;
        for (key, value) in &usage.extras {
            self.usage.extras.insert(key.clone(), value.clone());
        }
    }

    /// Consume a turn event into the transcript and the counters.
    ///
    /// Gh #462: the event belongs to a *tab*, not to the interface. When the
    /// reader has switched away from the tab that owns the running turn, the
    /// event is applied to that tab's own state (`with_tab_state`) - so the
    /// stream keeps landing where it started, and the visible tab stays
    /// untouched. On the owning tab this is a plain call with no swap.
    pub fn on_turn_event(&mut self, event: TurnEvent) {
        let ends_turn = matches!(event, TurnEvent::TurnEnded { .. });
        let owner = self.turn_tab.unwrap_or(self.active_tab);
        // The tab flags are interface-wide facts about a tab, so they are set
        // outside the swap (they live on the tab itself, not on `Chat`).
        if ends_turn && let Some(tab) = self.tabs.get_mut(owner) {
            tab.running = false;
            // Unread only when the settle happened off-screen: a turn the
            // reader is watching needs no reminder.
            tab.unread = owner != self.active_tab;
        }
        self.with_tab_state(owner, |chat| chat.apply_turn_event(event, owner));
    }

    /// Mark a turn as started, with the queue it drains at each boundary.
    pub fn begin_turn(&mut self, steer: lca_protocol::SteerQueue) {
        self.turn_running = true;
        // Gh #462: the turn belongs to the tab that submitted it, for the rest
        // of its life. This single line is what every later event is routed
        // by, and what `Ctrl+C` consults.
        self.turn_tab = Some(self.active_tab);
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.running = true;
            tab.unread = false;
        }
        self.separator.working();
        self.current_steer = Some(steer);
        self.turn_status = Some(TurnStatusLine {
            text: "running...".into(),
        });
        self.transcript.begin_assistant();
    }

    /// Take the submitted prompt the loop should run, if any.
    pub fn take_submitted(&mut self) -> Option<String> {
        self.submitted.take()
    }

    /// Take the queue marker the submitted prompt was handed in, after
    /// [`Self::take_submitted`] (ADR-0038's record marker).
    pub(crate) fn take_submitted_queue(&mut self) -> Option<lca_protocol::SubmitMode> {
        self.submitted_queue.take()
    }
}
