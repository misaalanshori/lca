//! The interactive loop on the engine: raw input flows through the engine's
//! parser into the chat's pi pipeline; the chat composes line strings and
//! the active renderer diffs them. The interactive path never goes through
//! a second input crate (ADR-0037).
//!
//! Two renderers coexist (FR-UI-21): the fullscreen (alt-screen) renderer
//! owns selection and is the default, and the main-screen renderer leaves
//! the terminal's own scrollback and selection in charge. `/fullscreen`
//! swaps them at runtime.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use lca_protocol::{StopReason, TurnOutcome, TurnStatus, Usage};
use lca_tui::engine::alt_screen::AltScreenRenderer;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::main_screen::MainScreenRenderer;
use lca_tui::engine::terminal::{InputHandler, ProcessTerminal, ResizeHandler, Terminal};

use crate::chat::Chat;
use crate::state::{Action, PermissionModal, PromptRequest, TurnChannels, TurnRunner, UiOptions};

/// The active screen renderer.
enum Screen {
    /// Fullscreen with app-owned selection.
    Alt(AltScreenRenderer),
    /// The terminal's own scrollback and selection.
    Main(MainScreenRenderer),
}

impl Screen {
    fn is_fullscreen(&self) -> bool {
        matches!(self, Screen::Alt(_))
    }

    fn enter(&mut self, term: &mut dyn Terminal) {
        if let Screen::Alt(r) = self {
            r.enter(term);
        }
    }

    fn leave(&mut self, term: &mut dyn Terminal, preserve: bool) {
        if let Screen::Alt(r) = self {
            r.leave(term, preserve);
        }
    }

    fn scroll(&self) -> u16 {
        match self {
            Screen::Alt(r) => r.scroll,
            Screen::Main(_) => 0,
        }
    }

    fn render(
        &mut self,
        term: &mut dyn Terminal,
        lines: Vec<String>,
        width: u16,
        height: u16,
    ) -> Option<(u16, u16)> {
        match self {
            Screen::Alt(r) => r.render_lines(term, lines, width, height),
            Screen::Main(r) => r.render(term, lines, width, height),
        }
    }

    fn handle_mouse(&mut self, data: &str) -> bool {
        match self {
            Screen::Alt(r) => r.handle_input(data),
            Screen::Main(_) => false,
        }
    }

    fn copy_on_select(&self) -> bool {
        matches!(self, Screen::Alt(r) if r.copy_on_select)
    }

    fn selected_text(&self) -> String {
        match self {
            Screen::Alt(r) => r.selected_text(),
            Screen::Main(_) => String::new(),
        }
    }

    fn copy_osc52(&self, term: &mut dyn Terminal, text: &str) {
        if let Screen::Alt(r) = self {
            r.copy_osc52(term, text);
        }
    }

    fn set_scroll(&mut self, scroll: u16) {
        if let Screen::Alt(r) = self {
            r.scroll = scroll;
        }
    }
}

fn switch_screen(screen: &mut Screen, fullscreen: bool, term: &mut dyn Terminal) {
    match (screen.is_fullscreen(), fullscreen) {
        (true, false) => {
            screen.leave(term, true);
            *screen = Screen::Main(MainScreenRenderer::new());
        }
        (false, true) => {
            let mut alt = AltScreenRenderer::new();
            alt.enter(term);
            *screen = Screen::Alt(alt);
        }
        _ => {}
    }
}

