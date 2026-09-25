//! The terminal interface: scrollback, streaming active area, status line,
//! input editor with history and completion, and the permission modal.
//!
//! Rendering is a pure function of [`UiState`] over ratatui's backend, so
//! tests assert against a virtual buffer. Extensions never reach the
//! terminal; their widgets join in Phase 6 through the declarative
//! vocabulary (ADR-0003).

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender};

use lca_protocol::CommandEffect;
use lca_protocol::Usage;
use lca_protocol::{StopReason, TurnEvent, TurnOutcome, TurnStatus};
use ratatui::Frame;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

/// Color policy (FR-UI-5): plain never paints a foreground color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Terminal colors allowed.
    Themed,
    /// Plain text only.
    Plain,
}

impl ColorMode {
    /// Map the configuration value onto the render policy.
    pub fn from_config(mode: lca_config::ColorMode) -> ColorMode {
        match mode {
            lca_config::ColorMode::Auto => ColorMode::Themed,
            lca_config::ColorMode::Never => ColorMode::Plain,
        }
    }
}

/// Input editor mode; multiline is carried in the buffer itself
/// (Shift+Enter), so mode is reserved for future modal input states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Normal editing.
    Normal,
}

/// A one-line description of the running turn (NFR-28: state also has a
/// text cue, never color alone).
#[derive(Debug, Clone)]
pub struct TurnStatusLine {
    /// The text shown on the status line.
    pub text: String,
}

/// How the input editor invokes a registered slash command.
pub type CommandInvoker = Arc<dyn Fn(&str, &str) -> CommandEffect + Send + Sync>;

/// Which extensions draw in a region: the CLI's view over the registry
/// (ADR-0003's pull model - the host asks, the extension answers).
pub type RegionRenderer =
    Arc<dyn Fn(&str) -> Vec<(String, lca_protocol::WidgetTree)> + Send + Sync>;

/// Deliver one user interaction to the extensions registered for that
/// region; the first `Some` answers and the host applies its effect.
/// Effects only ever originate here - from real user input - which is
/// what makes FR-UI-6 ("no modal without the user") enforceable.
pub type RegionInteractor = Arc<
    dyn Fn(&str, &lca_protocol::UiInput) -> Option<(String, lca_protocol::UiEffect)> + Send + Sync,
>;

/// Escape sequences and other control characters become visible text
/// before anything is drawn: spans carry data, never control codes
/// (FR-UI-2, ADR-0003). Tabs and newlines collapse to spaces because
/// the host owns layout.
pub fn sanitize_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\n' | '\t' | '\r' => out.push(' '),
            c if (c as u32) < 0x20 || c == '\x7f' => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// One node's lines, arena-style: node0 is the root and children are
/// indices. Every text node passes through [`sanitize_text`] - the one
/// choke point for FR-UI-2.
///
/// A node is rendered at most once per call. The arena is supplied by an
/// untrusted extension, so a child index that points at an ancestor (or
/// at itself) must not recurse forever or expand exponentially; the
/// `visited` set is the guard the widget-shaped sibling attack needs.
pub fn widget_lines(nodes: &[lca_protocol::Widget]) -> Vec<String> {
    fn walk(
        nodes: &[lca_protocol::Widget],
        index: usize,
        out: &mut Vec<String>,
        visited: &mut [bool],
    ) {
        use lca_protocol::Widget;
        let Some(node) = nodes.get(index) else { return };
        if visited.get(index) == Some(&true) {
            return;
        }
        if let Some(seen) = visited.get_mut(index) {
            *seen = true;
        }
        match node {
            Widget::Text { content, .. } => out.push(sanitize_text(content)),
            Widget::Image { media_type, bytes } => {
                out.push(format!("[image {media_type}, {} bytes]", bytes.len()))
            }
            Widget::Boxed { title, child } => {
                if let Some(title) = title {
                    out.push(format!("[{title}]"));
                }
                walk(nodes, *child as usize, out, visited);
            }
            Widget::Row(children) => {
                // Side by side, first line of each (v1 layout; ponytail:
                // a real row shaper when an extension needs wrapping).
                let parts: Vec<String> = children
                    .iter()
                    .filter_map(|child| {
                        let mut lines = Vec::new();
                        walk(nodes, *child as usize, &mut lines, visited);
                        lines.into_iter().next()
                    })
                    .collect();
                out.push(parts.join(" | "));
            }
            Widget::Column(children) => {
                for child in children {
                    walk(nodes, *child as usize, out, visited);
                }
            }
            Widget::Spinner { frames } => {
                let ticks = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.subsec_millis())
                    .unwrap_or(0);
                let count = chars_count(frames);
                if count > 0 {
                    let pick = (ticks / 80) as usize % count;
                    out.push(frames.chars().nth(pick).unwrap_or(' ').to_string());
                } else {
                    out.push(" ".to_string());
                }
            }
            Widget::Progress { label, fill } => {
                let fill = (*fill).clamp(0.0, 1.0);
                let width = 20;
                let done = (fill * width as f32).round() as usize;
                out.push(format!(
                    "{label} [{}{}] {:>3}%",
                    "#".repeat(done),
                    "-".repeat(width - done),
                    (fill * 100.0).round() as u32
                ));
            }
            Widget::KeyValue(pairs) => {
                for (key, value) in pairs {
                    out.push(format!("{key}: {}", sanitize_text(value)));
                }
            }
            Widget::Vendor(kind) => out.push(format!("[vendor {kind}]")),
        }
    }
    fn chars_count(text: &str) -> usize {
        text.chars().count()
    }
    let mut out = Vec::new();
    if !nodes.is_empty() {
        let mut visited = vec![false; nodes.len()];
        walk(nodes, 0, &mut out, &mut visited);
    }
    out
}

