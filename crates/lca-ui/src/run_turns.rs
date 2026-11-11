//! The running turn's state: the one place that knows whether a turn is
//! running, what it last reported, and whether it was aborted (`R16`).
//!
//! Split out of `run.rs` for gate 11's 1,200-line ceiling (gh #575; `run.rs` was
//! 1,196 lines before the gh #593 notice added four). **No name moved**: `run.rs`
//! re-exports `TurnState`, Rust lets an inherent `impl` live in any module of the
//! crate, and every row that drives `reap`/`settle`/`new` keeps its path because
//! the definition never changed files at all.
//!
//! Its lifecycle mirror (`run_status.rs`) is an `impl TurnState` in a third file
//! for the same reason; the split here is by *subject* (state and transition)
//! while that one is by *effect* (the ambient channels it writes).

use std::sync::mpsc::Receiver;

use lca_protocol::{StopReason, TurnOutcome, TurnStatus, Usage};

use crate::chat::Chat;
use crate::state::{DialogExchange, PermissionModal, PromptRequest, TurnChannels, TurnRunner};

/// The running turn's mutable state (R16), so the loop body stays a loop.
pub(crate) struct TurnState {
    pub(crate) active: Option<std::thread::JoinHandle<TurnOutcome>>,
    turn_rx: Option<Receiver<lca_protocol::TurnEvent>>,
    /// The session's prompt receiver: kept for the whole session, so a
    /// consent asked outside a turn still reaches the modal.
    prompt_rx: Receiver<PromptRequest>,
    /// The session's dialog receiver (gh #124): same lifetime, so a
    /// question asked outside a turn still reaches its modal.
    dialog_rx: Receiver<DialogExchange>,
    /// The session's prompt sender; each turn gets a clone.
    prompt_tx: std::sync::mpsc::SyncSender<PromptRequest>,
    pub(crate) cancel: Option<lca_tools::CancelFlag>,
    pub(crate) aborted: bool,
    /// The last reported tab-badge state (gh #213, change-only emits).
    pub(crate) program_status: Option<lca_tui::engine::program_status::ProgramState>,
    /// The finished turn's outcome (`None` pre-first-turn and post-abort).
    pub(crate) last_ok: Option<bool>,
    /// Whether `OSC 9;4` progress is currently on (gh #369): tracked so the
    /// spinner is armed once per working period, not once per frame.
    pub(crate) progress_active: bool,
    /// Wakes the loop when a turn producer enqueues (gh #370 flaw 1).
    waker: crate::run_wake::Waker,
}

impl TurnState {
    /// The session's prompt channel (created once in `run`).
    pub(crate) fn new(
        prompt_tx: std::sync::mpsc::SyncSender<PromptRequest>,
        prompt_rx: Receiver<PromptRequest>,
        dialog_rx: Receiver<DialogExchange>,
        waker: crate::run_wake::Waker,
    ) -> TurnState {
        TurnState {
            active: None,
            turn_rx: None,
            prompt_rx,
            prompt_tx,
            dialog_rx,
            cancel: None,
            aborted: false,
            program_status: None,
            last_ok: None,
            progress_active: false,
            waker,
        }
    }

