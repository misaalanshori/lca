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

use crate::ModelPicker;
use crate::chat::{Chat, ClickOutcome};
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

    /// The teardown contract. `preserve = false` is an exit: both
    /// renderers park the cursor on a fresh line below what they rendered
    /// so the returning shell prompt cannot overwrite it (gh #33).
    /// `preserve = true` is a mid-run handoff - the external editor and the
    /// fullscreen switch - and leaves the terminal exactly as it was.
    fn leave(&mut self, term: &mut dyn Terminal, preserve: bool) {
        match self {
            Screen::Alt(r) => r.leave(term, preserve),
            Screen::Main(r) if !preserve => r.finish(term),
            Screen::Main(_) => {}
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

    fn take_clicked_link(&mut self) -> Option<String> {
        match self {
            Screen::Alt(r) => r.take_clicked_link(),
            Screen::Main(_) => None,
        }
    }

    /// The cell a completed single click landed on, if any (gh #11).
    /// Alt-screen only: the main screen never captures the mouse.
    fn take_clicked_cell(&mut self) -> Option<(u16, u16)> {
        match self {
            Screen::Alt(r) => r.take_clicked_cell(),
            Screen::Main(_) => None,
        }
    }

    fn set_scrollbar(&mut self, scrollbar: Option<(u16, u16)>) {
        if let Screen::Alt(r) = self {
            r.set_scrollbar(scrollbar);
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

/// The notice for an OSC-8 link click (S7): "opened", or "cannot open
/// (reason)" when the opener reported one, else the bare fallback.
fn link_notice(url: &str, outcome: Option<Result<(), String>>) -> String {
    match outcome {
        Some(Ok(())) => format!("opened {url}"),
        Some(Err(reason)) => format!("cannot open {url}: {reason}"),
        None => format!("cannot open {url}"),
    }
}

/// The copy ladder (R6, kept ours for gh #9): the host's verified native
/// clipboard first, then OSC 52 when this screen can send it - and the
/// notice says exactly which step ran, never claiming a step that did
/// not. Returns the notice; the caller puts it on screen.
fn copy_through_ladder(
    text: &str,
    native: Option<&crate::state::ClipboardWriter>,
    osc52: Option<&mut dyn FnMut(&str)>,
) -> String {
    if native.is_some_and(|write| write(text)) {
        return "copied to clipboard".to_string();
    }
    match osc52 {
        Some(send) => {
            send(text);
            "copied (if your terminal blocked the clipboard, nothing was copied)".to_string()
        }
        None => "could not copy: no clipboard tool found (try /fullscreen)".to_string(),
    }
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
        // gh #11: a completed single click hit-tests the transcript
        // before selection claims it. A toggled run drops the row's link
        // with it (the click chose the run, not the link); anything
        // else falls through to the link and copy paths below.
        if let Some((col, row)) = screen.take_clicked_cell() {
            let (width, height) = chat.world.size;
            match chat.click_at(col, row, screen.scroll(), width, height) {
                ClickOutcome::ThinkingToggled => {
                    let _ = screen.take_clicked_link();
                }
                ClickOutcome::JumpBottom => screen.set_scroll(0),
                ClickOutcome::Ignored => {}
            }
        }
        // A click on an OSC-8 link opens it (R6).
        if let Some(url) = screen.take_clicked_link() {
            let outcome = chat
                .world
                .options
                .hooks
                .open_url
                .as_ref()
                .map(|open| open(&url));
            chat.world.notice = Some(link_notice(&url, outcome));
        }
        // Copy on release (issue #2): prefer a verified native
        // clipboard (R6); OSC 52 is the honest fallback, and the
        // notice says which one ran.
        if data.ends_with('m') && screen.copy_on_select() {
            let text = screen.selected_text();
            if !text.is_empty() {
                let notice = {
                    let mut send = |chunk: &str| screen.copy_osc52(terminal, chunk);
                    copy_through_ladder(
                        &text,
                        chat.world.options.hooks.copy_to_clipboard.as_ref(),
                        Some(&mut send),
                    )
                };
                chat.world.notice = Some(notice);
            }
        }
        return InputResult::Continue;
    }
    // gh #9 / pi 1.0.0's `app.message.copy` (Ctrl+X): a selection
    // copies itself, else Chat names the target - the sign-in URL on a
    // waiting login screen, the last assistant message otherwise. The
    // key still reaches `handle_key` below, so the Ctrl+X Ctrl+E chord
    // keeps arming on the same press (FR-UI-15).
    if chat.message_copy_key(data) {
        let selection = screen.selected_text();
        let target = if selection.trim().is_empty() {
            chat.message_copy_text()
        } else {
            Some(selection)
        };
        if let Some(text) = target {
            let notice = {
                let mut send = |chunk: &str| screen.copy_osc52(terminal, chunk);
                copy_through_ladder(
                    &text,
                    chat.world.options.hooks.copy_to_clipboard.as_ref(),
                    matches!(screen, Screen::Alt(_)).then(|| &mut send as &mut dyn FnMut(&str)),
                )
            };
            chat.world.notice = Some(notice);
        }
    }
    // gh #35: pi's `tui.altScreen.bottom` - End returns the fullscreen
    // viewport to the live bottom. Fullscreen only: in main-screen mode
    // End belongs to the editor (its scrollback is the terminal's), and
    // the gates live in the key's own check.
    if chat.alt_screen_bottom(data) {
        screen.set_scroll(0);
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
                let text = chat.editor.expanded_text();
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
    // One permission-prompt channel for the session, not one per turn
    // (gh #31 review): the host's endpoint consent asks outside a turn
    // too - when `/model` discovers an ungranted endpoint - and the
    // modal can only appear if this receiver is still being drained.
    let (prompt_tx, prompt_rx) = std::sync::mpsc::sync_channel(4);
    *options
        .prompt_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(prompt_tx.clone());
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

    let mut turns = TurnState::new(prompt_tx, prompt_rx);
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
        if tick_permission(&mut chat) {
            dirty = true;
        }
        // R4: a background login/identity step reports back here, so a slow
        // OAuth callback never blocks the loop.
        if poll_login(&mut chat) {
            dirty = true;
        }
        // ...and the endpoint consent's second step: rows the host
        // discovered once the grant landed open the picker here (gh #31).
        if poll_pending_models(&mut chat) {
            dirty = true;
        }
        // ...and so does a background `/compact`: the summarization call
        // runs on its own thread and the interface keeps painting.
        if chat.poll_compact() {
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

        // The separator's spinner animates while a turn runs (R2): one
        // frame per cadence, never while idle.
        if chat.tick() {
            dirty = true;
        }

        if dirty {
            let width = terminal.columns();
            let height = terminal.rows();
            if let Some(scroll) = chat.take_jump_scroll(width, height) {
                screen.set_scroll(scroll);
            }
            // gh #35: hold the reader's place across new output (pi's
            // follow-end rule in bottom coordinates), clamp to what the
            // transcript can show, and tell the renderer where the
            // scrollbar was painted - the render side and the copy side
            // read the same geometry, so the adornment can never leak
            // into a selection.
            let scroll = chat.clamp_scroll(screen.scroll(), width, height);
            screen.set_scroll(scroll);
            screen.set_scrollbar(
                chat.scrollbar_for_frame(width, height, scroll)
                    .map(|geometry| (geometry.column, geometry.rows)),
            );
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
struct TurnState {
    active: Option<std::thread::JoinHandle<TurnOutcome>>,
    turn_rx: Option<Receiver<lca_protocol::TurnEvent>>,
    /// The session's prompt receiver: kept for the whole session, so a
    /// consent asked outside a turn still reaches the modal.
    prompt_rx: Receiver<PromptRequest>,
    /// The session's prompt sender; each turn gets a clone.
    prompt_tx: std::sync::mpsc::SyncSender<PromptRequest>,
    cancel: Option<lca_tools::CancelFlag>,
    aborted: bool,
}

impl TurnState {
    /// The session's prompt channel (created once in `run`).
    fn new(
        prompt_tx: std::sync::mpsc::SyncSender<PromptRequest>,
        prompt_rx: Receiver<PromptRequest>,
    ) -> TurnState {
        TurnState {
            active: None,
            turn_rx: None,
            prompt_rx,
            prompt_tx,
            cancel: None,
            aborted: false,
        }
    }

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
        // The prompt receiver stays: a host-side consent asked between
        // turns (the picker's endpoint consent) must still find it.
        self.cancel = None;
        if let Some(error) = &outcome.error {
            chat.world.notice = Some(crate::state::sanitize_text(error));
        }
        // A turn that ends without its `TurnEnded` event still rests the
        // separator.
        chat.separator.idle();
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
        let queue = chat.take_submitted_queue();
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(256);
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
        terminal.set_progress(true);
        self.turn_rx = Some(event_rx);
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
        changed
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

/// Poll a background login/identity step (R4). Returns whether anything
/// changed; `None` from the hook means the step is still running.
fn poll_login(chat: &mut Chat) -> bool {
    let Some(poll) = chat.world.options.hooks.poll_login.clone() else {
        return false;
    };
    match poll() {
        Some(next) => {
            chat.apply_login_next(next);
            true
        }
        None => false,
    }
}

/// Open the `/model` picker when the host has rows ready for it - the
/// consent flow's second step (gh #31 review): the ask ran off-thread, so
/// the modal could render, and the list it produced arrives here. One-shot:
/// the host clears its cell as it hands the rows over.
fn poll_pending_models(chat: &mut Chat) -> bool {
    let Some(pending) = chat.world.options.pending_models.clone() else {
        return false;
    };
    let rows = pending();
    if rows.is_empty() || chat.model_picker.is_some() {
        return false;
    }
    chat.model_picker = Some(ModelPicker::new(rows));
    true
}

/// Fire the auto-approve countdown when it expires (returns whether
/// anything changed). Repaints while it runs so it ticks visibly. It needs
/// no turn state, so it is not a `TurnState` method.
fn tick_permission(chat: &mut Chat) -> bool {
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
                prompt_slot: Default::default(),
                pending_models: None,
                model_label: Arc::new(std::sync::Mutex::new("p/m".into())),
                context_window: Arc::new(std::sync::Mutex::new(0)),
                thinking: Arc::new(std::sync::Mutex::new(None)),
                theme: "auto".to_string(),
                theme_dir: std::path::PathBuf::new(),
                themes: crate::theme::THEMES.iter().map(|s| s.to_string()).collect(),
                initial_lines: Vec::new(),
                initial_records: Vec::new(),
                initial_tail_lines: Vec::new(),
                yolo: false,
                thinking_visibility: Default::default(),
                codeblock_border: Default::default(),
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
                fullscreen: false,
            },
            Arc::new(KeybindingsManager::new()),
        )
    }

    // Verifies: gh #9 / R6 - the copy ladder writes the text through the
    // host's verified native clipboard when it has one, falls back to
    // OSC 52 when the screen can send it, and says which step ran; with
    // neither available it says the copy did not happen instead of
    // pretending.
    #[test]
    fn the_copy_ladder_writes_the_text_and_says_which_step_ran() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let hook: crate::state::ClipboardWriter = {
            let seen = seen.clone();
            Arc::new(move |text: &str| {
                seen.lock().unwrap().push(text.to_string());
                true
            })
        };
        let notice = copy_through_ladder("hello world", Some(&hook), None);
        assert_eq!(notice, "copied to clipboard");
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["hello world".to_string()],
            "the text reaches the clipboard verbatim"
        );

        let mut sent = Vec::new();
        let notice = copy_through_ladder(
            "osc",
            None,
            Some(&mut |text: &str| sent.push(text.to_string())),
        );
        assert_eq!(sent, vec!["osc"], "OSC 52 carries the same text");
        assert!(
            notice.contains("nothing was copied"),
            "the unverified notice is honest: {notice}"
        );

        let notice = copy_through_ladder("nowhere", None, None);
        assert!(
            notice.starts_with("could not copy"),
            "no step ran, so no claim: {notice}"
        );
    }

    // Verifies: gh #9 (pi 1.0.0's `app.message.copy`) - with nothing
    // selected, Ctrl+X copies the last assistant message through that
    // ladder: this host has no native clipboard tool, so OSC 52 reaches
    // the terminal with the message's base64, the notice says the copy
    // is unverified, and the same press still arms the Ctrl+X Ctrl+E
    // chord (FR-UI-15's row is unchanged).
    #[test]
    fn ctrl_x_copies_the_last_assistant_message_through_the_ladder() {
        let mut term = FakeTerminal::new(80, 24);
        let mut screen = Screen::Main(MainScreenRenderer::new());
        switch_screen(&mut screen, true, &mut term);
        let mut chat = chat();
        chat.on_turn_event(lca_protocol::TurnEvent::TextDelta("copy me".into()));
        let (input_tx, _input_rx) = std::sync::mpsc::channel();
        let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
        let outcome = handle_input(
            "\x18",
            &mut chat,
            &mut screen,
            &mut term,
            &input_tx,
            &resize_tx,
            &None,
            &mut false,
        );
        assert!(matches!(outcome, InputResult::Continue));
        let out = term.output();
        assert!(
            out.contains("\x1b]52;c;"),
            "OSC 52 reached the terminal: {out:?}"
        );
        assert!(
            out.contains("Y29weSBtZQ=="),
            "the message, base64 (`copy me`): {out:?}"
        );
        assert_eq!(
            chat.world.notice.as_deref(),
            Some("copied (if your terminal blocked the clipboard, nothing was copied)"),
            "the honest unverified notice"
        );
        // The same press armed the external-editor chord.
        assert_eq!(chat.handle_key("\x05"), Action::ExternalEditor);
    }

    // Verifies: gh #9 (the selection half of `app.message.copy`) - a
    // mouse selection still copies on release through the same ladder
    // and the same honest notice the refactor kept: this row exists
    // because the key path now shares that ladder with it.
    #[test]
    fn a_mouse_selection_still_copies_on_release_through_the_ladder() {
        let mut term = FakeTerminal::new(80, 24);
        let mut screen = Screen::Main(MainScreenRenderer::new());
        switch_screen(&mut screen, true, &mut term);
        let mut chat = chat();
        let (input_tx, _input_rx) = std::sync::mpsc::channel();
        let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
        // A frame with text first, so the selection has rows to read.
        screen.render(&mut term, vec!["hello world".to_string()], 80, 24);
        let mut drive = |data: &str| {
            handle_input(
                data,
                &mut chat,
                &mut screen,
                &mut term,
                &input_tx,
                &resize_tx,
                &None,
                &mut false,
            )
        };
        drive("\x1b[<0;1;1M");
        drive("\x1b[<32;6;1M");
        let outcome = drive("\x1b[<0;6;1m");
        assert!(matches!(outcome, InputResult::Continue));
        let out = term.output();
        assert!(
            out.contains("\x1b]52;c;") && out.contains("aGVsbG8="),
            "the selection (`hello`, base64) went out over OSC 52: {out:?}"
        );
        assert_eq!(
            chat.world.notice.as_deref(),
            Some("copied (if your terminal blocked the clipboard, nothing was copied)"),
            "the same notice the mouse path always gave"
        );
        // The key path did not also fire: one press, one copy.
        assert!(!out.contains("Y29weSBtZQ=="), "nothing else was copied");
    }

    // Verifies: gh #35 - End (pi's `tui.altScreen.bottom`) returns the
    // fullscreen viewport to the live bottom, and only fullscreen: in
    // main-screen mode - and while a modal owns the keyboard - the key
    // is not a viewport key at all, so the editor keeps it.
    #[test]
    fn end_returns_the_fullscreen_viewport_to_the_live_bottom() {
        let mut term = FakeTerminal::new(80, 24);
        let mut screen = Screen::Main(MainScreenRenderer::new());
        switch_screen(&mut screen, true, &mut term);
        let mut chat = chat();
        chat.screen_mode = true;
        screen.set_scroll(12);
        let (input_tx, _input_rx) = std::sync::mpsc::channel();
        let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
        let outcome = handle_input(
            "\x1b[F",
            &mut chat,
            &mut screen,
            &mut term,
            &input_tx,
            &resize_tx,
            &None,
            &mut false,
        );
        assert!(matches!(outcome, InputResult::Continue));
        assert_eq!(screen.scroll(), 0, "End jumped to the live bottom");

        // The gate's own half: main-screen mode and an open modal never
        // claim the key.
        chat.screen_mode = false;
        assert!(
            !chat.alt_screen_bottom("\x1b[F"),
            "main screen: End stays with the editor"
        );
        chat.screen_mode = true;
        chat.world.show_permission("rm -rf /tmp/x".into());
        assert!(
            !chat.alt_screen_bottom("\x1b[F"),
            "a modal owns the keyboard, not the viewport"
        );
    }

    // Verifies: gh #33 - the exit teardown (`preserve = false`) parks the
    // cursor below the transcript in main-screen mode too; `Screen::leave`
    // used to delegate only to `Screen::Alt`, so the main-screen path
    // emitted nothing and the shell prompt landed on LCA's content.
    #[test]
    fn screen_leave_parks_the_cursor_in_main_screen_mode() {
        let mut term = FakeTerminal::new(80, 24);
        let mut screen = Screen::Main(MainScreenRenderer::new());
        screen.render(
            &mut term,
            vec!["transcript".into(), "footer".into()],
            80,
            24,
        );
        let _ = term.take_output();

        screen.leave(&mut term, false);
        let out = term.take_output();

        assert!(
            out.ends_with("\r\n\x1b[?7h\x1b[?25h"),
            "the exit write parks the cursor on a fresh line: {out:?}"
        );
    }

    // Verifies: gh #33's call-site audit - `preserve = true` is the
    // mid-run handoff (the external editor and the fullscreen switch), not
    // an exit, so it must still write nothing in main-screen mode.
    #[test]
    fn a_preserving_leave_in_main_screen_mode_writes_nothing() {
        let mut term = FakeTerminal::new(80, 24);
        let mut screen = Screen::Main(MainScreenRenderer::new());
        screen.render(&mut term, vec!["transcript".into()], 80, 24);
        let _ = term.take_output();

        screen.leave(&mut term, true);

        assert!(
            term.take_output().is_empty(),
            "a handoff leaves the terminal exactly as it was"
        );
    }

    // Verifies: S7 - a link click names the reason when the opener fails,
    // so "cannot open" never hides a missing launcher behind a bare no.
    #[test]
    fn a_failed_link_open_says_why() {
        assert_eq!(link_notice("https://x", Some(Ok(()))), "opened https://x");
        assert_eq!(
            link_notice(
                "https://x",
                Some(Err("no web browser found to open link".into()))
            ),
            "cannot open https://x: no web browser found to open link"
        );
        assert_eq!(link_notice("https://x", None), "cannot open https://x");
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
        // Not due: it keeps the modal and asks for a repaint.
        assert!(tick_permission(&mut chat));
        assert!(chat.world.permission.is_some());
        assert!(rx.try_recv().is_err(), "nothing fired yet");
        // Due: it fires `Once` and closes the modal.
        if let Some(modal) = chat.world.permission.as_mut() {
            modal.deadline = Some(std::time::Instant::now());
        }
        assert!(tick_permission(&mut chat));
        assert!(chat.world.permission.is_none());
        assert_eq!(rx.try_recv().ok(), Some(lca_permissions::Decision::Once));
        // Nothing open: nothing to do.
        assert!(!tick_permission(&mut chat));
    }

    // Verifies: gh #11 - a click's bytes travel the loop's input path:
    // SGR press+release through the alt-screen renderer hit-tests the
    // transcript and toggles the reasoning run, end to end (the same
    // bytes in main-screen mode stay with the terminal: no toggle).
    #[test]
    fn a_click_on_a_reasoning_row_toggles_the_run_through_the_loop() {
        use lca_tui::engine::text::strip_terminal_sequences;
        let strip = |lines: &[String]| -> Vec<String> {
            lines.iter().map(|l| strip_terminal_sequences(l)).collect()
        };
        let mut term = FakeTerminal::new(80, 24);
        let mut screen = Screen::Main(MainScreenRenderer::new());
        switch_screen(&mut screen, true, &mut term);
        let mut chat = chat();
        chat.screen_mode = true;
        chat.world.resize(80, 24);
        chat.transcript.push_user("quux question");
        chat.on_turn_event(lca_protocol::TurnEvent::ReasoningDelta(
            "alpha\nbeta\ngamma\ndelta\nepsilon".into(),
        ));
        chat.on_turn_event(lca_protocol::TurnEvent::TextDelta("the answer".into()));
        chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
            status: lca_protocol::TurnStatus::Ok,
            stop_reason: lca_protocol::StopReason::Stop,
        });
        let frame = strip(&chat.viewport(80, 24, 0));
        assert!(!frame.iter().any(|l| l.contains("delta")));
        let row = frame
            .iter()
            .position(|l| l.contains("alpha"))
            .expect("a visible reasoning row") as u16;
        let (input_tx, _input_rx) = std::sync::mpsc::channel();
        let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
        let press = format!("\x1b[<0;5;{}M", row + 1);
        let release = format!("\x1b[<0;5;{}m", row + 1);
        // Direct calls (no closure): each call's borrows end on return,
        // so the frame reads between them compile.
        let outcome = handle_input(
            &press,
            &mut chat,
            &mut screen,
            &mut term,
            &input_tx,
            &resize_tx,
            &None,
            &mut false,
        );
        assert!(matches!(outcome, InputResult::Continue));
        let outcome = handle_input(
            &release,
            &mut chat,
            &mut screen,
            &mut term,
            &input_tx,
            &resize_tx,
            &None,
            &mut false,
        );
        assert!(matches!(outcome, InputResult::Continue));
        let frame = strip(&chat.viewport(80, 24, 0));
        assert!(
            frame.iter().any(|l| l.contains("delta")),
            "the click toggled the run through the loop:\n{}",
            frame.join("\n")
        );

        // Main-screen: the same bytes never reach the transcript.
        chat.screen_mode = false;
        switch_screen(&mut screen, false, &mut term);
        // Collapse the run again first (Ctrl+T), so a toggle would show.
        chat.transcript.toggle_thinking_expanded();
        let frame = strip(&chat.viewport(80, 24, 0));
        assert!(!frame.iter().any(|l| l.contains("delta")));
        let outcome = handle_input(
            &press,
            &mut chat,
            &mut screen,
            &mut term,
            &input_tx,
            &resize_tx,
            &None,
            &mut false,
        );
        assert!(matches!(outcome, InputResult::Continue));
        let outcome = handle_input(
            &release,
            &mut chat,
            &mut screen,
            &mut term,
            &input_tx,
            &resize_tx,
            &None,
            &mut false,
        );
        assert!(matches!(outcome, InputResult::Continue));
        let frame = strip(&chat.viewport(80, 24, 0));
        assert!(
            !frame.iter().any(|l| l.contains("delta")),
            "main-screen clicks stay with the terminal:\n{}",
            frame.join("\n")
        );
    }
}