/// Static inputs for the interface.
pub struct UiOptions {
    /// `provider/model` for the status line - a cell, because the
    /// status line shows the session's model and `/model` rewrites it.
    pub model_label: std::sync::Arc<std::sync::Mutex<String>>,
    /// Conversation lines already resolved for display (resume).
    pub initial_lines: Vec<String>,
    /// Plain-text rendering (FR-UI-5).
    pub plain: bool,
    /// Invoke a registered slash command (the registry supplies the
    /// table; `/stats` fills its built-in slot through an extension,
    /// ADR-0019). Arguments are the bare typed name and its argument
    /// text.
    pub invoke_command: CommandInvoker,
    /// Slash commands offered by completion.
    pub slash_commands: Vec<String>,
    /// Workspace root for path completion.
    pub workspace: PathBuf,
    /// Extension trees per region (`None`: no ui-capable extension is
    /// registered, which is the default).
    pub render_regions: Option<RegionRenderer>,
    /// User interactions routed to extensions (FR-UI-6's only source).
    pub ui_events: Option<RegionInteractor>,
    /// The background update check's finding, set once a check finds a
    /// newer release (FR-CFG-6; the status line reads it every frame).
    pub update_notice: Option<std::sync::Arc<std::sync::OnceLock<String>>>,
}

/// The permission modal: what is being asked, and how to answer.
pub struct PermissionModal {
    /// The exact command or path (FR-UI-4).
    pub action: String,
    /// The worker waiting for the decision, when live.
    pub respond: Option<SyncSender<lca_permissions::Decision>>,
}

/// Everything the renderer draws.
pub struct UiState {
    /// Static inputs.
    pub options: UiOptions,
    /// The input buffer.
    pub buffer: String,
    /// Where the cursor sits, in bytes. `None` means the end of the buffer,
    /// which is the default so text assigned directly still appends; a
    /// movement key pins it to an explicit index.
    pub cursor: Option<usize>,
    /// Submitted lines, newest last (input editor history).
    pub history: Vec<String>,
    /// History browsing position.
    pub history_index: Option<usize>,
    /// Conversation scrollback, one entry per display line.
    pub scrollback: Vec<String>,
    /// The streaming area.
    pub active: String,
    /// A transient notice (command results, unknown commands).
    pub notice: Option<String>,
    /// A running tool line, when one is executing.
    pub tool_line: Option<String>,
    /// A turn is in flight.
    pub turn_running: bool,
    /// Status-line turn state.
    pub turn_status: Option<TurnStatusLine>,
    /// Accumulated usage for the session's visible totals (FR-CORE-8).
    pub usage: Usage,
    /// Editor mode.
    pub mode: InputMode,
    /// The open permission modal, if any.
    pub permission: Option<PermissionModal>,
    /// Ctrl+C seen once on an idle prompt.
    pub ctrl_c_armed: bool,
    /// The extension side panel is open.
    pub panel_open: bool,
    /// An extension modal is open (one at a time, user-dismissible:
    /// capability catalog `ui`).
    pub modal_open: bool,
    /// Terminal size, tracked across resizes (FR-UI-3).
    pub size: (u16, u16),
}

