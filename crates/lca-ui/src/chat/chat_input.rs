//! What a keypress does: the input ladder (`handle_key`/`global_key`), and
//! what a submitted buffer becomes.
//!
//! Split from `chat.rs` at the 1,200-line ceiling (gh #574). This is the seam
//! the code already had and the doc comment already named - "the modal first,
//! then the pickers, the bindings, and the editor" - so the reader can now
//! follow one dispatch path in one file. The modal and picker *handlers* stay
//! in `chat_modals.rs` and `chat_keys.rs`; this file is the ladder that reaches
//! them and the submit path at its foot.

use lca_tui::engine::keys;
use lca_tui::widgets::editor::EditorEvent;

use super::Chat;
use crate::chat_commands::is_turn_boundary_command;
use crate::state::Action;

impl Chat {
    /// Handle one keypress: the modal first, then the pickers, the
    /// bindings, and the editor.
    pub fn handle_key(&mut self, data: &str) -> Action {
        // Gh #210: any key dismisses a showing tooltip with zero
        // latency (and rearms the hover clock).
        self.dismiss_tooltip();
        if let Some(action) = self.handle_modal_key(data) {
            return action;
        }

        let key = keys::parse_key(data);

        // gh #30: a running turn's Ctrl+C reaches the agent even while a
        // picker owns the keyboard - the owner's own words in the row
        // that guards this ("not even ctrl-c is able to close it"). The
        // modal dispatch above keeps its own keys: this is pickers only.
        //
        // Gh #462: only a turn the reader can see is cancellable here. A
        // background tab's turn is not - silently killing work nobody is
        // looking at is how a `cargo test` gets thrown away. Switch to that
        // tab and cancel there.
        if self.cancellable_turn_visible() && self.keybindings.matches(data, "app.clear") {
            return Action::CancelTurn;
        }

        if let Some(action) = self.handle_picker_key(data, key.as_deref()) {
            return action;
        }

        // Escape cancels a running `!`/`!!` command (R4), after the pickers
        // (a picker's Escape closes it instead).
        if key.as_deref() == Some("escape")
            && let Some(shell) = self.shell.as_ref()
        {
            (shell.cancel)();
            self.say(format!("cancelling `{}`", shell.command));
            return Action::Continue;
        }

        // The transcript search owns the keyboard while open (FR-UI-12) - and **only
        // while it is the layer on top** (gh #373 gap 5). It used to take every key
        // whenever a query was set, so a search running under a raised modal swallowed
        // the modal's keys... except that the modal is dispatched earlier, so the real
        // defect was the reverse: `render` decided padding from a predicate set while
        // an overlay drew over the readout. Asking the stack makes "the search has the
        // keyboard" and "the search is visible" the same fact.
        if self.top_layer() == Some(crate::layer_stack::Layer::Search) {
            return self.handle_search_key(data, key.as_deref());
        }
        if self.keybindings.matches(data, "app.tools.expand") {
            // Gh #400 gap 2: the same key expands the startup header. One key for
            // "show me the detail" - the header is the detail a reader reaches for
            // before any card exists, and the brief is explicit that it must not
            // get a second binding.
            //
            // Both toggles run: a session with tool cards *and* a header should
            // expand both on one press, which is what a single "expand" key means.
            let header = self.transcript.toggle_startup_resources();
            self.transcript.toggle_tools_expanded();
            let _ = header;
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.thinking.toggle") {
            self.transcript.toggle_thinking_expanded();
            return Action::Continue;
        }
        // gh #209: session tabs - new, close (empty composer only,
        // so Ctrl+W keeps deleting words with text), cycle, and
        // Alt+digit direct jumps. Pickers own the keyboard while open
        // (above), the editor owns the rest below.
        if self.handle_tab_key(data, key.as_deref()) {
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.search") {
            self.search = Some(String::new());
            self.search_matches.clear();
            self.search_index = 0;
            // The readout renders from state in the dock (gh #239).
            return Action::Continue;
        }
        // gh #8 (EFG-003): pi's model cycle - Ctrl+P forward, Ctrl+Shift+P
        // (Alt+P where a terminal drops the shift) backward. The host does
        // the switch, so a cycle lands everywhere a `/model` pick does.
        if self.keybindings.matches(data, "app.model.cycleForward")
            || self.keybindings.matches(data, "app.model.cycleBackward")
        {
            let forward = self.keybindings.matches(data, "app.model.cycleForward");
            self.say(crate::state::sanitize_block(
                &match &self.world.options.hooks.cycle_model {
                    Some(cycle) => cycle(forward),
                    None => "model cycling is not available in this host".to_string(),
                },
            ));
            return Action::Continue;
        }
        // Ctrl+X Ctrl+E opens the external editor (FR-UI-15).
        if self.pending_ctrl_x {
            self.pending_ctrl_x = false;
            if key.as_deref() == Some("ctrl+e") {
                return Action::ExternalEditor;
            }
        }
        if self.keybindings.matches(data, "app.editor.external") {
            self.pending_ctrl_x = true;
            return Action::Continue;
        }

        // Gh #401 item 3: with a queue waiting, the dequeue key takes the
        // queue back into the editor before anything else claims it -
        // above the *editor*, because `Alt+Up` is also a cursor move there
        // and the editor would swallow it. With no queue the key falls
        // through to its other meanings (jump between prompts). The same
        // restore path as the abort rescue and the dock's hint.
        if (!self.pending.is_empty() || !self.compaction_queued.is_empty())
            && self.keybindings.matches(data, "app.message.dequeue")
        {
            self.restore_pending();
            return Action::Continue;
        }

        // The extension panel toggle is a host binding, not an extension's.
        if self.keybindings.matches(data, "app.panel.toggle") {
            self.world.panel_open = !self.world.panel_open;
            return Action::Continue;
        }

        match self.editor.handle_key(data) {
            EditorEvent::Submitted(text) => self.on_submit(text),
            EditorEvent::Exit => Action::Exit,
            EditorEvent::Changed | EditorEvent::None => self.global_key(data),
        }
    }

