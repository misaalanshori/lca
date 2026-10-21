//! The message queue: steering and follow-up (ADR-0038), the compaction
//! queue and its flush (gh #401), and the restore that returns everything
//! to the editor. Split from `chat.rs` under the workspace's 1,200-line
//! ceiling (gate 11); behaviour unchanged.

use super::chat::{Chat, PendingMessage};

impl Chat {
    /// Queue a message submitted while a turn runs (ADR-0038). `Steer`
    /// entries also go to the running turn's boundary queue.
    pub fn queue_submit(&mut self, text: String, mode: lca_protocol::SubmitMode) {
        if mode == lca_protocol::SubmitMode::Steer
            && let Some(steer) = &self.current_steer
        {
            steer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(lca_protocol::QueuedMessage {
                    text: text.clone(),
                    mode,
                });
        }
        self.pending.push(PendingMessage {
            text,
            mode,
            is_command: false,
        });
    }

    /// Queue a slash command submitted while a turn runs: it rides the
    /// pending queue in order but stays out of the running turn's steer
    /// queue (command text is never model input), and [`Self::take_next_pending`]
    /// dispatches it when the turn ends.
    pub fn queue_command(&mut self, text: String, mode: lca_protocol::SubmitMode) {
        self.pending.push(PendingMessage {
            text,
            mode,
            is_command: true,
        });
    }

    /// Pop the next queued message to auto-submit at turn end, in order.
    /// Leading queued commands dispatch first (they never become model
    /// text and never join the transcript as user messages); the first
    /// queued message behind them flushes as the next turn, if any.
    /// Gh #76: `All` follow-up mode flushes every pending follow-up as
    /// one joined submission instead (steer entries flush one per call
    /// either way - the boundary already delivered them).
    pub fn take_next_pending(&mut self) -> Option<String> {
        // Queued commands run first, as commands: dispatch each one in
        // order and stop flushing if one of them submitted a prompt of
        // its own (the rest wait behind that turn).
        while self.pending.first().is_some_and(|next| next.is_command) {
            let command = self.pending.remove(0);
            let _ = self.dispatch_command(&command.text);
            if self.submitted.is_some() {
                return None;
            }
        }
        if self.pending.is_empty() {
            return None;
        }
        if self.follow_up_mode == lca_protocol::QueueMode::All {
            return self.take_all_pending();
        }
        let next = self.pending.remove(0);
        self.transcript.push_user(next.text.clone());
        // The flush keeps the marker the message was queued with: a steer
        // that never reached a boundary is still "submitted while a turn
        // was running", and the record this turn writes has to say so.
        self.submitted_queue = Some(next.mode);
        Some(next.text)
    }

    /// Flush every pending message as one joined submission (gh #76's
    /// `all` follow-up mode): commands already dispatched above, so
    /// only texts remain; the first message's marker rides the turn.
    fn take_all_pending(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let drained: Vec<PendingMessage> = std::mem::take(&mut self.pending);
        let mode = drained.first().map(|next| next.mode);
        let mut texts = Vec::new();
        for next in &drained {
            texts.push(next.text.clone());
        }
        // Gh #254: one joined submission writes one turn record, so
        // the transcript carries one user entry — one entry per
        // drained message forked phantom indexes the host cannot map.
        let joined = texts.join("\n\n");
        self.transcript.push_user(joined.clone());
        self.submitted_queue = mode;
        Some(joined)
    }

    /// Park a message submitted while a compaction runs (gh #401, pi's
    /// `queueCompactionMessage`): the editor clears, the dock lists it,
    /// and the notice says when it will run. A compaction owns the
    /// session, so starting a turn now would race it; refusing the text
    /// would lose it, which is the failure this replaces.
    pub fn queue_for_compaction(&mut self, text: String, mode: lca_protocol::SubmitMode) {
        self.compaction_queued.push(PendingMessage {
            text,
            mode,
            is_command: false,
        });
        self.say("Queued message for after compaction");
    }

    /// Park a *command* in the same compaction queue. A command typed
    /// during compaction dispatches at once (pi's rule, so a slash line
    /// stays responsive); this is the seam for one that could not - a
    /// message queued before its extension registered, which is exactly
    /// the case pi's `flushCompactionQueue` also defends.
    pub fn queue_command_for_compaction(&mut self, text: String, mode: lca_protocol::SubmitMode) {
        self.compaction_queued.push(PendingMessage {
            text,
            mode,
            is_command: true,
        });
        self.say("Queued message for after compaction");
    }