/// Run the interface until the user exits.
///
/// # Errors
/// Returns an error when the terminal cannot be driven (the terminal
/// layer surfaces the I/O failure); the loop itself handles user exit by
/// returning `Ok(0)`.
pub fn run(options: UiOptions, runner: TurnRunner) -> anyhow::Result<i32> {
    let keybindings = Arc::new(KeybindingsManager::new());
    let mut chat = Chat::new(options, keybindings);
    let mut terminal = ProcessTerminal::new();
    let (input_tx, input_rx) = std::sync::mpsc::channel::<String>();
    let (resize_tx, resize_rx) = std::sync::mpsc::channel::<()>();
    let itx = input_tx.clone();
    let rtx = resize_tx.clone();
    terminal.start(
        Box::new(move |data| {
            let _ = itx.send(data);
        }),
        Box::new(move || {
            let _ = rtx.send(());
        }),
    );
    let mut screen = if chat.screen_mode {
        let mut alt = AltScreenRenderer::new();
        alt.enter(&mut terminal);
        Screen::Alt(alt)
    } else {
        Screen::Main(MainScreenRenderer::new())
    };
    terminal.set_title(&format!(
        "lca — {}",
        chat.world.options.workspace.to_string_lossy()
    ));
    chat.world.resize(terminal.columns(), terminal.rows());

    let mut active_turn: Option<std::thread::JoinHandle<TurnOutcome>> = None;
    let mut turn_rx: Option<Receiver<lca_protocol::TurnEvent>> = None;
    let mut prompt_rx: Option<Receiver<PromptRequest>> = None;
    let mut cancel_flag: Option<lca_tools::CancelFlag> = None;
    let mut aborted = false;
    let mut dirty = true;

    let result = 'main: loop {
        // A finished turn.
        if let Some(handle) = active_turn.take_if(|h| h.is_finished()) {
            let outcome = handle.join().unwrap_or_else(|_| TurnOutcome {
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
            chat.current_steer = None;
            terminal.set_progress(false);
            // Steering lifecycle (ADR-0038): an aborted turn returns its
            // queue to the editor; a completed turn runs the queued
            // messages in order.
            if aborted {
                chat.restore_pending();
                aborted = false;
            } else if let Some(next) = chat.take_next_pending() {
                chat.submitted = Some(next);
            }
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
            let steer = lca_protocol::steer_queue();
            let channels = TurnChannels {
                events: event_tx,
                prompt: prompt_tx,
                steer: steer.clone(),
            };
            chat.begin_turn(steer);
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
                // FR-UI-18: an auto-approve countdown, visible and
                // keyboard-interruptible.
                deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
            });
            dirty = true;
        }

        // Auto-approve countdown (FR-UI-18): fires only when the visible
        // deadline passes with no keypress.
        if chat
            .world
            .permission
            .as_ref()
            .is_some_and(|m| m.deadline.is_some_and(|d| std::time::Instant::now() >= d))
        {
            if let Some(modal) = chat.world.permission.take()
                && let Some(respond) = modal.respond
            {
                let _ = respond.send(lca_permissions::Decision::Once);
            }
            dirty = true;
        } else if chat.world.permission.is_some() {
            // Repaint so the countdown ticks visibly.
            dirty = true;
        }

        // The screen mode can change at runtime (FR-UI-21).
        if screen.is_fullscreen() != chat.screen_mode {
            switch_screen(&mut screen, chat.screen_mode, &mut terminal);
            dirty = true;
        }

        if dirty {
            let width = terminal.columns();
            let height = terminal.rows();
            if let Some(scroll) = chat.take_jump_scroll(width, height) {
                screen.set_scroll(scroll);
            }
            let viewport = chat.viewport(width, height, screen.scroll());
            screen.render(&mut terminal, viewport, width, height);
            dirty = false;
        }

        // Input.
        match input_rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(data) => {
                // Mouse (selection, wheel) is the renderer's.
                if data.starts_with("\x1b[<") {
                    screen.handle_mouse(&data);
                    // Copy on release (issue #2): a completed selection is
                    // written to the clipboard via OSC 52.
                    if data.ends_with('m') && screen.copy_on_select() {
                        let text = screen.selected_text();
                        if !text.is_empty() {
                            screen.copy_osc52(&mut terminal, &text);
                        }
                    }
                    dirty = true;
                    continue;
                }
                match chat.handle_key(&data) {
                    Action::Continue => {}
                    Action::Submit => {}
                    Action::CancelTurn => {
                        aborted = true;
                        if let Some(cancel) = &cancel_flag {
                            cancel.cancel();
                        }
                    }
                    Action::ExternalEditor => {
                        if let Some(editor) = chat.world.options.hooks.external_editor.clone() {
                            let text = chat.editor.text();
                            screen.leave(&mut terminal, true);
                            terminal.stop();
                            let edited = editor(&text);
                            let itx = input_tx.clone();
                            let rtx = resize_tx.clone();
                            terminal.start(
                                Box::new(move |d| {
                                    let _ = itx.send(d);
                                }) as InputHandler,
                                Box::new(move || {
                                    let _ = rtx.send(());
                                }) as ResizeHandler,
                            );
                            screen.enter(&mut terminal);
                            if let Some(edited) = edited {
                                chat.editor.set_text(&edited);
                            }
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
    screen.leave(&mut terminal, false);
    terminal.drain_input(1000, 50);
    terminal.stop();
    result.map(|()| 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lca_tui::engine::terminal::FakeTerminal;

    // Verifies: FR-UI-21 - the runtime toggle swaps the renderer both ways.
    #[test]
    fn switch_screen_toggles_the_renderer() {
        let mut term = FakeTerminal::new(80, 24);
        let mut screen = Screen::Main(MainScreenRenderer::new());
        assert!(!screen.is_fullscreen());
        switch_screen(&mut screen, true, &mut term);
        assert!(screen.is_fullscreen());
        // Entering the alt screen wrote the enable sequence.
        assert!(term.output().contains("\x1b[?1049h"));
        switch_screen(&mut screen, false, &mut term);
        assert!(!screen.is_fullscreen());
    }
}