impl UiState {
    /// Build the initial state.
    pub fn new(options: UiOptions) -> UiState {
        let workspace = if options.workspace.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            options.workspace.clone()
        };
        let mut options = options;
        options.workspace = workspace;
        let initial_lines = options.initial_lines.clone();
        UiState {
            options,
            scrollback: initial_lines,
            buffer: String::new(),
            cursor: None,
            history: Vec::new(),
            history_index: None,
            active: String::new(),
            notice: None,
            tool_line: None,
            turn_running: false,
            turn_status: None,
            usage: Usage::default(),
            mode: InputMode::Normal,
            permission: None,
            ctrl_c_armed: false,
            panel_open: false,
            modal_open: false,
            size: (80, 24),
        }
    }

    /// Record a terminal resize (FR-UI-3: nothing is lost).
    pub fn resize(&mut self, width: u16, height: u16) {
        self.size = (width, height);
    }

    /// The cursor's byte offset into `buffer`, clamped to a char boundary.
    pub fn cursor_index(&self) -> usize {
        let end = self.buffer.len();
        let Some(mut index) = self.cursor else {
            return end;
        };
        index = index.min(end);
        while index > 0 && !self.buffer.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    fn set_cursor(&mut self, index: usize) {
        self.cursor = (index < self.buffer.len()).then_some(index);
    }

    /// Insert `text` at the cursor and leave the cursor after it.
    pub fn insert_at_cursor(&mut self, text: &str) {
        let at = self.cursor_index();
        self.buffer.insert_str(at, text);
        self.set_cursor(at + text.len());
    }

    fn backspace(&mut self) {
        let at = self.cursor_index();
        if at == 0 {
            return;
        }
        let prev = prev_char_boundary(&self.buffer, at);
        self.buffer.replace_range(prev..at, "");
        self.set_cursor(prev);
    }

    fn delete_at_cursor(&mut self) {
        let at = self.cursor_index();
        if at >= self.buffer.len() {
            return;
        }
        let next = next_char_boundary(&self.buffer, at);
        self.buffer.replace_range(at..next, "");
        self.set_cursor(at);
    }

    fn move_left(&mut self) {
        let at = self.cursor_index();
        if at > 0 {
            self.set_cursor(prev_char_boundary(&self.buffer, at));
        }
    }

    fn move_right(&mut self) {
        let at = self.cursor_index();
        if at >= self.buffer.len() {
            self.cursor = None;
        } else {
            self.set_cursor(next_char_boundary(&self.buffer, at));
        }
    }

    fn move_home(&mut self) {
        let at = self.cursor_index();
        let start = self.buffer[..at].rfind('\n').map_or(0, |index| index + 1);
        self.set_cursor(start);
    }

    fn move_end(&mut self) {
        let at = self.cursor_index();
        let end = self.buffer[at..]
            .find('\n')
            .map_or(self.buffer.len(), |index| at + index);
        self.set_cursor(end);
    }

    /// Open the permission modal (FR-UI-4).
    pub fn show_permission(&mut self, action: String) {
        self.permission = Some(PermissionModal {
            action,
            respond: None,
        });
    }

    /// Consume a turn event from the worker.
    pub fn on_turn_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::TextDelta(delta) | TurnEvent::ReasoningDelta(delta) => {
                self.active.push_str(&delta);
            }
            TurnEvent::ToolStarted(call) => {
                self.tool_line = Some(format!("running {}({})...", call.name, call.arguments));
            }
            TurnEvent::ToolFinished(result) => {
                self.tool_line = None;
                self.active.push_str(&format!(
                    "\n> {} -> {}\n",
                    short_call(&result.call_id),
                    match result.status {
                        lca_protocol::ToolResultStatus::Ok => "ok",
                        lca_protocol::ToolResultStatus::Error => "error",
                        lca_protocol::ToolResultStatus::Denied => "denied",
                        lca_protocol::ToolResultStatus::Timeout => "timeout",
                    }
                ));
            }
            TurnEvent::ToolOutputChunk { chunk, .. } => {
                self.active.push_str(&chunk);
            }
            TurnEvent::Usage(usage) => {
                self.usage.input = self.usage.input.saturating_add(usage.input);
                self.usage.output = self.usage.output.saturating_add(usage.output);
                self.usage.cache_read = self.usage.cache_read.saturating_add(usage.cache_read);
                self.usage.cache_write = self.usage.cache_write.saturating_add(usage.cache_write);
                self.usage.cost += usage.cost;
            }
            TurnEvent::RetryScheduled {
                attempt,
                max,
                error,
                ..
            } => {
                self.notice = Some(format!("retry {attempt}/{max} after: {error}"));
            }
            TurnEvent::Error { message, .. } => {
                self.active.push_str(&format!("\n! {message}\n"));
            }
            TurnEvent::AssistantText(_) => {}
            TurnEvent::ExtensionEvent {
                extension,
                event,
                detail,
            } => {
                self.notice = Some(format!("[{extension}] {event}: {detail}"));
            }
            TurnEvent::TurnEnded {
                status,
                stop_reason,
            } => {
                if !self.active.trim().is_empty() {
                    let text = std::mem::take(&mut self.active);
                    self.scrollback.push(text);
                }
                self.tool_line = None;
                self.turn_running = false;
                self.turn_status = Some(TurnStatusLine {
                    text: match (status, stop_reason) {
                        (TurnStatus::Ok, StopReason::Stop) => "done".to_string(),
                        (TurnStatus::Ok, StopReason::Cancelled) => "cancelled".to_string(),
                        (TurnStatus::Error, StopReason::IterationLimit) => {
                            "stopped: iteration limit".to_string()
                        }
                        (TurnStatus::Error, _) => "done with errors".to_string(),
                        (TurnStatus::Ok, _) => "done".to_string(),
                    },
                });
            }
        }
    }
}

fn short_call(call_id: &str) -> String {
    let trimmed = call_id.trim_start_matches("call-");
    format!("call {trimmed}")
}

/// What a key press asks the loop to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing external; keep editing.
    Continue,
    /// Submit the buffer as a user turn.
    Submit,
    /// Cancel the running turn (FR-CORE-5).
    CancelTurn,
    /// Exit the interface.
    Exit,
}

