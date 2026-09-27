//! The interactive loop on the new engine, replacing the legacy ratatui
//! `run`. Raw input from `ProcessTerminal` is adapted to the state
//! machine's `KeyEvent`; the state is rendered by `render_state` and diffed
//! by `MainScreenRenderer`.

use std::sync::mpsc::Receiver;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lca_protocol::{StopReason, TurnOutcome, TurnStatus, Usage};
use lca_tui::engine::alt_screen::AltScreenRenderer;
use lca_tui::engine::terminal::{ProcessTerminal, Terminal};

use crate::render::render_state;
use crate::state::{
    Action, PermissionModal, PromptRequest, TurnChannels, TurnRunner, TurnStatusLine, UiOptions,
    UiState, handle_key,
};

/// Adapt a raw input sequence to a crossterm `KeyEvent` for the state
/// machine. The engine's parser is authoritative; this is only the bridge.
pub fn raw_to_key_event(data: &str) -> Option<KeyEvent> {
    let id = lca_tui::engine::keys::parse_key(data)?;
    let parts: Vec<&str> = id.split('+').collect();
    let name = *parts.last()?;
    let mut mods = KeyModifiers::NONE;
    for p in &parts[..parts.len().saturating_sub(1)] {
        match *p {
            "ctrl" => mods |= KeyModifiers::CONTROL,
            "alt" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            "super" => mods |= KeyModifiers::META,
            _ => {}
        }
    }
    let code = match name {
        "enter" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageUp" => KeyCode::PageUp,
        "pageDown" => KeyCode::PageDown,
        "space" => KeyCode::Char(' '),
        other if other.chars().count() == 1 => KeyCode::Char(other.chars().next().unwrap()),
        _ => return None,
    };
    Some(KeyEvent::new(code, mods))
}

/// Run the interface until the user exits.
pub fn run(options: UiOptions, runner: TurnRunner) -> anyhow::Result<i32> {
    let mut state = UiState::new(options);
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

    let mut active_turn: Option<std::thread::JoinHandle<TurnOutcome>> = None;
    let mut turn_rx: Option<Receiver<lca_protocol::TurnEvent>> = None;
    let mut prompt_rx: Option<Receiver<PromptRequest>> = None;
    let mut cancel_flag: Option<lca_tools::CancelFlag> = None;
    let mut next_input: Option<String> = None;
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
                    state.on_turn_event(event);
                }
            }
            turn_rx = None;
            prompt_rx = None;
            cancel_flag = None;
            if let Some(error) = &outcome.error {
                state.notice = Some(crate::state::sanitize_text(error));
            }
            state.usage.cost += outcome.usage.cost;
            dirty = true;
        }

        // Start a submitted turn.
        if !state.turn_running
            && let Some(text) = next_input.take()
        {
            let (event_tx, event_rx) = std::sync::mpsc::sync_channel(256);
            let (prompt_tx, prompt_rx_inner) = std::sync::mpsc::sync_channel(4);
            let cancel = lca_tools::CancelFlag::new();
            cancel_flag = Some(cancel.clone());
            let channels = TurnChannels {
                events: event_tx,
                prompt: prompt_tx,
            };
            // Issue #5: the user's own prompt must appear in the transcript,
            // distinct from the answer. The state machine never recorded it.
            let prompt_text = text.clone();
            state.scrollback.push(format!(
                "{}\x1b[1;36m{}\x1b[0m",
                "› ",
                prompt_text.replace('\n', "\n› ")
            ));
            state.turn_running = true;
            state.turn_status = Some(TurnStatusLine {
                text: "running...".into(),
            });
            state.notice = None;
            turn_rx = Some(event_rx);
            prompt_rx = Some(prompt_rx_inner);
            active_turn = Some(runner(text, channels, cancel));
            dirty = true;
        }

        // Drain worker channels.
        if let Some(rx) = &turn_rx {
            while let Ok(event) = rx.try_recv() {
                state.on_turn_event(event);
                dirty = true;
            }
        }
        if state.permission.is_none()
            && let Some(rx) = &prompt_rx
            && let Ok(request) = rx.try_recv()
        {
            state.permission = Some(PermissionModal {
                action: request.action,
                respond: Some(request.respond),
            });
            dirty = true;
        }

        if dirty {
            let width = terminal.columns();
            let height = terminal.rows();
            let document = render_state(&state, width, height);
            // The viewport is the tail of the document, minus the scroll
            // offset (wheel / selection auto-scroll).
            let total = document.len();
            let end = total.saturating_sub(renderer.scroll as usize);
            let start = end.saturating_sub(height as usize);
            let viewport: Vec<String> = document[start..end].to_vec();
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
                // Bracketed paste: insert verbatim.
                if let Some(rest) = data.strip_prefix("\x1b[200~")
                    && let Some(content) = rest.strip_suffix("\x1b[201~")
                {
                    state.insert_at_cursor(content);
                    dirty = true;
                    continue;
                }
                let Some(key) = raw_to_key_event(&data) else {
                    continue;
                };
                match handle_key(&mut state, key) {
                    Action::Continue => {}
                    Action::Submit => {
                        next_input = state.history.last().cloned();
                    }
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
            state.resize(terminal.columns(), terminal.rows());
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