    /// Reap a finished turn (returns whether one finished).
    pub(crate) fn reap(&mut self, chat: &mut Chat) -> bool {
        let Some(handle) = self.active.take_if(|h| h.is_finished()) else {
            return false;
        };
        let outcome = handle.join().unwrap_or_else(|_| TurnOutcome {
            status: TurnStatus::Error,
            stop_reason: StopReason::Error,
            usage: Usage::default(),
            error: Some("the turn worker panicked".to_string()),
        });
        if let Some(rx) = &self.turn_rx {
            while let Ok(event) = rx.try_recv() {
                chat.on_turn_event(event);
            }
        }
        self.turn_rx = None;
        // The prompt receiver stays: a host-side consent asked between
        // turns (the picker's endpoint consent) must still find it.
        self.cancel = None;
        // Gh #590: **the outcome's notice is the fallback, not the paint.**
        //
        // A terminal failure reaches the interface as a `TurnEvent::Error`, which
        // paints the transcript's error line (the turn's own channel - the one
        // every listener sees, and the one `headless` turns into its JSON
        // envelope). Printing `outcome.error` here as well painted the same text
        // twice: once as the ✗ line, once as a notice. So the notice is kept for
        // the case it exists for - a turn that failed *before* any sink event
        // could be delivered (a worker that never started, a panicking join, a
        // `fail()` before the sink is handed one) - and it is skipped when this
        // turn's own event already painted. Both strings are the same
        // `TurnOutcome::error`, so the flag is the only difference between them.
        //
        // Gh #462: the flag is read on the tab that owns the turn, beside the
        // transcript that painted the line - a background turn's settle must not
        // consult, or silence, the visible tab's.
        let owner = chat.turn_tab.unwrap_or(chat.active_tab);
        if let Some(error) = &outcome.error {
            let painted = chat.with_tab_state(owner, |chat| chat.turn_error_painted);
            if !painted {
                chat.with_tab_state(owner, |chat| {
                    chat.say(crate::state::sanitize_text(error));
                });
            }
        }
        // Gh #42: a counted miss the flag asked to surface shows
        // once, as a toast — a diagnostic, not history — beside any
        // error, which keeps the transcript to itself.
        if let Some(report) = chat
            .world
            .options
            .hooks
            .cache_miss_notice
            .as_ref()
            .and_then(|hook| hook())
        {
            chat.toast(crate::state::sanitize_text(&report));
        }
        // A turn that ends without its `TurnEnded` event still rests the
        // separator.
        chat.separator.idle();
        // Gh #327: once, from core's turn sum - into the tab that ran the
        // turn (gh #462). The session total is per session, so a background
        // turn's spend must land on its own tab, not on whatever the reader is
        // looking at when the worker happens to finish.
        chat.with_tab_state(owner, |chat| chat.fold_turn_usage(&outcome.usage));
        // Gh #590: the turn's paint flag is spent; the next `begin_turn` clears
        // it again. Clearing here as well keeps a settled interface from reading
        // a finished turn's flag as if it belonged to whatever runs next.
        chat.with_tab_state(owner, |chat| chat.turn_error_painted = false);
        // Gh #213: the resting badge follows the outcome (read before
        // the aborted branch below overwrites it for cancels).
        self.last_ok = Some(outcome.error.is_none());
        // Gh #462: the turn is over, so the tab that owned it stops reporting
        // a running marker. A turn that ended without its `TurnEnded` event (a
        // panicking worker, an early error) never ran the event-side clear, so
        // the settle happens here too - one place that knows the worker
        // really stopped.
        chat.settle_turn_tab();
        chat.turn_running = false;
        // Gh #462: a tab switch that arrived mid-turn reaches the host here,
        // once the worker has stopped writing. Before this point the host's
        // current-session binding still names the session the turn ran in,
        // which is what kept its records in the right log.
        chat.apply_deferred_host_switch();
        chat.current_steer = None;
        // Gh #369: the taskbar spinner is now driven by the unified
        // `sync_program_status` (turn, compaction, and shell all count), so
        // the hand-maintained arm/disarm here is gone - one owner, one edge.
        // Steering lifecycle (ADR-0038): an aborted turn returns its queue
        // to the editor; a completed turn runs the queued messages in order.
        // Gh #213: the resting badge follows the outcome (an abort
        // rests idle, like startup).
        if self.aborted {
            self.last_ok = None;
            chat.restore_pending();
            self.aborted = false;
        } else if let Some(next) = chat.take_next_pending() {
            chat.submitted = Some(next);
        }
        true
    }

    /// Start a submitted turn (returns whether one started).
    pub(crate) fn start(&mut self, chat: &mut Chat, runner: &TurnRunner) -> bool {
        if chat.turn_running {
            return false;
        }
        let Some(text) = chat.take_submitted() else {
            return false;
        };
        let queue = chat.take_submitted_queue();
        // Gh #370 flaw 1: a streamed event wakes the loop; the forwarder keeps the
        // bounded sender (and its backpressure) and wakes after each enqueue.
        let (event_tx, event_rx) = crate::run_wake::forwarded_channel(256, self.waker.clone());
        let prompt_tx = self.prompt_tx.clone();
        let cancel = lca_tools::CancelFlag::new();
        self.cancel = Some(cancel.clone());
        let steer = lca_protocol::steer_queue();
        let channels = TurnChannels {
            events: event_tx,
            prompt: prompt_tx,
            steer: steer.clone(),
            queue,
        };
        chat.begin_turn(steer);
        // Gh #369: arming happens in `sync_program_status`, from the same
        // `WorkKind` the badge uses; nothing to do here.
        self.turn_rx = Some(event_rx);
        self.active = Some(runner(text, channels, cancel));
        true
    }

    /// Drain the turn, shell, and prompt channels (returns whether anything
    /// changed).
    pub(crate) fn drain(&mut self, chat: &mut Chat) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.turn_rx {
            while let Ok(event) = rx.try_recv() {
                chat.on_turn_event(event);
                changed = true;
            }
        }
        // Drain the running `!`/`!!` command (R4).
        if chat.poll_shell() {
            changed = true;
        }
        if chat.world.permission.is_none()
            && chat.world.dialog.is_none()
            && let Ok(request) = self.prompt_rx.try_recv()
        {
            chat.world.permission = Some(PermissionModal {
                action: request.action,
                respond: Some(request.respond),
                // FR-UI-18: an auto-approve countdown, visible and
                // keyboard-interruptible.
                deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
            });
            changed = true;
        }
        // A host-rendered question (gh #124): its own module, so
        // this file stays under the ceiling.
        changed |= crate::dialogs::drain_dialog(chat, &self.dialog_rx);
        changed
    }

    /// Cancel and join the running turn on shutdown.
    pub(crate) fn shutdown(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.active.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
#[path = "run_593_tests.rs"]
mod split_tests;
