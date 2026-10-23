//! The turn-event applier (gh #462): what one event does to the state in
//! hand, split from `chat.rs` for the 1,200-line ceiling.
//!
//! [`Chat::on_turn_event`] decides *whose* state an event belongs to (the tab
//! that owns the running turn); this module decides what it does once that
//! state is in hand. Before gh #462 the two questions were one function,
//! because there was only ever one answer to the first one.

use lca_protocol::{StopReason, TurnEvent, TurnStatus};

use crate::chat::Chat;
use crate::state::TurnStatusLine;
use crate::transcript::ToolStatus;

impl Chat {
    /// Apply one turn event to the state in hand (gh #462).
    ///
    /// Split out of [`Chat::on_turn_event`] because that function decides
    /// *whose* state this is and this one decides what it does - the two
    /// questions a background turn made separable.
    pub(crate) fn apply_turn_event(&mut self, event: TurnEvent, owner: usize) {
        match event {
            // Reconstruction events for headless/rpc; rendered elsewhere.
            TurnEvent::TurnStarted | TurnEvent::MessageStarted { .. } => {}
            TurnEvent::MessageEnded { .. } | TurnEvent::RetryFinished { .. } => {}
            // Gh #496: the live user is *in* the session when a threshold or
            // overflow compaction fires, so the event that ends one leaves a row.
            // `CompactionStarted` deliberately does not: the separator already
            // shows `Compacting`, and a second live indicator of the same fact is
            // the duplication this repo keeps consolidating away. What the
            // transcript can add is the *record* of the event, in its
            // chronological slot, naming the reason and whether it worked.
            TurnEvent::CompactionStarted { .. } => {
                // A new compaction begins, so the *next* end gets its own row.
                self.compaction_marked = false;
            }
            TurnEvent::CompactionEnded { reason, success } => {
                self.note_live_compaction(&reason, success)
            }
            TurnEvent::TextDelta(delta) => self.transcript.append_text(&delta),
            TurnEvent::ReasoningDelta(delta) => self.transcript.append_reasoning(&delta),
            TurnEvent::ToolStarted(call) => {
                // gh #404 gap 1: the parent link rides along so an orphaned
                // child (a parent card that is not on screen) still renders as a
                // top-level card instead of vanishing.
                self.transcript.start_tool_with_id(
                    call.name,
                    call.arguments,
                    call.parent_call_id,
                    Some(call.call_id),
                );
            }
            TurnEvent::ToolFinished(result) => {
                let status = match result.status {
                    lca_protocol::ToolResultStatus::Ok => ToolStatus::Ok,
                    lca_protocol::ToolResultStatus::Error => ToolStatus::Error,
                    lca_protocol::ToolResultStatus::Denied => ToolStatus::Denied,
                    lca_protocol::ToolResultStatus::Timeout => ToolStatus::Timeout,
                };
                // gh #9: the tool's structured diff rides into the card
                // (`extras` is the carrier; display text is never
                // re-parsed to recover a change).
                // gh #404 gap 1: the nested calls ride into the parent's card as
                // a checklist. `ToolResult.nested` is the only place they exist -
                // a nested call is never its own record - so this is the one
                // wiring point that closes the flood.
                // gh #404 gap 1: the children arrive by two paths (the live
                // start/finish pairs, and the bounded record on the parent's
                // result). `finish_tool_with_id` keeps whichever came first.
                let children: Vec<crate::transcript::ChildCall> = result
                    .nested
                    .iter()
                    .map(|nested| crate::transcript::ChildCall {
                        name: nested.name.clone(),
                        status: nested.status.into(),
                        head: nested.content_head.clone(),
                    })
                    .collect();
                self.transcript.finish_tool_with_id(
                    status,
                    Some(result.content.clone()),
                    result.extras.get("diff").cloned(),
                    result.duration_ms,
                    children,
                    Some(result.call_id.clone()),
                );
                // R5: a tool that read an image shows it through the ladder.
                for image in &result.images {
                    let info =
                        lca_tui::widgets::image::ImageInfo::new(&image.media_type, &image.bytes);
                    self.transcript.push_image(info, image.bytes.clone());
                }
            }
            TurnEvent::ToolOutputChunk { chunk, .. } => {
                self.transcript.append_tool_output(&chunk);
            }
            TurnEvent::Usage(usage) => {
                // FR-UI-20: the live prompt size is this call's input side.
                self.footer.context_used = usage.input + usage.cache_read + usage.cache_write;
                // The turn's own output side, for the tok/s reading at its end.
                // Gh #464: the meter takes the whole record and uses only the
                // calls that stamped a generation span, so an unstamped call's
                // tokens can never pad the numerator.
                self.meter.note_usage(&usage);
                // gh #327: this is display state only. The session total is
                // folded once at the turn boundary; growing it here would
                // count the turn twice (the stream, then `reap`) and would
                // also double-count a provider that refines its totals
                // across events (gh #276). The turn-local view absorbs each
                // report instead, which is the correct merge.
                self.turn_usage.absorb(&usage);
            }
            TurnEvent::RetryScheduled {
                attempt,
                max,
                error,
                delay_ms,
                ..
            } => {
                // The separator counts the backoff down (chrome.md's
                // `RetryStatusIndicator`); the error text stays a notice.
                self.separator.retrying(attempt, max, delay_ms);
                self.say(format!("retry {attempt}/{max} after: {error}"));
            }
            TurnEvent::Error { message, .. } => {
                self.transcript.push_error(message);
            }
            TurnEvent::AssistantText(_) => {}
            TurnEvent::UserInjected { text, mode } => {
                // A steered message crossed the boundary: it leaves the
                // pending band and joins the transcript (ADR-0038).
                if let Some(pos) = self.pending.iter().position(|p| p.text == text) {
                    self.pending.remove(pos);
                }
                let _ = mode;
                self.transcript.push_user(text);
            }
            TurnEvent::ExtensionEvent {
                extension,
                event,
                detail,
            } => {
                // Host diagnostics (cache divergence, sync notes)
                // float and fade; they are not conversation.
                self.toast(format!("[{extension}] {event}: {detail}"));
            }
            TurnEvent::TurnEnded {
                status,
                stop_reason,
            } => {
                // The footer's throughput reading (owner ask): output tokens
                // over the generation span core stamped where the stream was
                // consumed, summed over the turn's calls (gh #464).
                self.meter.finish_turn(&mut self.footer);
                self.transcript.finish_assistant();
                // gh #462: `turn_running` is an interface fact and stays set
                // until `reap` joins the worker; only the per-tab cue is
                // cleared here. `refresh_active_tab_title` is likewise about
                // the tab the *turn* owns, so it is routed by the caller's
                // swap - the title resolves from the records the turn wrote.
                self.turn_status = Some(TurnStatusLine {
                    text: match (status, stop_reason) {
                        (TurnStatus::Ok, StopReason::Stop) => "done".to_string(),
                        (TurnStatus::Ok, StopReason::Cancelled) => "cancelled".to_string(),
                        (TurnStatus::Error, StopReason::IterationLimit) => {
                            "stopped: iteration limit".to_string()
                        }
                        (TurnStatus::Error, _) => "done with errors".to_string(),
                        (TurnStatus::Ok, _) => "done".to_string(),
                    },
                });
                // The tab's title resolves from its own session, which is the
                // tab the turn ran in (gh #209/#462).
                if let Some(id) = self.tabs.get(owner).map(|tab| tab.session_id.clone())
                    && let Some(title) = self.resolve_tab_title(&id)
                    && let Some(tab) = self.tabs.get_mut(owner)
                {
                    title.clone_into(&mut tab.title);
                }
                // pi clears the indicator on stop; the transcript's error
                // line and the footer's status carry a stop that was not
                // clean (chrome.md has no error kind).
                self.separator.idle();
            }
        }
    }

