//! The turn runner (S1): the worker thread that owns one turn, the event
//! sink that forwards to the interface, and the permission prompt that
//! blocks the worker for a human answer.

use std::sync::Arc;

use lca_core::{Agent, TurnEvent, TurnOutcome, TurnSink, TurnStatus};
use lca_permissions::{Decision, PermissionPrompt, ProposalDiff};
use lca_ui::{PromptRequest, TurnChannels};

use super::Ui;

impl Ui {
    /// The turn runner the interface calls once per submission. Each call
    /// gets a fresh worker thread; the runtime is built there, not on the
    /// interface's thread.
    pub(super) fn turn_runner(self: &Arc<Self>) -> lca_ui::TurnRunner {
        let ui = self.clone();
        Box::new(move |text, channels, cancel| {
            let ui = ui.clone();
            std::thread::spawn(move || turn_worker(&ui, text, channels, cancel))
        })
    }
}

/// One turn, on its own thread: install this turn's prompt modal, drain the
/// staged attachments, build a current-thread runtime, and run the agent
/// against the session the interface is showing *now* (R3).
fn turn_worker(
    ui: &Ui,
    text: String,
    channels: TurnChannels,
    cancel: lca_tools::CancelFlag,
) -> TurnOutcome {
    // ADR-0038's marker rides the channels: a prompt that was queued while
    // an earlier turn ran still records as queued when it becomes a turn.
    let queue = channels.queue;
    let mut sink = ChannelSink {
        tx: channels.events.clone(),
    };
    // ADR-0038: the interface's steering queue is drained by the turn loop at
    // each model-call boundary.
    let steer = channels.steer.clone();
    // Install this turn's modal into the shared slot before any extension
    // call runs, so an extension's own process/pty command reaches the same
    // permission prompt the model's tools do.
    let turn_prompt: Arc<std::sync::Mutex<dyn PermissionPrompt>> =
        Arc::new(std::sync::Mutex::new(UiPrompt {
            tx: channels.prompt.clone(),
        }));
    ui.shared_prompt.set(turn_prompt);
    let mut prompt = UiPrompt {
        tx: channels.prompt,
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            return lca_core::TurnOutcome {
                status: TurnStatus::Error,
                stop_reason: lca_core::StopReason::Error,
                usage: Default::default(),
                error: Some(format!("cannot start the async runtime: {err}")),
            };
        }
    };
    let mut tools = ui.tools.lock().unwrap_or_else(|p| p.into_inner());
    // Drain anything `/attach` staged: append each stub to the message text
    // and pass the hashes as the user record's attachments.
    let staged = ui
        .pending_attachments
        .lock()
        .map(|mut guard| std::mem::take(&mut *guard))
        .unwrap_or_default();
    let mut turn_text = text;
    let mut turn_attachments = Vec::new();
    for attachment in &staged {
        turn_text.push('\n');
        turn_text.push_str(&attachment.stub);
        turn_attachments.push(attachment.hash.clone());
    }
    runtime.block_on(async {
        // The session's model is whatever /model last set: the status line
        // and the compaction backend follow the same cell, so every consumer
        // agrees per turn.
        let choice = ui
            .model_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut turn_config = ui.agent_config.clone();
        turn_config.model = choice.id;
        turn_config.model_context_window = choice.window;
        turn_config.reasoning_effort = ui
            .thinking_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        turn_config.steer = steer;
        // R3: read the session the interface is showing *now*, so a `/tree`
        // or `/resume` switch takes effect on the next turn.
        let session = ui
            .current_session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        // gh #29 (QA-004): an endpoint host outside the manifest's fixed
        // hosts is consented to here, on the request path, through the
        // same prompt the model's tool commands ask through - before the
        // stream starts, so the modal lands while the turn is running.
        if let Some(host) = crate::net_consent::env_configured_host(&ui.data)
            && crate::provider_ready(&ui.provider_name, &ui.data)
            && crate::net_consent::endpoint_consent(
                &host,
                &ui.grants,
                &ui.cwd,
                &mut prompt,
                &ui.store,
                &session,
            ) == crate::net_consent::EndpointConsent::Denied
        {
            return lca_core::TurnOutcome {
                status: TurnStatus::Error,
                stop_reason: lca_core::StopReason::Error,
                usage: Default::default(),
                error: Some(crate::net_consent::denied_message(&host)),
            };
        }
        let mut agent = Agent::new(
            &ui.store,
            &session,
            ui.provider.as_ref(),
            &mut tools,
            ui.grants.clone(),
            &mut prompt,
            ui.proposals.as_ref(),
            turn_config,
        );
        agent
            .run_turn_queued(&turn_text, queue, &turn_attachments, &mut sink, &cancel)
            .await
    })
}

/// Forwards turn events to the interface.
struct ChannelSink {
    tx: std::sync::mpsc::SyncSender<TurnEvent>,
}

impl TurnSink for ChannelSink {
    fn on_event(&mut self, event: TurnEvent) {
        let _ = self.tx.send(event);
    }
}

/// The interactive permission prompt: shows the exact action (FR-UI-4) and
/// blocks the worker until the user answers (FR-UI-6's modal rules are
/// enforced by the interface, which only opens it outside a running turn's
/// input path).
struct UiPrompt {
    tx: std::sync::mpsc::SyncSender<PromptRequest>,
}

impl PermissionPrompt for UiPrompt {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        let (respond, response) = std::sync::mpsc::sync_channel(1);
        if self
            .tx
            .send(PromptRequest {
                action: action.display(),
                respond,
            })
            .is_err()
        {
            return Decision::Denied;
        }
        response.recv().unwrap_or(Decision::Denied)
    }

    fn review_proposals(&mut self, diff: &ProposalDiff) -> bool {
        let mut display = String::from("Project proposals changed:\n");
        for (pattern, note) in &diff.added {
            display.push_str(&format!("  + {pattern} ({note})\n"));
        }
        for (pattern, note) in &diff.removed {
            display.push_str(&format!("  - {pattern} ({note})\n"));
        }
        display.push_str("Apply the new set?");
        let (respond, response) = std::sync::mpsc::sync_channel(1);
        if self
            .tx
            .send(PromptRequest {
                action: display,
                respond,
            })
            .is_err()
        {
            return false;
        }
        !matches!(
            response.recv().unwrap_or(Decision::Denied),
            Decision::Denied
        )
    }
}