/// Apply one extension effect. `OpenModal` is dropped while a turn
/// runs: an extension cannot interrupt work (FR-UI-6), and this is the
/// single place effects are applied, all of them sourced from real
/// user input.
fn apply_ui_effect(state: &mut UiState, effect: lca_protocol::UiEffect) -> Option<Action> {
    use lca_protocol::UiEffect;
    match effect {
        UiEffect::None => None,
        UiEffect::CloseModal => {
            state.modal_open = false;
            None
        }
        UiEffect::OpenModal => {
            if !state.turn_running {
                state.modal_open = true;
            }
            None
        }
        UiEffect::ShowNotice(text) => {
            state.notice = Some(sanitize_text(&text));
            None
        }
        UiEffect::InsertText(text) => {
            state.buffer.push_str(&sanitize_text(&text));
            None
        }
        UiEffect::SubmitPrompt(text) => {
            state.buffer = text;
            Some(Action::Submit)
        }
    }
}

/// Map a key to the interaction vocabulary the ui world speaks.
fn key_input(key: crossterm::event::KeyEvent) -> lca_protocol::UiInput {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Char(character) => lca_protocol::UiInput::Key {
            key: character.to_string(),
        },
        KeyCode::Enter => lca_protocol::UiInput::Submit {
            text: String::new(),
        },
        KeyCode::Esc => lca_protocol::UiInput::Cancel,
        other => lca_protocol::UiInput::Key {
            key: format!("{other:?}"),
        },
    }
}

/// Handle one key press against the state (NFR-27: keyboard only).
pub fn handle_key(state: &mut UiState, key: crossterm::event::KeyEvent) -> Action {
    use crossterm::event::{KeyCode, KeyModifiers};

    // The modal swallows every key while open.
    if let Some(modal) = state.permission.take() {
        use lca_permissions::Decision;
        let PermissionModal { action, respond } = modal;
        let answer = |decision: Decision| {
            if let Some(respond) = respond {
                let _ = respond.send(decision);
            }
        };
        return match key.code {
            KeyCode::Char('o') => {
                answer(Decision::Once);
                Action::Continue
            }
            KeyCode::Char('a') => {
                answer(Decision::Always);
                Action::Continue
            }
            KeyCode::Char('d') | KeyCode::Esc | KeyCode::Enter => {
                answer(Decision::Denied);
                Action::Continue
            }
            _ => {
                state.permission = Some(PermissionModal {
                    action,
                    respond: None,
                });
                Action::Continue
            }
        };
    }

    use crossterm::event::KeyCode as K;
    // The extension modal runs first: Enter submits what is typed,
    // Escape dismisses (host-level: a modal is user-dismissible), and
    // everything else goes to the extension that opened it.
    if state.modal_open {
        if key.code == K::Esc {
            state.modal_open = false;
            return Action::Continue;
        }
        let input = match key.code {
            K::Enter => {
                let text = std::mem::take(&mut state.buffer);
                lca_protocol::UiInput::Submit { text }
            }
            other => key_input(crossterm::event::KeyEvent::new(other, key.modifiers)),
        };
        if let Some(interactor) = &state.options.ui_events
            && let Some((_, effect)) = interactor("modal", &input)
            && let Some(action) = apply_ui_effect(state, effect)
        {
            return action;
        }
        return Action::Continue;
    }

    // Ctrl+P toggles the side panel (the host's binding, not routed to
    // an extension).
    if key.code == K::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
        state.panel_open = !state.panel_open;
        return Action::Continue;
    }

    // With the panel open and no control chord in flight, keys belong
    // to whatever extension registered for the region: the live session
    // in the reference panel eats them the way a terminal would. An
    // extension that claims nothing closes the panel instead.
    if state.panel_open && !key.modifiers.contains(KeyModifiers::CONTROL) {
        let input = key_input(key);
        if let Some(interactor) = &state.options.ui_events {
            if let Some((_, effect)) = interactor("panel", &input) {
                if let Some(action) = apply_ui_effect(state, effect) {
                    return action;
                }
                return Action::Continue;
            }
            // Nothing claims the panel: close it and let the key edit
            // normally.
            state.panel_open = false;
        }
    }

    match key.code {
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
            state.insert_at_cursor("\n");
            Action::Continue
        }
        KeyCode::Enter => {
            if state.buffer.is_empty() {
                return Action::Continue;
            }
            let submitted = state.buffer.clone();
            if submitted.starts_with('/') {
                let command_line = submitted.strip_prefix('/').unwrap_or(&submitted);
                let mut parts = command_line.splitn(2, ' ');
                let name = parts.next().unwrap_or("").to_string();
                let argument = parts.next().unwrap_or("").to_string();
                state.history.push(submitted);
                state.history_index = None;
                state.buffer.clear();
                state.cursor = None;
                // Host-level commands the interface answers itself, so they
                // work with no extension installed.
                match name.as_str() {
                    "help" => {
                        state.notice = Some(help_notice(&state.options.slash_commands));
                        return Action::Continue;
                    }
                    "quit" | "exit" => return Action::Exit,
                    _ => {}
                }
                let full = format!("/{name}");
                if state
                    .options
                    .slash_commands
                    .iter()
                    .any(|command| command == &full)
                {
                    match (state.options.invoke_command)(&name, &argument) {
                        CommandEffect::ShowWidget(text) => {
                            state.notice = Some(sanitize_text(&text))
                        }
                        CommandEffect::InsertText(text) => {
                            state.insert_at_cursor(&sanitize_text(&text))
                        }
                        CommandEffect::SubmitPrompt(text) => {
                            state.buffer = text;
                            state.cursor = None;
                            return Action::Submit;
                        }
                        CommandEffect::None => {}
                    }
                    return Action::Continue;
                }
                state.notice = Some(sanitize_text(&format!("unknown command /{name}")));
                return Action::Continue;
            }
            // A turn needs a model; say so rather than letting it fail deep
            // inside the provider with a transport error.
            if state
                .options
                .model_label
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .trim()
                .is_empty()
            {
                state.buffer.clear();
                state.cursor = None;
                state.notice = Some(
                    "No model is active. Use /login to sign in, or /model to choose one."
                        .to_string(),
                );
                return Action::Continue;
            }
            let submitted = std::mem::take(&mut state.buffer);
            state.cursor = None;
            state.history.push(submitted);
            state.history_index = None;
            Action::Submit
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.turn_running {
                Action::CancelTurn
            } else if state.ctrl_c_armed {
                state.ctrl_c_armed = false;
                Action::Exit
            } else {
                state.ctrl_c_armed = true;
                Action::Continue
            }
        }
        KeyCode::Up => {
            if !state.history.is_empty() {
                let index = match state.history_index {
                    None => state.history.len() - 1,
                    Some(0) => 0,
                    Some(index) => index - 1,
                };
                state.history_index = Some(index);
                state.buffer = state.history[index].clone();
                state.cursor = None;
            }
            Action::Continue
        }
        KeyCode::Down => {
            match state.history_index.take() {
                Some(index) if index + 1 < state.history.len() => {
                    state.history_index = Some(index + 1);
                    state.buffer = state.history[index + 1].clone();
                }
                Some(_) => state.buffer.clear(),
                None => {}
            }
            state.cursor = None;
            Action::Continue
        }
        KeyCode::Tab => {
            complete(state);
            Action::Continue
        }
        KeyCode::Backspace => {
            state.backspace();
            Action::Continue
        }
        KeyCode::Delete => {
            state.delete_at_cursor();
            Action::Continue
        }
        KeyCode::Left => {
            state.move_left();
            Action::Continue
        }
        KeyCode::Right => {
            state.move_right();
            Action::Continue
        }
        KeyCode::Home => {
            state.move_home();
            Action::Continue
        }
        KeyCode::End => {
            state.move_end();
            Action::Continue
        }
        KeyCode::Char(c) => {
            state.insert_at_cursor(&c.to_string());
            state.ctrl_c_armed = false;
            Action::Continue
        }
        _ => Action::Continue,
    }
}

