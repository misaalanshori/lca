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

    fn tick_auto_scroll(&mut self) -> bool {
        match self {
            Screen::Alt(r) => r.tick_auto_scroll(),
            Screen::Main(_) => false,
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

/// What one input event decided about the loop.
enum InputResult {
    /// Keep running.
    Continue,
    /// Leave the interface.
    Exit,
}

/// Handle one raw input event (R16): capability replies, mouse, and keys.
#[allow(clippy::too_many_arguments)]
fn handle_input(
    data: &str,
    chat: &mut Chat,
    screen: &mut Screen,
    terminal: &mut dyn Terminal,
    input_tx: &std::sync::mpsc::Sender<String>,
    resize_tx: &std::sync::mpsc::Sender<()>,
    cancel_flag: &Option<lca_tools::CancelFlag>,
    aborted: &mut bool,
) -> InputResult {
    // Terminal capability replies (R10): the OSC 11 background
    // and the DSR color-scheme report drive the auto theme.
    if lca_tui::engine::colors::is_osc11_background_color_response(data) {
        if let Some(rgb) = lca_tui::engine::colors::parse_osc11_background_color(data) {
            chat.apply_detected_scheme(rgb.scheme());
        }
        return InputResult::Continue;
    }
    if let Some(scheme) = lca_tui::engine::colors::parse_terminal_color_scheme_report(data) {
        chat.apply_detected_scheme(scheme);
        return InputResult::Continue;
    }
    // Mouse (selection, wheel) is the renderer's.
    if data.starts_with("\x1b[<") {
        screen.handle_mouse(data);
        // Copy on release (issue #2): prefer a verified native
        // clipboard (R6); OSC 52 is the honest fallback, and the
        // notice says which one ran.
        if data.ends_with('m') && screen.copy_on_select() {
            let text = screen.selected_text();
            if !text.is_empty() {
                let verified = chat
                    .world
                    .options
                    .hooks
                    .copy_to_clipboard
                    .as_ref()
                    .is_some_and(|write| write(&text));
                if !verified {
                    screen.copy_osc52(terminal, &text);
                }
                chat.world.notice = Some(if verified {
                    "copied to the clipboard".to_string()
                } else {
                    "copied via OSC 52 (unverified)".to_string()
                });
            }
        }
        return InputResult::Continue;
    }
    match chat.handle_key(data) {
        Action::Continue => {}
        Action::Submit => {}
        Action::CancelTurn => {
            *aborted = true;
            if let Some(cancel) = &cancel_flag {
                cancel.cancel();
            }
        }
        Action::ExternalEditor => {
            if let Some(editor) = chat.world.options.hooks.external_editor.clone() {
                let text = chat.editor.text();
                screen.leave(terminal, true);
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
                screen.enter(terminal);
                if let Some(edited) = edited {
                    chat.editor.set_text(&edited);
                }
            }
        }
        Action::Exit => return InputResult::Exit,
    }
    InputResult::Continue
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
    // R10: ask the terminal for its background color and color-scheme
    // preference; the replies set the auto theme when they arrive.
    terminal.write("\x1b]11;?\x07\x1b[?996n");

    let mut turns = TurnState::default();
    let mut dirty = true;

    let result = 'main: loop {
        // Turn lifecycle (R16: `TurnState` owns the running turn).
        if turns.reap(&mut chat, &mut terminal) {
            dirty = true;
        }
        if turns.start(&mut chat, &runner, &mut terminal) {
            dirty = true;
        }
        if turns.drain(&mut chat) {
            dirty = true;
        }
        if turns.tick_permission(&mut chat) {
            dirty = true;
        }

        // Edge auto-scroll while a selection drag sits on a viewport edge
        // (R6).
        if screen.tick_auto_scroll() {
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
                if let InputResult::Exit = handle_input(
                    &data,
                    &mut chat,
                    &mut screen,
                    &mut terminal,
                    &input_tx,
                    &resize_tx,
                    &turns.cancel,
                    &mut turns.aborted,
                ) {
                    break 'main Ok(());
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

    turns.shutdown();
    screen.leave(&mut terminal, false);
    terminal.drain_input(1000, 50);
    terminal.stop();
    result.map(|()| 0)
}

/// The running turn's mutable state (R16), so the loop body stays a loop.
#[derive(Default)]
struct TurnState {
    active: Option<std::thread::JoinHandle<TurnOutcome>>,
    turn_rx: Option<Receiver<lca_protocol::TurnEvent>>,
    prompt_rx: Option<Receiver<PromptRequest>>,
    cancel: Option<lca_tools::CancelFlag>,
    aborted: bool,
}

impl TurnState {
    /// Reap a finished turn (returns whether one finished).
    fn reap(&mut self, chat: &mut Chat, terminal: &mut dyn Terminal) -> bool {
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
        self.prompt_rx = None;
        self.cancel = None;
        if let Some(error) = &outcome.error {
            chat.world.notice = Some(crate::state::sanitize_text(error));
        }
        chat.usage.cost += outcome.usage.cost;
        chat.turn_running = false;
        chat.current_steer = None;
        terminal.set_progress(false);
        // Steering lifecycle (ADR-0038): an aborted turn returns its queue
        // to the editor; a completed turn runs the queued messages in order.
        if self.aborted {
            chat.restore_pending();
            self.aborted = false;
        } else if let Some(next) = chat.take_next_pending() {
            chat.submitted = Some(next);
        }
        true
    }

    /// Start a submitted turn (returns whether one started).
    fn start(&mut self, chat: &mut Chat, runner: &TurnRunner, terminal: &mut dyn Terminal) -> bool {
        if chat.turn_running {
            return false;
        }
        let Some(text) = chat.take_submitted() else {
            return false;
        };
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(256);
        let (prompt_tx, prompt_rx_inner) = std::sync::mpsc::sync_channel(4);
        let cancel = lca_tools::CancelFlag::new();
        self.cancel = Some(cancel.clone());
        let steer = lca_protocol::steer_queue();
        let channels = TurnChannels {
            events: event_tx,
            prompt: prompt_tx,
            steer: steer.clone(),
        };
        chat.begin_turn(steer);
        terminal.set_progress(true);
        self.turn_rx = Some(event_rx);
        self.prompt_rx = Some(prompt_rx_inner);
        self.active = Some(runner(text, channels, cancel));
        true
    }

    /// Drain the turn, shell, and prompt channels (returns whether anything
    /// changed).
    fn drain(&mut self, chat: &mut Chat) -> bool {
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
            && let Some(rx) = &self.prompt_rx
            && let Ok(request) = rx.try_recv()
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
        changed
    }

    /// Fire the auto-approve countdown when it expires (returns whether
    /// anything changed). Repaints while it runs so it ticks visibly.
    fn tick_permission(&mut self, chat: &mut Chat) -> bool {
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
            true
        } else {
            chat.world.permission.is_some()
        }
    }

    /// Cancel and join the running turn on shutdown.
    fn shutdown(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.active.take() {
            let _ = handle.join();
        }
    }
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

    fn chat() -> Chat {
        Chat::new(
            UiOptions {
                model_label: Arc::new(std::sync::Mutex::new("p/m".into())),
                thinking: Arc::new(std::sync::Mutex::new(None)),
                initial_lines: Vec::new(),
                plain: true,
                invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
                slash_commands: Vec::new(),
                models: Vec::new(),
                workspace: std::path::PathBuf::from("."),
                render_regions: None,
                ui_events: None,
                update_notice: None,
                login: None,
                complete_login: None,
                pick_login: None,
                confirm_login_grant: None,
                hooks: crate::state::UiHooks::default(),
                fullscreen: true,
            },
            Arc::new(KeybindingsManager::new()),
        )
    }

    // Verifies: R19 - the auto-approve countdown (FR-UI-18) fires exactly at
    // its deadline, with the same decision pressing Allow would send.
    #[test]
    fn the_auto_approve_countdown_fires_at_the_deadline() {
        let mut chat = chat();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        chat.world.permission = Some(PermissionModal {
            action: "rm -rf /tmp/x".into(),
            respond: Some(tx),
            deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
        });
        let mut turns = TurnState::default();
        // Not due: it keeps the modal and asks for a repaint.
        assert!(turns.tick_permission(&mut chat));
        assert!(chat.world.permission.is_some());
        assert!(rx.try_recv().is_err(), "nothing fired yet");
        // Due: it fires `Once` and closes the modal.
        if let Some(modal) = chat.world.permission.as_mut() {
            modal.deadline = Some(std::time::Instant::now());
        }
        assert!(turns.tick_permission(&mut chat));
        assert!(chat.world.permission.is_none());
        assert_eq!(rx.try_recv().ok(), Some(lca_permissions::Decision::Once));
        // Nothing open: nothing to do.
        assert!(!turns.tick_permission(&mut chat));
    }
}