    /// Keys the editor does not claim (the escape ladder, cancel, exit).
    fn global_key(&mut self, data: &str) -> Action {
        // Prompt jump (FR-UI-11): the authoritative matcher, since the
        // arrow-modifier dialects are not all round-tripped by `parse_key`.
        if self.keybindings.matches(data, "app.prompt.previous") {
            self.jump_prompt(-1);
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.prompt.next") {
            self.jump_prompt(1);
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.clear") {
            return if self.cancellable_turn_visible() {
                Action::CancelTurn
            } else if !self.editor.text().is_empty() {
                // gh #225: the hint promises `clear/exit` - text
                // present clears (and stands down an armed exit).
                self.editor.set_text("");
                self.world.ctrl_c_armed = false;
                Action::Continue
            } else if self.world.ctrl_c_armed {
                self.world.ctrl_c_armed = false;
                Action::Exit
            } else {
                self.world.ctrl_c_armed = true;
                Action::Continue
            };
        }
        // Gh #132: pi's double-escape sits ahead of the interrupt arm:
        // a lone Escape with an empty editor re-arms the 500 ms window
        // (the editor consumed its own Escapes on the way here: popup,
        // search, non-empty buffer). A running turn keeps its cancel
        // semantics, and a non-empty editor keeps the disarm below, so
        // this never fires mid-turn or off a clearing key.
        if !self.turn_running
            && keys::parse_key(data).as_deref() == Some("escape")
            && self.editor.text().trim().is_empty()
        {
            // The interrupt arm's disarm rides along: every Escape
            // still stands down an armed Ctrl+C.
            self.world.ctrl_c_armed = false;
            return self.double_escape();
        }
        // Gh #403 gap 4: a suspend is about the process, not the prompt.
        if self.keybindings.matches(data, "app.suspend") {
            return Action::Suspend;
        }
        if self.keybindings.matches(data, "app.interrupt") {
            return if self.cancellable_turn_visible() {
                Action::CancelTurn
            } else {
                self.world.ctrl_c_armed = false;
                Action::Continue
            };
        }
        if self.keybindings.matches(data, "app.message.followUp")
            && (self.turn_running || self.compacting)
        {
            // Queue a follow-up for turn end (ADR-0038) - unless it is a
            // slash command, which dispatches as a command, never as
            // model text (composer-polish fold-in). Gh #401: the same key
            // works during a compaction, where the message parks in the
            // compaction queue with its follow-up mode intact.
            let text = self.editor.submit();
            if text.trim().is_empty() {
                return Action::Continue;
            }
            if text.trim().starts_with('/') {
                if is_turn_boundary_command(text.trim()) {
                    self.queue_command(text, lca_protocol::SubmitMode::FollowUp);
                    return Action::Continue;
                }
                return self.dispatch_command(text.trim());
            }
            if self.compacting && !self.turn_running {
                self.queue_for_compaction(text, lca_protocol::SubmitMode::FollowUp);
                return Action::Continue;
            }
            self.queue_submit(text, lca_protocol::SubmitMode::FollowUp);
            return Action::Continue;
        }
        Action::Continue
    }

    /// Handle a submitted editor buffer.
    pub(in crate::chat) fn on_submit(&mut self, text: String) -> Action {
        self.submit_text(text, lca_protocol::SubmitMode::Steer)
    }

    /// Submit a buffer in one mode (gh #401). `Enter` submits as a steer,
    /// `Alt+Enter` as a follow-up; the mode only matters when the submit
    /// cannot start a turn, where it decides how the queue drains.
    pub(in crate::chat) fn submit_text(
        &mut self,
        text: String,
        mode: lca_protocol::SubmitMode,
    ) -> Action {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Action::Continue;
        }
        // `!cmd` / `!!cmd` shell mode (FR-UI-14).
        if trimmed.starts_with('!') {
            return self.run_shell_mode(trimmed);
        }
        if trimmed.starts_with('/') {
            // A slash command typed mid-turn never reaches the model as
            // text: turn-boundary commands queue as commands for turn
            // end, the rest dispatch at once (composer-polish fold-in).
            if self.turn_running && is_turn_boundary_command(trimmed) {
                self.queue_command(trimmed.to_string(), lca_protocol::SubmitMode::Steer);
                return Action::Continue;
            }
            return self.dispatch_command(trimmed);
        }
        // Gh #401 (pi's `queueCompactionMessage`): a compaction owns the
        // session, so a submit during it parks in the compaction queue
        // and flushes when it ends. Extension commands above still run
        // immediately, which is pi's rule and keeps `/...` responsive.
        if self.compacting && !self.turn_running {
            self.queue_for_compaction(text, mode);
            return Action::Continue;
        }
        // A turn needs a model; say so rather than failing deep in the
        // provider with a transport error.
        if self.model_label().trim().is_empty() {
            self.say(
                "No model is active. Use /login to sign in, or /model to choose one.".to_string(),
            );
            return Action::Continue;
        }
        // ADR-0038: while a turn runs, a submitted message queues rather
        // than starting a second turn. Enter steers (injected at the next
        // boundary); Alt+Enter queues a follow-up for turn end.
        if self.turn_running {
            self.queue_submit(text, mode);
            return Action::Continue;
        }
        self.transcript.push_user(text.clone());
        self.submitted_queue = None;
        self.submitted = Some(text);
        Action::Submit
    }
}