/// The byte offset of the previous character boundary before `index`.
fn prev_char_boundary(text: &str, index: usize) -> usize {
    let mut previous = index.saturating_sub(1);
    while previous > 0 && !text.is_char_boundary(previous) {
        previous -= 1;
    }
    previous
}

/// The byte offset of the next character boundary after `index`.
fn next_char_boundary(text: &str, index: usize) -> usize {
    let mut next = index + 1;
    while next < text.len() && !text.is_char_boundary(next) {
        next += 1;
    }
    next
}

/// The `/help` text: the commands the interface offers, then the keys.
fn help_notice(commands: &[String]) -> String {
    let mut sorted: Vec<&String> = commands.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut out = String::from("commands:\n");
    for command in sorted {
        out.push_str("  ");
        out.push_str(command);
        out.push('\n');
    }
    out.push_str("Enter sends, Shift+Enter adds a line, Tab completes, Ctrl+C cancels");
    out
}

fn complete(state: &mut UiState) {
    if state.buffer.starts_with('/') && !state.buffer.contains(char::is_whitespace) {
        let prefix = state.buffer.clone();
        let mut matches: Vec<String> = state
            .options
            .slash_commands
            .iter()
            .filter(|command| command.starts_with(&prefix))
            .cloned()
            .collect();
        matches.sort();
        match matches.len() {
            0 => {}
            1 => {
                state.buffer = matches[0].clone();
                state.cursor = None;
            }
            _ => {
                if let Some(common) = common_prefix(&matches)
                    && common.len() > prefix.len()
                {
                    state.buffer = common;
                    state.cursor = None;
                }
                state.notice = Some(format!("completions: {}", matches.join("  ")));
            }
        }
        return;
    }

    // Path completion: the last whitespace-delimited token.
    let (head, partial) = match state.buffer.rfind(char::is_whitespace) {
        Some(index) => state.buffer.split_at(index + 1),
        None => ("", state.buffer.as_str()),
    };
    if partial.is_empty() {
        return;
    }
    let (dir_part, file_prefix) = match partial.rfind('/') {
        Some(index) => (&partial[..=index], &partial[index + 1..]),
        None => ("", partial),
    };
    let base = state
        .options
        .workspace
        .join(dir_part.trim_end_matches('/').trim_start_matches("./"));
    let base = if dir_part.is_empty() {
        state.options.workspace.clone()
    } else {
        base
    };
    let Ok(entries) = std::fs::read_dir(&base) else {
        return;
    };
    let mut matches: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            !name.starts_with('.') && name.starts_with(file_prefix)
        })
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            format!(
                "{dir_part}{name}{}",
                if is_dir && dir_part.is_empty() {
                    "/".to_string()
                } else {
                    String::new()
                }
            )
        })
        .collect();
    matches.sort();
    if matches.len() == 1 {
        state.buffer = format!("{head}{}", matches[0]);
        state.cursor = None;
    } else if let Some(common) = common_prefix(&matches)
        && common.len() > dir_part.len() + file_prefix.len()
    {
        state.buffer = format!("{head}{common}");
        state.cursor = None;
    } else if matches.len() > 1 {
        state.notice = Some(format!("completions: {}", matches.join("  ")));
    }
}