    /// Run the queue parked during a compaction (gh #401, pi's
    /// `flushCompactionQueue`). Returns whether anything was flushed.
    ///
    /// Ordering is pi's and is what keeps a queued command from being
    /// swallowed: leading commands dispatch first, the first prompt starts
    /// a turn (or joins the running one), and the rest re-queue in order.
    /// A first prompt that cannot become a turn rolls the whole queue back
    /// (the restore half of pi's `restoreQueue`), so a compaction ending
    /// during a broken login never eats what the user typed.
    pub fn flush_compaction_queue(&mut self) -> bool {
        if self.compaction_queued.is_empty() {
            return false;
        }
        let queued: Vec<PendingMessage> = std::mem::take(&mut self.compaction_queued);
        // Leading extension commands run now, as commands (never as model
        // text); a command that starts its own turn parks the rest.
        let mut rest: Vec<PendingMessage> = Vec::new();
        let mut first_prompt: Option<PendingMessage> = None;
        for message in queued {
            if first_prompt.is_none() && message.is_command {
                let _ = self.dispatch_command(&message.text);
                if self.submitted.is_some() {
                    rest.push(message);
                    break;
                }
                continue;
            }
            if first_prompt.is_none() {
                first_prompt = Some(message);
            } else {
                rest.push(message);
            }
        }
        let Some(first) = first_prompt else {
            // Only commands: nothing to turn.
            self.pending.extend(rest);
            return true;
        };
        // A turn still running (an auto-compaction inside it) takes the
        // message the way any mid-turn submit does, rather than starting a
        // second turn: pi's `willRetry` branch, same destination.
        if self.turn_running {
            if first.mode == lca_protocol::SubmitMode::Steer
                && let Some(steer) = &self.current_steer
            {
                steer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(lca_protocol::QueuedMessage {
                        text: first.text.clone(),
                        mode: first.mode,
                    });
            }
            self.pending.push(first);
            self.pending.extend(rest);
            return true;
        }
        // A turn needs a model; without one the queue goes back whole
        // (pi's `restoreQueue`) rather than evaporating.
        if self.model_label().trim().is_empty() {
            self.compaction_queued.push(first);
            self.compaction_queued.extend(rest);
            self.say(
                "No model is active; the message stays queued. Use /login to sign in, or /model to choose one."
                    .to_string(),
            );
            return true;
        }
        self.transcript.push_user(first.text.clone());
        self.submitted_queue = Some(first.mode);
        self.submitted = Some(first.text);
        self.pending.extend(rest);
        true
    }

    /// Restore the queued messages to the editor, in order (FR-CORE-12).
    ///
    /// Gh #401 (pi's `restoreQueuedMessagesToEditor`): the compaction queue
    /// comes too, so one action returns everything; the queue is joined
    /// `\n\n`-separated and placed **ahead of** the live draft, because the
    /// queue is older than what the user typed while waiting; and the
    /// notice names how many came back. Returns that count.
    pub fn restore_pending(&mut self) -> usize {
        let parked: Vec<PendingMessage> = std::mem::take(&mut self.compaction_queued);
        if self.pending.is_empty() && parked.is_empty() {
            return 0;
        }
        // A restored message must not also be injected at the next boundary.
        if let Some(steer) = &self.current_steer {
            steer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
        }
        let queued: Vec<PendingMessage> = self.pending.drain(..).chain(parked).collect();
        let restored = queued.len();
        let mut text = String::new();
        for (i, pending) in queued.into_iter().enumerate() {
            if i > 0 {
                text.push_str("\n\n");
            }
            text.push_str(&pending.text);
        }
        let draft = self.editor.text();
        let merged = if draft.trim().is_empty() {
            text
        } else if text.is_empty() {
            draft
        } else {
            format!("{text}\n\n{draft}")
        };
        // `set_text` parks the caret at the end, which is where the next
        // keystroke should land (pi does the same).
        self.editor.set_text(&merged);
        self.say(format!(
            "Restored {restored} queued message{} to editor",
            if restored == 1 { "" } else { "s" }
        ));
        restored
    }
}
