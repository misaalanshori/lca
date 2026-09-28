//! The interactive loop on the engine: raw input flows through the engine's
//! parser into the chat's pi pipeline; the chat composes line strings and
//! the alt-screen renderer diffs them. The interactive path never goes
//! through a second input crate (ADR-0037).

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use lca_protocol::{StopReason, TurnOutcome, TurnStatus, Usage};
use lca_tui::engine::alt_screen::AltScreenRenderer;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::terminal::{ProcessTerminal, Terminal};

use crate::chat::Chat;
use crate::state::{Action, PermissionModal, PromptRequest, TurnChannels, TurnRunner, UiOptions};

/// Run the interface until the user exits.
pub fn run(options: UiOptions, runner: TurnRunner) -> anyhow::Result<i32> {
    let keybindings = Arc::new(KeybindingsManager::new());
    let mut chat = Chat::new(options, keybindings);
    let mut terminal = ProcessTerminal::new();
    let (input_tx, input_rx) = std::sync::mpsc::channel::<String>();
    let (resize_tx, resize_rx) = std::sync::mpsc::channel::<()>();
    terminal.start(
        Box::new(move |data| {
            let _ = input_tx.send(data);
        }),
        Box::new(move || {
            let _ = resize_tx.send(());
        }),
    );
    let mut renderer = AltScreenRenderer::new();
    renderer.enter(&mut terminal);
    terminal.set_title(&format!(
        "lca — {}",
        chat.world.options.workspace.to_string_lossy()
    ));

    let mut active_turn: Option<std::thread::JoinHandle<TurnOutcome>> = None;
    let mut turn_rx: Option<Receiver<lca_protocol::TurnEvent>> = None;
    let mut prompt_rx: Option<Receiver<PromptRequest>> = None;
    let mut cancel_flag: Option<lca_tools::CancelFlag> = None;
    let mut dirty = true;

    let result = 'main: loop {
        // A finished turn.
        if active_turn.as_ref().is_some_and(|h| h.is_finished()) {
            let outcome = active_turn
                .take()
                .expect("handle")
                .join()
                .unwrap_or_else(|_| TurnOutcome {
                    status: TurnStatus::Error,
                    stop_reason: StopReason::Error,
                    usage: Usage::default(),
                    error: Some("the turn worker panicked".to_string()),
                });
            if let Some(rx) = &turn_rx {
                while let Ok(event) = rx.try_recv() {
                    chat.on_turn_event(event);
                }
            }
            turn_rx = None;
            prompt_rx = None;
            cancel_flag = None;
            if let Some(error) = &outcome.error {
                chat.world.notice = Some(crate::state::sanitize_text(error));
            }
            chat.usage.cost += outcome.usage.cost;
            chat.turn_running = false;
            terminal.set_progress(false);
            dirty = true;
        }

        // Start a submitted turn.
        if !chat.turn_running
            && let Some(text) = chat.take_submitted()
        {
            let (event_tx, event_rx) = std::sync::mpsc::sync_channel(256);
            let (prompt_tx, prompt_rx_inner) = std::sync::mpsc::sync_channel(4);
            let cancel = lca_tools::CancelFlag::new();
            cancel_flag = Some(cancel.clone());
            let channels = TurnChannels {
                events: event_tx,
                prompt: prompt_tx,
            };
            chat.begin_turn();
            terminal.set_progress(true);
            turn_rx = Some(event_rx);
            prompt_rx = Some(prompt_rx_inner);
            active_turn = Some(runner(text, channels, cancel));
            dirty = true;
        }

        // Drain worker channels.
        if let Some(rx) = &turn_rx {
            while let Ok(event) = rx.try_recv() {
                chat.on_turn_event(event);
                dirty = true;
            }
        }
        if chat.world.permission.is_none()
            && let Some(rx) = &prompt_rx
            && let Ok(request) = rx.try_recv()
        {
            chat.world.permission = Some(PermissionModal {
                action: request.action,
                respond: Some(request.respond),
            });
            dirty = true;
        }

        if dirty {
            let width = terminal.columns();
            let height = terminal.rows();
            let viewport = chat.viewport(width, height, renderer.scroll);
            renderer.render_lines(&mut terminal, viewport, width, height);
            dirty = false;
        }

        // Input.
        match input_rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(data) => {
                // Mouse (selection, wheel) is the renderer's.
                if data.starts_with("\x1b[<") {
                    renderer.handle_input(&data);
                    // Copy on release (issue #2): a completed selection is
                    // written to the clipboard via OSC 52.
                    if data.ends_with('m') && renderer.copy_on_select {
                        let text = renderer.selected_text();
                        if !text.is_empty() {
                            renderer.copy_osc52(&mut terminal, &text);
                        }
                    }
                    dirty = true;
                    continue;
                }
                match chat.handle_key(&data) {
                    Action::Continue => {}
                    Action::Submit => {}
                    Action::CancelTurn => {
                        if let Some(cancel) = &cancel_flag {
                            cancel.cancel();
                        }
                    }
                    Action::Exit => break 'main Ok(()),
                }
                dirty = true;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break 'main Ok(()),
        }

        // Resize.
        if resize_rx.try_recv().is_ok() {
            chat.world.resize(terminal.columns(), terminal.rows());
            dirty = true;
        }
    };

    if let Some(cancel) = cancel_flag {
        cancel.cancel();
    }
    if let Some(handle) = active_turn {
        let _ = handle.join();
    }
    renderer.leave(&mut terminal, false);
    terminal.drain_input(1000, 50);
    terminal.stop();
    result.map(|()| 0)
}