fn common_prefix(items: &[String]) -> Option<String> {
    let first = items.first()?;
    let mut prefix = first.as_str();
    for item in items {
        let mut end = 0;
        for (a, b) in prefix.bytes().zip(item.bytes()) {
            if a != b {
                break;
            }
            end += 1;
        }
        prefix = &prefix[..end];
        if prefix.is_empty() {
            return None;
        }
    }
    Some(prefix.to_string())
}

/// Channels between the UI loop and the worker thread running a turn.
pub struct TurnChannels {
    /// Turn events flow to the UI.
    pub events: SyncSender<TurnEvent>,
    /// Permission requests flow to the UI.
    pub prompt: SyncSender<PromptRequest>,
}

/// A worker asking for permission (FR-UI-4: `action` is the exact command
/// or path).
pub struct PromptRequest {
    /// The exact action display.
    pub action: String,
    /// Where the decision goes.
    pub respond: SyncSender<lca_permissions::Decision>,
}

/// Starts one turn on a worker thread and hands back its join handle; the
/// binary supplies this so the crate stays free of provider wiring. The
/// runner is callable once per submission.
pub type TurnRunner = Box<
    dyn Fn(String, TurnChannels, lca_tools::CancelFlag) -> std::thread::JoinHandle<TurnOutcome>
        + Send
        + Sync,
>;

/// Render one frame into a terminal (tests use `TestBackend`).
pub fn render<B: Backend>(
    terminal: &mut ratatui::Terminal<B>,
    state: &UiState,
) -> std::io::Result<()> {
    terminal.draw(|frame| draw_frame(frame, state))?;
    Ok(())
}

