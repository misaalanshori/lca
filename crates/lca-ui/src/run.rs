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

use lca_tui::engine::alt_screen::AltScreenRenderer;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::main_screen::MainScreenRenderer;
use lca_tui::engine::terminal::{InputHandler, ResizeHandler, Terminal};

use crate::chat::Chat;

use crate::chat_mouse::PickerHit;
use crate::chat_poll::{poll_login, poll_pending_models, tick_permission};
use crate::chat_render::ClickOutcome;
use crate::run_mouse::handle_mouse_event;
use crate::state::{Action, TurnRunner, UiOptions};

/// The active screen renderer.
pub(crate) enum Screen {
    /// Fullscreen with app-owned selection.
    Alt(AltScreenRenderer),
    /// The terminal's own scrollback and selection.
    Main(MainScreenRenderer),
}

use crate::copy_ladder::{CopyReport, copy_through_ladder};

impl Screen {
    pub(crate) fn is_fullscreen(&self) -> bool {
        matches!(self, Screen::Alt(_))
    }

    pub(crate) fn enter(&mut self, term: &mut dyn Terminal) {
        if let Screen::Alt(r) = self {
            r.enter(term);
        }
    }

    /// The teardown contract. `preserve = false` is an exit: both
    /// renderers park the cursor on a fresh line below what they rendered
    /// so the returning shell prompt cannot overwrite it (gh #33).
    /// `preserve = true` is a mid-run handoff - the external editor and the
    /// fullscreen switch - and leaves the terminal exactly as it was.
    pub(crate) fn leave(&mut self, term: &mut dyn Terminal, preserve: bool) {
        match self {
            Screen::Alt(r) => r.leave(term, preserve),
            Screen::Main(r) if !preserve => r.finish(term),
            Screen::Main(_) => {}
        }
    }

    pub(crate) fn scroll(&self) -> u16 {
        match self {
            Screen::Alt(r) => r.scroll,
            Screen::Main(_) => 0,
        }
    }

    /// Paint the frame. `document` is the whole session as rows - the alt screen
    /// keeps it for the exit handoff (gh #374 gap 1); the main screen ignores it,
    /// because its append contract already wrote every row as it went.
    fn render(
        &mut self,
        term: &mut dyn Terminal,
        lines: Vec<String>,
        document: Vec<String>,
        width: u16,
        height: u16,
    ) -> Option<(u16, u16)> {
        match self {
            Screen::Alt(r) => {
                r.set_document(document);
                r.render_lines(term, lines, width, height)
            }
            Screen::Main(r) => r.render(term, lines, width, height),
        }
    }

    fn handle_mouse(&mut self, data: &str) -> bool {
        match self {
            Screen::Alt(r) => {
                r.set_right_click_paste_allowed(
                    lca_tui::engine::alt_screen::right_click_paste_allowed(
                        cfg!(windows),
                        std::env::var("TERM_PROGRAM").ok().as_deref(),
                    ),
                );
                r.handle_input(data)
            }
            Screen::Main(_) => false,
        }
    }

    /// Take a right-button paste request (gh #374 gap 4).
    pub(crate) fn take_right_click_paste(&mut self) -> bool {
        match self {
            Screen::Alt(r) => r.take_right_click_paste(),
            Screen::Main(_) => false,
        }
    }

    /// Whether the pointer hovers the fullscreen scrollbar (gh #164).
    ///
    /// The reader half of [`AltScreenRenderer::set_scrollbar_hover`], so a row can
    /// assert that a gesture really reached the renderer rather than being
    /// swallowed: `FOCUS_OUT` drops the hover, and nothing else on the input path
    /// writes it without a motion event.
    #[cfg(test)]
    pub(crate) fn scrollbar_hovered(&self) -> bool {
        matches!(self, Screen::Alt(r) if r.scrollbar_hovered())
    }

    pub(crate) fn copy_on_select(&self) -> bool {
        matches!(self, Screen::Alt(r) if r.copy_on_select)
    }

