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

use lca_core::{StopReason, TurnEvent, TurnOutcome, TurnStatus};
use lca_protocol::Usage;
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

/// Static inputs for the interface.
pub struct UiOptions {
    /// `provider/model` for the status line.
    pub model_label: String,
    /// Conversation lines already resolved for display (resume).
    pub initial_lines: Vec<String>,
    /// Plain-text rendering (FR-UI-5).
    pub plain: bool,
    /// Session statistics for `/stats`.
    pub stats: Arc<dyn Fn() -> String + Send + Sync>,
    /// Slash commands offered by completion.
    pub slash_commands: Vec<String>,
    /// Workspace root for path completion.
    pub workspace: PathBuf,
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
            size: (80, 24),
        }
    }

    /// Record a terminal resize (FR-UI-3: nothing is lost).
    pub fn resize(&mut self, width: u16, height: u16) {
        self.size = (width, height);
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
    /// Nothing外 external; keep editing.
    Continue,
    /// Submit the buffer as a user turn.
    Submit,
    /// Cancel the running turn (FR-CORE-5).
    CancelTurn,
    /// Exit the interface.
    Exit,
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

    match key.code {
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
            state.buffer.push('\n');
            Action::Continue
        }
        KeyCode::Enter => {
            if state.buffer.is_empty() {
                return Action::Continue;
            }
            let submitted = state.buffer.clone();
            if let Some(command) = submitted.strip_prefix('/') {
                let name = command.split_whitespace().next().unwrap_or("").to_string();
                state.history.push(submitted);
                state.history_index = None;
                state.buffer.clear();
                if name == "stats" {
                    state.notice = Some((state.options.stats)());
                    return Action::Continue;
                }
                if name == "exit" || name == "quit" {
                    return Action::Exit;
                }
                let known = state
                    .options
                    .slash_commands
                    .iter()
                    .any(|c| c.trim_start_matches('/') == name.as_str());
                state.notice = Some(if known {
                    format!("/{name} is not available in this build yet")
                } else {
                    format!("unknown command /{name}")
                });
                return Action::Continue;
            }
            let submitted = std::mem::take(&mut state.buffer);
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
            Action::Continue
        }
        KeyCode::Tab => {
            complete(state);
            Action::Continue
        }
        KeyCode::Backspace => {
            state.buffer.pop();
            Action::Continue
        }
        KeyCode::Left if !state.buffer.is_empty() => {
            state.buffer.pop();
            Action::Continue
        }
        KeyCode::Char(c) => {
            state.buffer.push(c);
            state.ctrl_c_armed = false;
            Action::Continue
        }
        _ => Action::Continue,
    }
}

fn complete(state: &mut UiState) {
    if state.buffer.starts_with('/') && !state.buffer.contains(char::is_whitespace) {
        let prefix = state.buffer.as_str();
        let matches: Vec<&String> = state
            .options
            .slash_commands
            .iter()
            .filter(|command| command.starts_with(prefix))
            .collect();
        if matches.len() == 1 {
            state.buffer = matches[0].clone();
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
    } else if let Some(common) = common_prefix(&matches)
        && common.len() > dir_part.len() + file_prefix.len()
    {
        state.buffer = format!("{head}{common}");
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

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let scroll_text = state.scrollback.join("\n");
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
    let status_spans = vec![
        Span::styled(
            format!(" {} ", state.options.model_label),
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
    frame.render_widget(Paragraph::new(Line::from(status_spans)), chunks[2]);

    let input_text = if state.buffer.is_empty() {
        "> ".to_string()
    } else {
        format!("> {}", state.buffer)
    };
    let input = Paragraph::new(input_text)
        .block(Block::default().borders(Borders::ALL).title("input"))
        .wrap(Wrap { trim: false });
    frame.render_widget(input, chunks[3]);
    frame.set_cursor_position(cursor_position(chunks[3], state));

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
    // Single-line view of the buffer: the cursor sits after the last char.
    let (last_line, row) = match state.buffer.rfind('\n') {
        Some(index) => (&state.buffer[index + 1..], 1u16),
        None => (state.buffer.as_str(), 0u16),
    };
    let inner_x = last_line.chars().count() as u16;
    (
        input_area.x + 1 + inner_x.min(input_area.width.saturating_sub(3)),
        input_area.y + 1 + row,
    )
}

/// Run the interface until the user exits; returns the process exit code.
pub fn run(options: UiOptions, runner: TurnRunner) -> anyhow::Result<i32> {
    use crossterm::event::{Event, KeyModifiers, read};
    use crossterm::execute;
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    let mut stdout = std::io::stdout();
    crossterm::terminal::enable_raw_mode()?;
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
                state.notice = Some(error.clone());
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
    crossterm::terminal::disable_raw_mode()?;
    execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result.map(|()| 0)
}