/// Draw one frame. Pure with respect to the state (NFR-28: every colored
/// state also carries a text cue).
fn draw_frame(frame: &mut Frame, state: &UiState) {
    let plain = state.options.plain;
    let theme = |plain: bool, color: Color| {
        if plain {
            Style::default()
        } else {
            Style::default().fg(color)
        }
    };

    // Extension content this frame: pulled from the registry's view
    // (ADR-0003's model - the host asks, never the other way round).
    let footer_trees = state
        .options
        .render_regions
        .as_ref()
        .map(|render| render("footer"))
        .unwrap_or_default();
    let footer_lines: Vec<String> = footer_trees
        .iter()
        .flat_map(|(_, tree)| widget_lines(&tree.nodes))
        .take(3) // the catalog's footer constraint
        .collect();

    // The side panel, when open, takes a right-hand column (the
    // catalog's `panel` region).
    let (main_area, panel_area) = if state.panel_open {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(30), Constraint::Length(42)])
            .split(frame.area());
        (split[0], Some(split[1]))
    } else {
        (frame.area(), None)
    };

    let input_rows = wrapped_rows(
        &format!("> {}", state.buffer),
        main_area.width.saturating_sub(2).max(1) as usize,
    );
    let input_height = (input_rows + 2).clamp(3, 12);
    let mut vertical = vec![Constraint::Min(3), Constraint::Min(3)];
    if !footer_lines.is_empty() {
        // The footer draws with a border: two rows of chrome around its
        // lines (the catalog's "up to three lines" plus the frame).
        vertical.push(Constraint::Length(footer_lines.len() as u16 + 2));
    }
    vertical.push(Constraint::Length(1));
    vertical.push(Constraint::Length(input_height));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vertical)
        .split(main_area);
    let footer_row = (!footer_lines.is_empty()).then_some(chunks[2]);
    let status_row = chunks[chunks.len() - 2];
    let input_row = chunks[chunks.len() - 1];

    let scroll_text = if state.scrollback.is_empty() {
        // First run: say what the interface is for instead of a blank box.
        "Type a message to start. /help lists commands, /login signs in.".to_string()
    } else {
        state.scrollback.join("\n")
    };
    let scroll_lines = scroll_text.lines().count() as u16;
    let scroll_height = chunks[0].height.saturating_sub(2);
    let scroll_offset = scroll_lines.saturating_sub(scroll_height);
    let scrollback = Paragraph::new(scroll_text)
        .block(Block::default().borders(Borders::ALL).title("conversation"))
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset, 0));
    frame.render_widget(scrollback, chunks[0]);

    let mut active_text = String::new();
    if let Some(notice) = &state.notice {
        active_text.push_str(&format!("* {notice}\n"));
    }
    active_text.push_str(&state.active);
    if let Some(tool) = &state.tool_line {
        active_text.push_str(&format!("\n{tool}"));
    }
    if state.turn_running && active_text.is_empty() {
        active_text.push_str("...");
    }
    let active = Paragraph::new(active_text)
        .block(Block::default().borders(Borders::ALL).title("active"))
        .wrap(Wrap { trim: false });
    frame.render_widget(active, chunks[1]);

    let state_cue = if state.turn_running {
        state
            .turn_status
            .as_ref()
            .map(|status| status.text.clone())
            .unwrap_or_else(|| "running...".to_string())
    } else {
        state
            .turn_status
            .as_ref()
            .map(|status| status.text.clone())
            .unwrap_or_default()
    };
    let model_label = state
        .options
        .model_label
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    // An empty label is the "no provider is signed in yet" state (the CLI
    // leaves it empty rather than pretending a default model is active).
    let model_label = if model_label.trim().is_empty() {
        "no model".to_string()
    } else {
        model_label
    };
    let mut status_spans = vec![
        Span::styled(
            format!(" {} ", model_label),
            theme(plain, Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "in {} cache {} out {} ",
                state.usage.input, state.usage.cache_read, state.usage.output
            ),
            theme(plain, Color::Gray),
        ),
        Span::styled(format!("| {state_cue}"), theme(plain, Color::Green)),
    ];
    // The session cost, once a turn has spent something (the status
    // line shows model, context use, session cost, extension segments).
    if state.usage.cost > 0.0 {
        status_spans.push(Span::styled(
            format!(" ${:.4} ", state.usage.cost),
            theme(plain, Color::Gray),
        ));
    }
    // The update check's notice, once today's check finds a newer tag;
    // the loop's50 ms redraw tick shows it without a wakeup of its own.
    if let Some(notice) = state
        .options
        .update_notice
        .as_ref()
        .and_then(|cell| cell.get())
    {
        status_spans.push(Span::styled(
            format!(" {notice} "),
            theme(plain, Color::Yellow),
        ));
    }
    // Extension status segments join the line (FR-UI-1: their trees,
    // our spans).
    if let Some(render) = &state.options.render_regions {
        for (_name, tree) in render("status-line") {
            for line in widget_lines(&tree.nodes).into_iter().take(1) {
                status_spans.push(Span::styled(
                    format!(" {line} "),
                    theme(plain, Color::Magenta),
                ));
            }
        }
    }
    frame.render_widget(Paragraph::new(Line::from(status_spans)), status_row);

    if let Some(footer_row) = footer_row {
        frame.render_widget(
            Paragraph::new(footer_lines.join("\n"))
                .block(Block::default().borders(Borders::ALL).title("footer")),
            footer_row,
        );
    }

    if let Some(panel_area) = panel_area {
        let mut panel_lines: Vec<String> = Vec::new();
        if let Some(render) = &state.options.render_regions {
            for (_name, tree) in render("panel") {
                panel_lines.extend(widget_lines(&tree.nodes));
            }
        }
        if panel_lines.is_empty() {
            panel_lines.push("(nothing registered for the panel)".to_string());
        }
        frame.render_widget(
            Paragraph::new(panel_lines.join("\n"))
                .block(Block::default().borders(Borders::ALL).title("panel"))
                .wrap(Wrap { trim: false }),
            panel_area,
        );
    }

    let input_text = if state.buffer.is_empty() {
        "> ".to_string()
    } else {
        format!("> {}", state.buffer)
    };
    let input = Paragraph::new(input_text)
        .block(Block::default().borders(Borders::ALL).title("input"))
        .wrap(Wrap { trim: false });
    frame.render_widget(input, input_row);
    frame.set_cursor_position(cursor_position(input_row, state));

    if let Some(modal) = &state.permission {
        let area = centered(frame.area(), 70, 9);
        frame.render_widget(Clear, area);
        let body = format!(
            "Allow this action?\n\n  {}\n\nAllow once [o] / Allow always for this pattern [a] / Deny [d]",
            modal.action
        );
        let dialog = Paragraph::new(body)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme(plain, Color::Yellow))
                    .title("permission required"),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(dialog, area);
    } else if state.modal_open {
        // The extension modal: same host-drawn chrome, extension-drawn
        // content - every line already passed through the sanitizer.
        let area = centered(frame.area(), 70, 9);
        frame.render_widget(Clear, area);
        let trees = state
            .options
            .render_regions
            .as_ref()
            .map(|render| render("modal"))
            .unwrap_or_default();
        let mut lines: Vec<String> = Vec::new();
        let mut title = String::from("extension");
        for (name, tree) in &trees {
            for line in widget_lines(&tree.nodes) {
                if title == "extension" && line.starts_with('[') {
                    title = line.trim_matches(['[', ']']).to_string();
                }
                lines.push(line);
            }
            if lines.is_empty() {
                lines.push(name.clone());
            }
        }
        let dialog = Paragraph::new(lines.join("\n"))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme(plain, Color::Magenta))
                    .title(title),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(dialog, area);
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn cursor_position(input_area: Rect, state: &UiState) -> (u16, u16) {
    let inner_width = input_area.width.saturating_sub(2).max(1) as usize;
    let at = state.cursor_index();
    // The rendered text is "> " plus the buffer up to the cursor. Walking
    // it - not just counting characters - makes explicit newlines and soft
    // wrapping both land the cursor in the right cell.
    let mut row = 0usize;
    let mut col = 0usize;
    for ch in "> ".chars().chain(state.buffer[..at].chars()) {
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
            if col >= inner_width {
                row += 1;
                col = 0;
            }
        }
    }
    let max_row = input_area.height.saturating_sub(2) as usize;
    let row = row.min(max_row);
    (input_area.x + 1 + col as u16, input_area.y + 1 + row as u16)
}