    /// Record a finished compaction in the transcript (gh #496).
    ///
    /// The user is looking at this transcript when a threshold or overflow
    /// compaction fires, so the row is the only evidence they get live: the
    /// summary itself lives in a `Compaction` record this crate cannot reach
    /// (no `lca-session` dependency, by design), and it appears on `/resume`
    /// where `replay_records` puts it in record order. Printing a marker that
    /// names *that* it happened, and *why*, is the honest half.
    ///
    /// `success: false` must not look like success. It means no summary record
    /// was written: the context is exactly as it was, and a neutral marker would
    /// tell the user the opposite of the truth about their session.
    ///
    /// **One marker per compaction, not per event.** `compact_candidate` emits
    /// one `CompactionEnded` per attempt, and the overflow recovery can attempt
    /// more than one, so a duplicate end must not append a second row: the
    /// transcript is the record, and two rows read as two summaries. The
    /// `CompactionStarted` arm clears `compaction_marked` so a *real* second
    /// compaction still gets its own marker.
    fn note_live_compaction(&mut self, reason: &str, success: bool) {
        if self.compaction_marked {
            return;
        }
        self.compaction_marked = true;
        // Gh #393: the live row is the same collapsed card the replayed record
        // gets, so the two views agree on *shape* even though they differ on
        // content (#496: the event carries no summary). The reason still rides
        // along - on a failure especially, since `success: false` means nothing
        // was rewritten and a neutral line would say the opposite.
        self.transcript
            .push_compaction_card(crate::transcript::Card::live(reason, success));
    }
}