    pub(crate) fn selected_text(&self) -> String {
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

    pub(crate) fn set_scrollbar(&mut self, scrollbar: Option<(u16, u16)>) {
        if let Screen::Alt(r) = self {
            r.set_scrollbar(scrollbar);
        }
    }

    pub(crate) fn set_scroll(&mut self, scroll: u16) {
        if let Screen::Alt(r) = self {
            r.scroll = scroll;
        }
    }

    /// Mirror the scrollbar hover flag into the renderer (gh #164).
    pub(crate) fn set_scrollbar_hover(&mut self, hover: bool) {
        if let Screen::Alt(r) = self {
            r.set_scrollbar_hover(hover);
        }
    }

    /// Begin a scrollbar drag: the renderer drops selection so the
    /// gesture scrolls instead (gh #164).
    pub(crate) fn begin_scrollbar_drag(&mut self) {
        if let Screen::Alt(r) = self {
            r.begin_scrollbar_drag();
        }
    }

    /// End a scrollbar drag (gh #164).
    pub(crate) fn end_scrollbar_drag(&mut self) {
        if let Screen::Alt(r) = self {
            r.end_scrollbar_drag();
        }
    }

    /// Drop any selection (gh #167).
    pub(crate) fn clear_selection(&mut self) {
        if let Screen::Alt(r) = self {
            r.clear_selection();
        }
    }

    pub(crate) fn tick_auto_scroll(&mut self) -> bool {
        match self {
            Screen::Alt(r) => r.tick_auto_scroll(),
            Screen::Main(_) => false,
        }
    }
}

/// The fullscreen-exit notice (gh #82): the resume hint names the
/// live session; anything else (or no id) stays quiet. Pure for the
/// row that pins it.
fn exit_hint(tuning: &crate::state::DisplayTuning, session_id: &str) -> Option<String> {
    if tuning.fullscreen_exit_output.as_str() == "resume-hint" && !session_id.is_empty() {
        Some(format!(
            "session continues in scrollback - resume with: {}",
            crate::resume_command::resume_command(session_id, None)
        ))
    } else {
        None
    }
}

pub(crate) fn switch_screen(screen: &mut Screen, fullscreen: bool, term: &mut dyn Terminal) {
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

/// Handle one raw input event (R16): capability replies, mouse, and keys.
///
/// The pointer half lives in [`run_mouse`] (split out for the 1,200-line
/// ceiling); this function is the keyboard half plus the dispatch into it.
#[allow(clippy::too_many_arguments)]
fn handle_input(
    data: &str,
    chat: &mut Chat,
    screen: &mut Screen,
    terminal: &mut dyn Terminal,
    input_tx: &std::sync::mpsc::Sender<String>,
    resize_tx: &std::sync::mpsc::Sender<()>,
    // Gh #400: when startup adopted the terminal, the loop reads the early
    // receiver, not the channel `input_tx` feeds. The `$EDITOR` path stops and
    // restarts the terminal, so it has to re-point at whichever stream the loop
    // is actually draining or the session goes deaf after the editor exits.
    early_receiver: Option<&crate::run_early_input::EarlyStream>,
    cancel_flag: &Option<lca_tools::CancelFlag>,
    aborted: &mut bool,
) -> InputResult {
    // Terminal capability replies (R10 + gh #379 gap 1): the OSC 11 background,
    // the DSR colour-scheme report, and the `DA1` sentinel that closes the query.
    // `theme_query::handle_reply` owns the dispatch and the lifecycle, so the
    // apply path and the composer guard share one recogniser.
    if crate::theme_query::handle_reply(chat, data) {
        return InputResult::Continue;
    }
    // Focus reports (gh #374 gap 2): the alt screen consumes them so a finished
    // selection survives `Alt+Tab` while an interrupted gesture does not.
    //
    // This has to sit **outside** the SGR gate below, and that gate is exactly
    // why the bug was invisible: `screen.handle_mouse` is only called for input
    // starting with `\x1b[<`, and `\x1b[O` does not, so the renderer's focus arm
    // was unreachable. The guard alone would have been dead code (#377/#379's
    // failure mode), so the dispatch and the `?1004h` request both land here.
    //
    // The host also own half the gesture state: `Chat::scroll_drag` is the
    // scrollbar grab offset, which no renderer reset can reach.
    if data == lca_tui::engine::alt_screen::FOCUS_OUT
        || data == lca_tui::engine::alt_screen::FOCUS_IN
    {
        screen.handle_mouse(data);
        if data == lca_tui::engine::alt_screen::FOCUS_OUT {
            chat.scroll_drag = None;
            chat.scrollbar_hover = false;
            chat.drawer_hover = false;
            chat.dismiss_tooltip();
        }
        return InputResult::Continue;
    }
    // Mouse (selection, wheel) is the renderer's - except wheel motion
    // over the autocomplete popup, which scrolls the offers (gh #175,
    // pi's select-list wheel path), and motion and clicks over
    // extension regions, which the host maps first (gh #172).
    if data.starts_with("\x1b[<") {
        // Gh #164: the keystone normalizes first. Press/motion/release
        // route through the dispatch below; the wheel keeps its existing
        // path (shift-jump, extension regions, the popup, transcript).
        if let Some(sgr) = lca_tui::engine::alt_screen::parse_sgr_mouse(data) {
            let event =
                lca_tui::engine::core::MouseEvent::from_sgr(sgr.bits, sgr.x, sgr.y, sgr.press);
            if handle_mouse_event(event, chat, screen) {
                return InputResult::Continue;
            }
        }
        if let Some(mouse) = lca_tui::engine::alt_screen::parse_sgr_mouse(data)
            && mouse.bits & 64 != 0
        {
            // Gh #210: a wheel moves content under a still pointer, so
            // any tooltip (and its clock) goes.
            chat.dismiss_tooltip();
            // Gh #173: Shift+wheel steps between prompts (the loop pins
            // the target); a plain wheel scrolls, as before.
            if let Some(delta) = crate::run_selection::shift_wheel_jump(&mouse) {
                chat.jump_prompt(delta);
                return InputResult::Continue;
            }
            let (width, height) = chat.world.size;
            let delta = if mouse.bits & 3 == 0 { -1 } else { 1 };
            let (col, row) = (mouse.x.saturating_sub(1), mouse.y.saturating_sub(1));
            // A wheel over an extension region scrolls it (gh #172):
            // scroll containers glide, dialog selects move, and the
            // gesture never reaches the transcript behind a modal.
            if chat.wheel_extension(col, row, width, height, delta) {
                return InputResult::Continue;
            }
            // Gh #167: a wheel inside the picker box walks the
            // selection (pi's select-list wheel path), never the
            // transcript behind it. One shared seam with the
            // keyboard paging (gh #226).
            if let Some(hit) = chat.picker_hit(col, row, width, height)
                && !matches!(hit, PickerHit::Backdrop)
            {
                chat.wheel_picker(delta.clamp(-1, 1) as i8);
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
        // Gh #374 gap 4: a right-button press on a Windows-shaped terminal is a
        // paste, read through the one clipboard path (#406) and reported honestly
        // when it finds nothing to paste.
        if screen.take_right_click_paste() {
            input_helpers::paste_from_clipboard(chat);
            return InputResult::Continue;
        }
        // gh #11: a completed single click hit-tests the transcript
        // before selection claims it. A toggled run drops the row's link
        // with it (the click chose the run, not the link); anything
        // else falls through to the link and copy paths below.
        // Extension cells map first (gh #172): a mapped click never
        // reaches the transcript, and a swallowed one eats the link
        // behind the chrome with it.
        if let Some((col, row)) = screen.take_clicked_cell() {
            let (width, height) = chat.world.size;
            // Gh #207: the drawer tab is chrome - it toggles before
            // any region or row behind it answers.
            if chat.click_drawer(col, row, width, height) {
                let _ = screen.take_clicked_link();
            // Gh #165: the composer owns its rows - a click there
            // focuses the editor (dismissing any picker) instead of
            // hit-testing the transcript behind it.
            } else if chat.place_editor_caret(col, row, width, height) {
                let _ = screen.take_clicked_link();
            } else {
                match chat.click_extension(col, row, width, height) {
                    Some(_) => {
                        let _ = screen.take_clicked_link();
                    }
                    None => {
                        let scrolled = screen.scroll();
                        let before = chat.scroll_extent(width, height).0;
                        match chat.click_at(col, row, scrolled, width, height) {
                            ClickOutcome::ThinkingToggled | ClickOutcome::ToolToggled => {
                                let _ = screen.take_clicked_link();
                                // Gh #247: the height delta rides the scroll
                                // offset, so a collapse keeps the clicked card
                                // stationary instead of clamping at the top.
                                let after = chat.scroll_extent(width, height).0;
                                screen.set_scroll(crate::chat_render::anchor_scroll(
                                    scrolled, before, after,
                                ));
                            }
                            ClickOutcome::JumpBottom => screen.set_scroll(0),
                            // Gh #173: stepper clicks only set the target; the
                            // loop pins it on the next frame like a key jump.
                            ClickOutcome::PreviousPrompt => chat.jump_prompt(-1),
                            ClickOutcome::NextPrompt => chat.jump_prompt(1),
                            ClickOutcome::SuggestionAccepted => {}
                            ClickOutcome::Ignored => {}
                        }
                    }
                }
            }
        }
        input_helpers::finish_mouse_event(chat, screen, terminal, data);
        return InputResult::Continue;
    }
    // gh #9 / pi 1.0.0's `app.message.copy` (Ctrl+X): a selection
    // copies itself, else Chat names the target - the sign-in URL on a
    // waiting login screen, the last assistant message otherwise. The
    // key still reaches `handle_key` below, so the Ctrl+X Ctrl+E chord
    // keeps arming on the same press (FR-UI-15).
    if chat.message_copy_key(data) {
        let selection = screen.selected_text();
        // Gh #200: on the OAuth waiting screen the target is the
        // sign-in URL itself, and a verified native copy says so.
        let from_wait = selection.trim().is_empty() && chat.world.login_waiting.is_some();
        // Gh #398 gap 1: the MCP sign-in screen shows the other wrapped
        // authorization URL, and it gets the same in-place confirmation.
        let from_mcp = selection.trim().is_empty()
            && chat.world.login_waiting.is_none()
            && chat.mcp_sign_in_url().is_some();
        let target = if selection.trim().is_empty() {
            chat.message_copy_text()
        } else {
            Some(selection)
        };
        if let Some(text) = target {
            let report = {
                let mut send = |chunk: &str| screen.copy_osc52(terminal, chunk);
                copy_through_ladder(
                    &text,
                    chat.world.options.hooks.copy_to_clipboard.as_ref(),
                    matches!(screen, Screen::Alt(_)).then(|| &mut send as &mut dyn FnMut(&str)),
                )
            };
            // Gh #398 gap 1: on the waiting screen the confirmation also shows
            // **in place**, where the hint was - a reader who pressed the key
            // should not have to hunt for a toast in the corner. The toast stays
            // for the non-URL targets and for the OSC 52 fallback's honesty.
            if from_wait || from_mcp {
                chat.note_url_copied(report != CopyReport::Failed);
            }
            // Gh #374 gap 4: the dwell follows the report, so a success fades
            // sooner than a caveat or a failure. An unverified copy is not a
            // success - it takes the error window because its caveat has to be
            // read.
            let notice = match report {
                CopyReport::Native if from_wait || from_mcp => {
                    "\u{2713} Copied sign-in URL to clipboard".to_string()
                }
                ref report => report.notice(),
            };
            if report.is_error() {
                chat.toast_error(notice);
            } else {
                chat.toast_success(notice);
            }
        }
    }
    // Gh #35 + gh #375 gap 4: the fullscreen viewport's own scroll keys.
    // `Ctrl+End` returns it to the live bottom and `Ctrl+Home` to the transcript's
    // top; both leave the caret alone, because plain `Home`/`End` now belong to the
    // editor unconditionally (pi's split). Gh #375 gap 5 adds paging. What each
    // target means lives in `scroll_target`, so the arithmetic has one home.
    if let Some(target) = chat.viewport_scroll(data) {
        let (content, window) = chat.scroll_extent(terminal.columns(), terminal.rows());
        screen.set_scroll(crate::chat_keys::scroll_target(
            target,
            screen.scroll(),
            content,
            window,
        ));
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
                // A restarted reader must feed the stream the loop actually
                // reads. With an adopted terminal that is the early channel;
                // pointing it at `input_tx` instead left the session deaf once
                // the editor exited (found as a 120 s hang in gh #121's e2e).
                let tx =
                    early_receiver.map_or_else(|| input_tx.clone(), |stream| stream.sender.clone());
                let rtx = resize_tx.clone();
                terminal.start(
                    Box::new(move |d| {
                        let _ = tx.send(d);
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
        Action::InteractiveShell(command) => {
            // Gh #407 pattern 3: the same handoff `$EDITOR` uses, because it is
            // the same problem - a child that needs the terminal the interface is
            // holding. Stop, run with inherited stdio, restart, re-render. Doing
            // it a second way here is how the two drift.
            if let Some(run) = chat.world.options.hooks.run_interactive.clone() {
                screen.leave(terminal, true);
                terminal.stop();
                let outcome = run(&command);
                // A restarted reader must feed the stream the loop actually
                // reads (the gh #121 lesson above): the early channel when the
                // terminal was adopted, `input_tx` otherwise.
                let tx =
                    early_receiver.map_or_else(|| input_tx.clone(), |stream| stream.sender.clone());
                let rtx = resize_tx.clone();
                terminal.start(
                    Box::new(move |d| {
                        let _ = tx.send(d);
                    }) as InputHandler,
                    Box::new(move || {
                        let _ = rtx.send(());
                    }) as ResizeHandler,
                );
                screen.enter(terminal);
                chat.finish_interactive_shell(outcome);
            } else {
                chat.say("this host cannot hand the terminal to a command".to_string());
            }
        }
        Action::Suspend => {
            // Gh #403 gap 4: on a platform with no job control the frame is not
            // touched at all - tearing it down to attempt a suspend that cannot
            // happen would be a visible fault for no gain - and the refusal is a
            // notice, not an error, because nothing failed.
            let supported = lca_tui::engine::signals::suspend_supported();
            if let Some(notice) = crate::run_terminal::suspend_refusal(supported) {
                chat.say(notice);
                return InputResult::Continue;
            }
            crate::run_terminal::suspend(
                screen,
                terminal,
                input_tx,
                resize_tx,
                early_receiver,
                supported,
            );
        }
        Action::Exit => return InputResult::Exit,
    }
    InputResult::Continue
}

/// Run the interface until the user exits, returning the process exit code.
///
/// **Infallible, and the signature says so** (gh #575). It used to return
/// `anyhow::Result<i32>` with a doc block promising "an error when the terminal
/// cannot be driven" - a path that never existed: the 371-line body has no `?`
/// and no `Err(`, and every exit is an `Ok`. Two consequences were wrong:
///
/// - `lca-cli` carried an `Err(err) => INTERNAL` arm for it that could not fire,
///   which is how a "is the terminal broken?" branch survived unreviewed;
/// - `lca-srdd`'s rule is that "a library never returns a boxed opaque error",
///   and `lca-ui` is a library - so this was the crate's only `anyhow` use.
///
/// A terminal that cannot enter raw mode is **not** a failure of this function:
/// it is a *degraded* terminal, and `run_terminal_cooked` reports it as a
/// startup notice while the interface keeps working. Nothing else in the loop
/// can fail, so `i32` is the honest type.
///
/// The `tui::run` seam in `lca-cli` still returns `Result` because *its own*
/// setup (a session that cannot open, a temp dir that cannot be made) can fail
/// before this is called; the binary is where `anyhow` is allowed.
#[must_use]
pub fn run(mut options: UiOptions, runner: TurnRunner) -> i32 {
    // gh #66: the user's key overrides replace defaults (an empty list
    // disables); anything the registry never defined, any conflict, or a
    // load failure is reported loud on the startup notice - a typo keeps
    // defaults, never silently nothing.
    let keybindings = Arc::new(KeybindingsManager::with_user_bindings(std::mem::take(
        &mut options.keybinding_overrides,
    )));
    let keybinding_notice = crate::run_keybindings::keybinding_problems(
        options.keybinding_error.clone(),
        &keybindings.unknown_actions(),
        keybindings.invalid_keys(),
        keybindings.conflicts(),
    );
    // One permission-prompt channel for the session, not one per turn
    // (gh #31 review): the host's endpoint consent asks outside a turn
    // too - when `/model` discovers an ungranted endpoint - and the
    // modal can only appear if this receiver is still being drained.
    // Gh #370 flaw 1: the wake channel every producer signals (see `run_wake`).
    let (waker, wakes) = crate::run_wake::Wakes::new();
    let (prompt_tx, prompt_rx) = crate::run_wake::forwarded_channel(4, waker.clone());
    *options
        .prompt_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(prompt_tx.clone());
    // The dialog channel (gh #124): same session-lifetime shape as the
    // prompt above - a tool asking mid-turn and a command asking
    // between turns both reach the same drain.
    let (dialog_tx, dialog_rx) = crate::run_wake::forwarded_channel(4, waker.clone());
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
    // The `!`/`!!` path reaches its waker through `UiState`.
    chat.world.waker = Some(waker.clone());
    // Gh #110: bare `-r` opens the session picker over the fresh
    // session instead of starting on it.
    if chat.world.options.open_resume_picker {
        chat.open_resume_picker();
    }
    // gh #66: a bad keybindings file is loud from the first frame.
    if let Some(notice) = keybinding_notice {
        chat.say(notice);
    }
    if !initial_messages.is_empty() {
        let first = initial_messages.remove(0);
        chat.transcript.push_user(first.clone());
        chat.submitted = Some(first);
        for message in initial_messages {
            chat.queue_submit(message, lca_protocol::SubmitMode::FollowUp);
        }
    }
    // Gh #400 (type-while-loading): the host may have taken the terminal
    // before the expensive startup load, so raw mode and the reader were live
    // while extensions registered. Adopt that terminal and replay what it
    // buffered, in arrival order, through this loop's own handler - one
    // interpretation path, so an early key means exactly what a late one does.
    //
    // Without an early terminal (a test, an embedder), start one here: the old
    // shape, unchanged.
    let (input_tx, input_rx) = std::sync::mpsc::channel::<String>();
    let (resize_tx, resize_rx) = std::sync::mpsc::channel::<()>();
    // Gh #400: adopt the terminal startup claimed before the load, replaying
    // what it buffered - or start one here when nothing was claimed (a test, an
    // embedder). See `run_early_input::AdoptedTerminal::take`.
    let adopted = crate::run_early_input::AdoptedTerminal::take(
        chat.world.options.early_input.take(),
        &input_tx,
        &resize_tx,
    );
    let startup_keys = adopted.startup_keys;
    let early_input = adopted.receiver;
    let mut terminal = adopted.terminal;
    let mut screen = if chat.screen_mode {
        let mut alt = AltScreenRenderer::new();
        alt.enter(&mut terminal);
        Screen::Alt(alt)
    } else {
        Screen::Main(MainScreenRenderer::new())
    };
    crate::run_selection::apply_screen_tuning(&mut screen, &chat.display_tuning());
    terminal.set_title(&format!(
        "lca — {}",
        chat.world.options.workspace.to_string_lossy()
    ));
    chat.world.resize(terminal.columns(), terminal.rows());
    // R10 + gh #379 gap 1 + gh #324 gap 2: ask for the cell size, then the
    // background and the colour-scheme preference, then close with a `DA1`
    // sentinel. Terminals answer `DA1` whether or not they understand `OSC 11` and
    // answer in order, so its arrival proves the replies before it are answered or
    // absent - which is what lets the theme stop listening instead of waiting out a
    // timeout on a terminal that ignores colours. The sentinel is **last** by
    // construction, and `startup_query` composes the cell query in front of it so a
    // call site cannot get that order backwards.
    terminal.write(&lca_tui::engine::colors::startup_query());
    // Gh #592: say it once, at startup, if this terminal refused raw mode.
    //
    // The check runs **after** `start` has been called (the adopted terminal
    // started before the load), so the reason is final by here. It is a notice
    // and not an `Err`: every keybinding below still works as a *binding*, but
    // the keys themselves arrive through the shell's line discipline, so the
    // honest thing is to tell the user which half is degraded rather than to
    // refuse a terminal that can still show a frame.
    if let Some(notice) = crate::run_terminal_cooked::cooked_input_notice(&terminal) {
        chat.say(notice);
    }

    let mut turns = TurnState::new(prompt_tx, prompt_rx, dialog_rx, waker.clone());
    // Gh #400: keystrokes that arrived during the load are the first thing the
    // editor sees, in the order they were typed.
    for data in &startup_keys {
        chat.handle_key(data);
    }
    // Gh #370 flaw 3: two tiers, one render decision. `dirty` is gone as a bare flag and
    // lives on the clock, which decides *when* a frame is allowed - it is still the only
    // thing that decides *whether* one is needed, so there is no second paint path.
    let mut frames = crate::run_wake::FrameClock::new();
    // The very first frame is not paced and not part of either tier: the loop has painted
    // nothing, so the terminal (and an adopted early terminal, which may already have had
    // keys replayed into it) needs a first full paint before any input arrives.
    let mut first_frame = true;
    // Gh #403 gap 1: the transcript generation this loop has already
    // accounted for. The seam below resets the selection whenever the
    // *whole transcript was replaced*, which is the one state change that
    // makes a selection address text that no longer exists. Appends (a
    // streaming delta), resizes, and marker repaints leave the earlier rows
    // exactly where they were, so the user's selection over them is still
    // valid and survives - that direction is pinned by tests too, because a
    // reset that fires too often is the mirror-image bug.
    let mut seen_generation = chat.transcript.generation();

    let result = 'main: loop {
        // Gh #403 gap 4: a terminal that is gone ends the loop with the documented
        // code. Checked every iteration because the `SIGHUP` handler can only set an
        // atomic and the tick is the floor for sources with no producer - one tick of
        // latency at worst, never a hang.
        if crate::run_terminal::hangup_requested(&terminal) {
            break 'main lca_tui::engine::signals::EXIT_CODE;
        }
        // Turn lifecycle (R16: `TurnState` owns the running turn).
        if turns.reap(&mut chat) {
            frames.request_stream();
        }
        if turns.start(&mut chat, &runner) {
            frames.request_stream();
        }
        if turns.drain(&mut chat) {
            frames.request_stream();
        }
        if tick_permission(&mut chat) {
            frames.request_stream();
        }
        // R4: a background login/identity step reports back here, so a slow
        // OAuth callback never blocks the loop.
        if poll_login(&mut chat) {
            frames.request_stream();
        }
        // ...and the endpoint consent's second step: rows the host
        // discovered once the grant landed open the picker here (gh #31).
        if poll_pending_models(&mut chat) {
            frames.request_stream();
        }
        // ...and a background catalog discovery: `/model` opened
        // instantly and the rows fill the loader here (gh #232).
        if chat.poll_model_refresh() {
            frames.request_stream();
        }
        // ...and so does a background `/compact`: the summarization call
        // runs on its own thread and the interface keeps painting.
        if chat.poll_compact() {
            frames.request_stream();
        }
        // ...and a background `/mcp login`: the browser flow outlives
        // the command that started it.
        if crate::chat_commands::poll_mcp(&mut chat) {
            frames.request_stream();
        }
        // ...and the `/extensions` hub: catalog rows land through
        // the loader, install results post as notices (#227).
        if let Some(hub) = chat.extension_hub.as_mut() {
            if hub.poll_hub() {
                frames.request_stream();
            }
            if hub.tick() {
                frames.request_stream();
            }
        }
        if chat.poll_install() {
            frames.request_stream();
        }

        // Edge auto-scroll while a selection drag sits on a viewport edge
        // (R6).
        if screen.tick_auto_scroll() {
            frames.request_stream();
        }

        // The screen mode can change at runtime (FR-UI-21).
        if screen.is_fullscreen() != chat.screen_mode {
            let was_fullscreen = screen.is_fullscreen();
            switch_screen(&mut screen, chat.screen_mode, &mut terminal);
            crate::run_selection::apply_screen_tuning(&mut screen, &chat.display_tuning());
            // Gh #82: leaving fullscreen can name the resume command.
            if was_fullscreen && !chat.screen_mode {
                let tuning = chat.display_tuning();
                let id = chat
                    .world
                    .options
                    .hooks
                    .current_session_id
                    .as_ref()
                    .map(|session| session())
                    .unwrap_or_default();
                if let Some(hint) = exit_hint(&tuning, &id) {
                    chat.say(hint);
                }
            }
            frames.request_stream();
        }

        // Gh #403 gap 1: one seam for every path that replaces the
        // transcript - a `/resume` switch, a `/tree` branch jump, a fork, a
        // clone, and `/import` all land on `replay_records`, which is the
        // only place that clears the transcript and therefore the only place
        // that bumps the generation. Putting the reset here rather than at
        // those four call sites is deliberate: four call sites is how the
        // fifth gets missed.
        let generation =
            crate::run_selection::reset_selection_if_replaced(&mut screen, &chat, seen_generation);
        if generation != seen_generation {
            seen_generation = generation;
            frames.request_stream();
        }

        // The separator's spinner animates while a turn runs (R2): one
        // frame per cadence, never while idle.
        if chat.tick() {
            frames.request_stream();
        }

        // Gh #213/#369: the tab badge and the taskbar spinner follow the
        // lifecycle (turn, compaction, shell), on change only.
        turns.sync_program_status(&chat, &mut terminal);
        // Gh #369: the OSC 9;4 keepalive rides this loop's 50 ms tick rather
        // than a new thread; it writes only while the indicator is armed.
        terminal.tick_progress();

        // Gh #370 flaw 3: the clock decides **whether and when**. Input (tier 2) always
        // releases; a stream delta (tier 1) releases at most once per
        // `MIN_RENDER_INTERVAL`, and a delta that arrives inside that window coalesces
        // into the frame already queued rather than being dropped.
        let now = std::time::Instant::now();
        if first_frame || frames.render(now) {
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
            // Gh #374 gap 1: the alt screen also receives the **document** -
            // everything above the `height`-row window - so its exit hands back the
            // whole session, not the visible slice. It rides out of the `viewport`
            // call above rather than being recomposed; the scrollback renderer
            // ignores it (its append contract already wrote every row).
            let document = chat.take_document();
            screen.render(&mut terminal, viewport, document, width, height);
            // gh #209: mirror the live scroll for per-tab restores.
            chat.last_scroll = screen.scroll();
            frames.mark_rendered(now);
            first_frame = false;
        }

        // Input (gh #400): when startup adopted a terminal that was already
        // reading, its receiver *is* the input channel - the same reader thread
        // keeps filling it, so there is one stream and no handover gap.
        // Gh #370 flaw 1: block on every source, not stdin alone. Input is taken
        // first, non-blocking, so a keystroke is never queued behind a producer; only
        // when there is none does the loop sleep - at most one tick - on the wake.
        //
        // Gh #370 flaw 2: take **every** event that has already arrived before painting
        // once. A fullscreen drag emits dozens of SGR `Move` sequences per second, and
        // popping one per iteration made every cell of the drag cost a complete repaint.
        // The drain is bounded (see `input_helpers::drain_input`) and every drained event
        // is still handled in order - keystrokes are never coalesced away, and a drag's
        // intermediate positions are observable to the gesture, so they are handled too.
        let mut input = match early_input.as_ref() {
            Some(early) => input_helpers::drain_input(&mut || early.try_recv()),
            None => input_helpers::drain_input(&mut || input_rx.try_recv().ok()),
        };
        let woke = if input.is_empty() {
            // Gh #370 flaw 3: a queued tier-1 frame waits at most its remaining budget, so
            // a stream holds ~60 FPS instead of dropping to the tick's 20. With nothing
            // requested this is the plain tick - never shorter, because waking to check a
            // deadline that cannot exist is a spin (the 200 ms guard).
            wakes.wait_for(frames.next_wait(std::time::Instant::now()))
        } else {
            crate::run_wake::Woke::Input
        };
        if input.is_empty() {
            // Nothing typed: a producer woke us, or the tick elapsed (gh #210's
            // hover tooltip clock runs on both).
            if woke != crate::run_wake::Woke::Input {
                let (width, height) = chat.world.size;
                if chat.poll_tooltip(width, height, screen.scroll(), std::time::Instant::now()) {
                    frames.request_stream();
                }
            }
        } else {
            // Every drained event is handled, in arrival order, before one paint. `Exit`
            // stops at the event that asked for it: the events behind it were typed after
            // the quit, and replaying them into a loop that is ending would be wrong.
            let mut exit = false;
            for data in input.drain(..) {
                if let InputResult::Exit = handle_input(
                    &data,
                    &mut chat,
                    &mut screen,
                    &mut terminal,
                    &input_tx,
                    &resize_tx,
                    early_input.as_ref(),
                    &turns.cancel,
                    &mut turns.aborted,
                ) {
                    exit = true;
                    break;
                }
            }
            if exit {
                break 'main 0;
            }
            // Tier 2: input preempts the pace. It is never paced, because the throttle
            // exists to bound a stream nobody typed into.
            frames.request_immediate();
        }

        // Resize.
        if resize_rx.try_recv().is_ok() {
            chat.world.resize(terminal.columns(), terminal.rows());
            // A resize is tier 2: the next pass paints at the new geometry rather than up
            // to an interval later at the old one.
            frames.request_immediate();
        }
    };

    turns.shutdown();
    // Gh #403 gap 4: **dispose before touching the terminal.** The writes below are
    // gated at `raw_write`, so this is about not *trying*: a dead fd must not be left
    // to `drain_input` (which would spin on `POLLHUP` for its full window) and the
    // exit dump has nowhere to go. `terminal.stop` still runs - it joins the reader
    // thread - and skips its own writes on a hung-up terminal.
    if !crate::run_terminal::hangup_requested(&terminal) {
        screen.leave(&mut terminal, false);
        terminal.drain_input(1000, 50);
    }
    terminal.stop();
    result
}

// Gh #575: the turn state and its transitions live in `run_turns.rs` (gh #592's
// notice pushed this file past the ceiling); re-exported so every path a row or
// sibling names stays `crate::run::TurnState`.
pub(crate) use crate::run_turns::TurnState;
// The rows in `run_tests.rs` build a permission modal through `use super::*`.
#[cfg(test)]
pub(crate) use crate::state::PermissionModal;

#[path = "run_input_helpers.rs"]
pub(crate) mod input_helpers;

#[cfg(test)]
#[path = "run_batch_tests.rs"]
mod batch_tests;

#[cfg(test)]
#[path = "run_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "run_viewport_keys_tests.rs"]
mod viewport_keys_tests;

#[cfg(test)]
#[path = "run_theme_query_tests.rs"]
mod theme_query_tests;

/// Gh #590's count rows (one failed turn, one error line), split out of
/// `run_tests.rs` for gate 11's ceiling.
#[cfg(test)]
#[path = "run_590_tests.rs"]
mod turn_error_paint;

// Gh #575: `run` is infallible - the signature is the assertion.
#[cfg(test)]
#[path = "run_575_tests.rs"]
mod run_575_tests;