/// How many terminal rows `text` occupies at `width` columns, counting
/// explicit newlines and soft wrapping.
fn wrapped_rows(text: &str, width: usize) -> u16 {
    let width = width.max(1);
    let mut rows = 1usize;
    let mut col = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            rows += 1;
            col = 0;
        } else {
            col += 1;
            if col >= width {
                rows += 1;
                col = 0;
            }
        }
    }
    rows.min(u16::MAX as usize) as u16
}

/// Restores the terminal (raw mode off, main screen, cursor visible) when
/// dropped, so every ordinary exit and early error leaves the console usable.
struct TerminalRestore;

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::Show,
        );
    }
}

/// Restore the terminal even when a panic aborts the process: the release
/// profile sets `panic = "abort"`, which skips destructors. A Windows
/// console left in raw mode outlives the process, and that is what made the
/// next command - `ext install`'s consent prompt - look frozen.
fn install_terminal_restore_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::Show,
        );
        previous(info);
    }));
}

/// Run the interface until the user exits; returns the process exit code.
pub fn run(options: UiOptions, runner: TurnRunner) -> anyhow::Result<i32> {
    use crossterm::event::{Event, KeyModifiers, read};
    use crossterm::execute;
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    install_terminal_restore_hook();
    let mut stdout = std::io::stdout();
    crossterm::terminal::enable_raw_mode()?;
    let _restore = TerminalRestore;
    execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let (event_tx, event_rx) = std::sync::mpsc::channel::<crossterm::event::Event>();
    std::thread::spawn(move || {
        while let Ok(event) = read() {
            if event_tx.send(event).is_err() {
                break;
            }
        }
    });

    let mut state = UiState::new(options);
    let mut active_turn: Option<std::thread::JoinHandle<TurnOutcome>> = None;
    let mut turn_rx: Option<Receiver<TurnEvent>> = None;
    let mut prompt_rx: Option<Receiver<PromptRequest>> = None;
    let mut cancel_flag: Option<lca_tools::CancelFlag> = None;
    let mut next_input: Option<String> = None;

    let result = loop {
        if let Some(handle) = &active_turn
            && handle.is_finished()
        {
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
                state.notice = Some(sanitize_text(error));
            }
            state.usage.cost += outcome.usage.cost;
            continue;
        }

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
            state.turn_running = true;
            state.turn_status = Some(TurnStatusLine {
                text: "running...".into(),
            });
            state.notice = None;
            turn_rx = Some(event_rx);
            prompt_rx = Some(prompt_rx_inner);
            active_turn = Some(runner(text, channels, cancel));
        }

        // Drain worker channels without blocking the input loop.
        if let Some(rx) = &turn_rx {
            while let Ok(event) = rx.try_recv() {
                state.on_turn_event(event);
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
        }

        let frame_result = terminal.draw(|frame| draw_frame(frame, &state));
        if let Err(err) = frame_result {
            break Err(anyhow::anyhow!("render failed: {err}"));
        }

        match event_rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(Event::Key(key)) if key.kind == crossterm::event::KeyEventKind::Press => {
                match handle_key(&mut state, key) {
                    Action::Continue => {}
                    Action::Submit => {
                        let submitted = state.history.last().cloned().unwrap_or_default();
                        next_input = Some(submitted);
                    }
                    Action::CancelTurn => {
                        if let Some(cancel) = &cancel_flag {
                            cancel.cancel();
                        }
                    }
                    Action::Exit => break Ok(()),
                }
            }
            Ok(Event::Resize(width, height)) => {
                state.resize(width, height);
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Ok(()),
        }

        // A modal open while idle still needs its key handled even without
        // a turn (tests open one directly).
        let _ = KeyModifiers::CONTROL;
    };

    if let Some(cancel) = cancel_flag {
        cancel.cancel();
    }
    if let Some(handle) = active_turn {
        let _ = handle.join();
    }

    result.map(|()| 0)
}
