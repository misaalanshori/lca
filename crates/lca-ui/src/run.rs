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
use crate::state::{
    Action, DialogExchange, PermissionModal, PromptRequest, TurnChannels, TurnRunner, UiOptions,
};

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

/// The loud report for a `keybindings.toml` that failed, overrode nothing
/// known, or double-claims a key (gh #66): `None` when the file parsed
/// clean (or is absent). The caller shows it on the startup notice and
/// keeps defaults - the failure mode being killed is the silent typo.
fn keybinding_problems(
    load_error: Option<String>,
    unknown: &[String],
    invalid: &[(String, String)],
    conflicts: &[lca_tui::engine::keybindings::KeybindingConflict],
) -> Option<String> {
    let mut problems = Vec::new();
    if let Some(err) = load_error {
        problems.push(err);
    }
    if !unknown.is_empty() {
        problems.push(format!(
            "unknown action(s): {} - keeping defaults for those",
            unknown.join(", ")
        ));
    }
    for (action, key) in invalid {
        problems.push(format!(
            "invalid key {key} for {action} - keeping defaults for it"
        ));
    }
    for conflict in conflicts {
        problems.push(format!(
            "key {} claimed by {} - first match wins",
            conflict.key,
            conflict.keybindings.join(", ")
        ));
    }
    if problems.is_empty() {
        return None;
    }
    Some(format!("keybindings.toml: {}", problems.join("; ")))
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
            // gh #10: the reply carries the background itself, not just
            // its scheme - a `system` theme rebuilds from it.
            chat.apply_terminal_background(rgb);
        }
        return InputResult::Continue;
    }
    if let Some(scheme) = lca_tui::engine::colors::parse_terminal_color_scheme_report(data) {
        chat.apply_detected_scheme(scheme);
        return InputResult::Continue;
    }
    // Mouse (selection, wheel) is the renderer's - except wheel motion
    // over the autocomplete popup, which scrolls the offers (gh #175,
    // pi's select-list wheel path), and motion and clicks over
    // extension regions, which the host maps first (gh #172).
    if data.starts_with("\x1b[<") {
        if let Some(mouse) = lca_tui::engine::alt_screen::parse_sgr_mouse(data)
            && mouse.bits & 64 != 0
        {
            let (width, height) = chat.world.size;
            let delta = if mouse.bits & 3 == 0 { -1 } else { 1 };
            let (col, row) = (mouse.x.saturating_sub(1), mouse.y.saturating_sub(1));
            // A wheel over an extension region scrolls it (gh #172):
            // scroll containers glide, dialog selects move, and the
            // gesture never reaches the transcript behind a modal.
            if chat.wheel_extension(col, row, width, height, delta) {
                return InputResult::Continue;
            }
            let row = mouse.y.saturating_sub(1);
            if let Some((top, len)) = chat.popup_rect(width, height)
                && row >= top
                && row < top.saturating_add(len)
            {
                chat.editor.move_suggestion(delta);
                return InputResult::Continue;
            }
        }
        screen.handle_mouse(data);
        // gh #11: a completed single click hit-tests the transcript
        // before selection claims it. A toggled run drops the row's link
        // with it (the click chose the run, not the link); anything
        // else falls through to the link and copy paths below.
        // Extension cells map first (gh #172): a mapped click never
        // reaches the transcript, and a swallowed one eats the link
        // behind the chrome with it.
        if let Some((col, row)) = screen.take_clicked_cell() {
            let (width, height) = chat.world.size;
            match chat.click_extension(col, row, width, height) {
                Some(_) => {
                    let _ = screen.take_clicked_link();
                }
                None => match chat.click_at(col, row, screen.scroll(), width, height) {
                    ClickOutcome::ThinkingToggled => {
                        let _ = screen.take_clicked_link();
                    }
                    ClickOutcome::JumpBottom => screen.set_scroll(0),
                    ClickOutcome::SuggestionAccepted => {}
                    ClickOutcome::Ignored => {}
                },
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
pub fn run(mut options: UiOptions, runner: TurnRunner) -> anyhow::Result<i32> {
    // gh #66: the user's key overrides replace defaults (an empty list
    // disables); anything the registry never defined, any conflict, or a
    // load failure is reported loud on the startup notice - a typo keeps
    // defaults, never silently nothing.
    let keybindings = Arc::new(KeybindingsManager::with_user_bindings(std::mem::take(
        &mut options.keybinding_overrides,
    )));
    let keybinding_notice = keybinding_problems(
        options.keybinding_error.clone(),
        &keybindings.unknown_actions(),
        keybindings.invalid_keys(),
        keybindings.conflicts(),
    );
    // One permission-prompt channel for the session, not one per turn
    // (gh #31 review): the host's endpoint consent asks outside a turn
    // too - when `/model` discovers an ungranted endpoint - and the
    // modal can only appear if this receiver is still being drained.
    let (prompt_tx, prompt_rx) = std::sync::mpsc::sync_channel(4);
    *options
        .prompt_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(prompt_tx.clone());
    // The dialog channel (gh #124): same session-lifetime shape as the
    // prompt above - a tool asking mid-turn and a command asking
    // between turns both reach the same drain.
    let (dialog_tx, dialog_rx) = std::sync::mpsc::sync_channel(4);
    *options
        .dialog_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(dialog_tx.clone());
    // #109: positional CLI messages arrive exactly as if typed — the
    // first paints its user band and runs when the loop reaches
    // `TurnState::start`, the rest queue as follow-ups (their bands
    // appear at flush, with the marker, like any queued message).
    let mut initial_messages = std::mem::take(&mut options.initial_messages);
    let mut chat = Chat::new(options, keybindings);
    // gh #66: a bad keybindings file is loud from the first frame.
    if let Some(notice) = keybinding_notice {
        chat.world.notice = Some(notice);
    }
    if !initial_messages.is_empty() {
        let first = initial_messages.remove(0);
        chat.transcript.push_user(first.clone());
        chat.submitted = Some(first);
        for message in initial_messages {
            chat.queue_submit(message, lca_protocol::SubmitMode::FollowUp);
        }
    }
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

    let mut turns = TurnState::new(prompt_tx, prompt_rx, dialog_rx);
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
    /// The session's dialog receiver (gh #124): same lifetime, so a
    /// question asked outside a turn still reaches its modal.
    dialog_rx: Receiver<DialogExchange>,
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
        dialog_rx: Receiver<DialogExchange>,
    ) -> TurnState {
        TurnState {
            active: None,
            turn_rx: None,
            prompt_rx,
            prompt_tx,
            dialog_rx,
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
#[path = "run_tests.rs"]
mod tests;
