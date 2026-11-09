//! The host'"'"'s own prompt surfaces: what the CLI answers when *it* needs to ask.
//!
//! Split from `tui/mod.rs` at gate 11'"'"'s 1,200-line ceiling (gh #543). Two types
//! live here and they are the same subject: the CLI asks through the interface
//! when no turn owns the keyboard. `SessionPrompt` carries a permission decision
//! (gh #31 review), `SessionDialogs` carries the four dialog shapes
//! (gh #124) - and both answer the denied value when the loop is gone, which is
//! the honesty the headless contract (gh #124) shares.
//!
//! This is *not* the same subject as `hooks.rs`'"'"'s extension-dialog prompters,
//! which the gh #543 brief proposed splitting out: those live in
//! `lca_permissions::DialogPrompt` impls on the **host** side and are reached
//! through `UiHooks`, while these are the CLI'"'"'s own session-lifetime senders.
//! Two different questions, two files - and `session_hooks.rs` (the session list /
//! switch / fork builders) is a third subject again, already separated.
//!
//! `SessionPrompt` stays `pub(super)` and `SessionDialogs`'"'"'s methods keep their
//! reach, so `commands.rs`'"'"'s `super::SessionPrompt` and `Ui::new`'"'"'s construction
//! still compile unchanged.

use std::sync::{Arc, Mutex};

use lca_permissions::{Decision, PermissionPrompt, ProposalDiff};

/// The prompt the host's own consent asks through when no turn is running:
/// it sends the request into the interface's session channel and waits for
/// the answer, exactly as a turn's prompt does, so the *same*
/// [`crate::net_consent::endpoint_consent`] seam works before `/model`'s
/// live discovery (gh #31 review; gh #29's rule that every live request
/// to a profile endpoint is consented first). With no channel yet - before
/// `run` starts - it denies, which is the honest answer with nobody to ask.
pub(super) struct SessionPrompt {
    // `pub(in crate::tui)`: exactly the reach a private field of `tui/mod.rs`
    // had - `commands.rs` and `Ui::new` construct it, nothing outside `tui` does.
    pub(super) slot: Arc<Mutex<Option<std::sync::mpsc::SyncSender<lca_ui::PromptRequest>>>>,
}

impl PermissionPrompt for SessionPrompt {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        self.ask_annotated(action, None)
    }

    /// #78: tool hints ride to the prompt surface; the decision path
    /// never reads them.
    fn ask_annotated(
        &mut self,
        action: &lca_permissions::Action,
        annotations: Option<&lca_protocol::ToolAnnotations>,
    ) -> Decision {
        let Some(sender) = self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        else {
            return Decision::Denied;
        };
        let (respond, response) = std::sync::mpsc::sync_channel(1);
        if sender
            .send(lca_ui::PromptRequest {
                action: action.display_annotated(annotations),
                respond,
            })
            .is_err()
        {
            return Decision::Denied;
        }
        response.recv().unwrap_or(Decision::Denied)
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// The session-lifetime dialog bridge (gh #124): `SharedDialogs` routes
/// here, and each question rendezvous with the TUI loop over
/// `dialog_slot`. A dead loop answers the denied values, exactly like
/// [`SessionPrompt`] denies a dead prompt.
pub(super) struct SessionDialogs {
    pub(super) slot: Arc<Mutex<Option<std::sync::mpsc::SyncSender<lca_ui::DialogExchange>>>>,
}

impl SessionDialogs {
    /// Ask one question: send the exchange, wait for the answer, and map
    /// a dead loop to `denied`.
    fn ask(&mut self, dialog: lca_protocol::UiDialog) -> lca_protocol::DialogAnswer {
        // Decided up front: both dead-loop paths below answer this.
        let denied = denied_answer(&dialog);
        let Some(sender) = self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        else {
            return denied;
        };
        let (respond, response) = std::sync::mpsc::sync_channel(1);
        if sender
            .send(lca_ui::DialogExchange { dialog, respond })
            .is_err()
        {
            return denied;
        }
        response.recv().unwrap_or(denied)
    }
}

/// The denied value for one question (gh #124's headless contract,
/// shared with the dead-loop path above).
fn denied_answer(dialog: &lca_protocol::UiDialog) -> lca_protocol::DialogAnswer {
    match dialog {
        lca_protocol::UiDialog::Confirm { .. } => lca_protocol::DialogAnswer::Confirm(false),
        lca_protocol::UiDialog::Select { .. } => lca_protocol::DialogAnswer::Select(None),
        lca_protocol::UiDialog::Input { .. } => lca_protocol::DialogAnswer::Input(None),
        lca_protocol::UiDialog::Notify { .. } => lca_protocol::DialogAnswer::Notify,
    }
}

impl lca_permissions::DialogPrompt for SessionDialogs {
    fn confirm(&mut self, title: &str, message: &str) -> bool {
        matches!(
            self.ask(lca_protocol::UiDialog::Confirm {
                title: title.to_string(),
                message: message.to_string(),
            }),
            lca_protocol::DialogAnswer::Confirm(true)
        )
    }

    fn select(&mut self, title: &str, options: &[String]) -> Option<String> {
        match self.ask(lca_protocol::UiDialog::Select {
            title: title.to_string(),
            options: options.to_vec(),
        }) {
            lca_protocol::DialogAnswer::Select(choice) => choice,
            _ => None,
        }
    }

    fn input(&mut self, label: &str, placeholder: Option<&str>) -> Option<String> {
        match self.ask(lca_protocol::UiDialog::Input {
            label: label.to_string(),
            placeholder: placeholder.map(str::to_string),
        }) {
            lca_protocol::DialogAnswer::Input(text) => text,
            _ => None,
        }
    }

    fn notify(&mut self, message: &str, level: &str) {
        let _ = self.ask(lca_protocol::UiDialog::Notify {
            message: message.to_string(),
            level: level.to_string(),
        });
    }
}
