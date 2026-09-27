//! The interactive composition root, ported in shape from pi's
//! `coding-agent/src/modes/interactive/interactive-mode.ts`
//! (`pi-tui-re/src_re/agent-components/interactive-mode.md`).
//!
//! The event loop: raw input from the engine, the editor and global keys,
//! a turn on a worker thread whose events stream into the transcript, the
//! permission modal, and the footer. Main-screen mode keeps the transcript
//! in scrollback; a fullscreen toggle swaps in the alt-screen renderer with
//! application-owned selection (owner issue #2).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use lca_permissions::Decision;
use lca_protocol::{CommandEffect, TurnEvent, TurnOutcome, TurnStatus};
use lca_tools::CancelFlag;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::terminal::{ProcessTerminal, Terminal};
use lca_tui::engine::text::truncate_to_width;
use lca_tui::widgets::editor::{Editor, EditorEvent};

use crate::chat::Chat;
use crate::theme::Theme;
use crate::transcript::ToolStatus;

/// A worker asking for permission (the exact command or path).
pub struct PromptRequest {
    /// The exact action display.
    pub action: String,
    /// Where the decision goes.
    pub respond: SyncSender<Decision>,
}

/// Channels between the UI loop and the turn worker.
pub struct TurnChannels {
    /// Turn events flow to the UI.
    pub events: SyncSender<TurnEvent>,
    /// Permission requests flow to the UI.
    pub prompt: SyncSender<PromptRequest>,
}

/// Starts one turn on a worker thread; the binary supplies this.
pub type TurnRunner = Box<
    dyn Fn(String, TurnChannels, CancelFlag) -> std::thread::JoinHandle<TurnOutcome> + Send + Sync,
>;

/// A slash-command invoker.
pub type CommandInvoker = Arc<dyn Fn(&str, &str) -> CommandEffect + Send + Sync>;

/// Static inputs for the interface.
pub struct AppOptions {
    /// `provider/model` for the footer (a cell: `/model` rewrites it).
    pub model_label: Arc<Mutex<String>>,
    /// The working directory shown in the footer.
    pub cwd: PathBuf,
    /// The session title.
    pub session_name: String,
    /// Plain-text rendering (FR-UI-5).
    pub plain: bool,
    /// Slash commands offered by completion.
    pub slash_commands: Vec<String>,
    /// Invoke a registered slash command.
    pub invoke_command: CommandInvoker,
    /// The context window of the active model (0 = unknown).
    pub context_window: u64,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            model_label: Arc::new(Mutex::new(String::new())),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            session_name: String::new(),
            plain: false,
            slash_commands: Vec::new(),
            invoke_command: Arc::new(|_, _| CommandEffect::None),
            context_window: 0,
        }
    }
}

/// The interactive app state.
pub struct App {
    /// The chat document.
    pub chat: Chat,
    /// The keybindings.
    pub keybindings: Arc<KeybindingsManager>,
    /// Static options.
    pub options: AppOptions,
    /// The open permission prompt, if any.
    pub permission: Option<PromptRequest>,
    /// Ctrl+C seen once on an idle prompt.
    pub ctrl_c_armed: bool,
    /// The last usage accumulated.
    pub usage: lca_protocol::Usage,
    /// Context tokens used (roughly, from the last turn).
    pub context_used: u64,
}

impl App {
    /// A new app.
    pub fn new(options: AppOptions, theme: Theme) -> Self {
        let keybindings = Arc::new(KeybindingsManager::new());
        let mut editor = Editor::new();
        editor.set_keybindings(keybindings.clone());
        // Slash-command completion.
        let commands = options
            .slash_commands
            .iter()
            .map(|name| lca_tui::widgets::autocomplete::SlashCommand {
                name: name.trim_start_matches('/').to_string(),
                description: None,
                argument_hint: None,
                argument_completions: None,
            })
            .collect();
        let provider = Arc::new(
            lca_tui::widgets::autocomplete::CombinedAutocompleteProvider::new(
                commands,
                options.cwd.clone(),
            ),
        );
        editor.set_autocomplete(provider);
        let mut chat = Chat::new(editor, theme);
        chat.footer.cwd = options.cwd.to_string_lossy().to_string();
        chat.footer.session = options.session_name.clone();
        chat.footer.context_window = options.context_window;
        Self {
            chat,
            keybindings,
            options,
            permission: None,
            ctrl_c_armed: false,
            usage: lca_protocol::Usage::default(),
            context_used: 0,
        }
    }

    /// Handle a raw key while a turn is not running. Returns a submitted
    /// prompt, or `None`.
    pub fn handle_key(&mut self, data: &str) -> Option<String> {
        // Permission modal takes priority.
        if let Some(prompt) = &self.permission {
            if self.keybindings.matches(data, "tui.select.confirm") {
                let _ = prompt.respond.send(Decision::Once);
                self.permission = None;
                return None;
            }
            if self.keybindings.matches(data, "tui.select.cancel") {
                let _ = prompt.respond.send(Decision::Denied);
                self.permission = None;
                return None;
            }
            return None;
        }
        // Ctrl+C: cancel/exit when idle.
        if data == "\x03" {
            if self.ctrl_c_armed {
                return Some("/exit".to_string());
            }
            self.ctrl_c_armed = true;
            self.chat.notice = Some("press Ctrl+C again to exit".to_string());
            return None;
        }
        self.ctrl_c_armed = false;
        match self.chat.editor.handle_key(data) {
            EditorEvent::Submitted(text) => Some(text),
            EditorEvent::Changed | EditorEvent::None | EditorEvent::Exit => None,
        }
    }

    /// Apply a turn event to the transcript.
    pub fn on_turn_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::TextDelta(delta) => self.chat.transcript.append_text(&delta),
            TurnEvent::ReasoningDelta(delta) => self.chat.transcript.append_reasoning(&delta),
            TurnEvent::AssistantText(_) => {} // deltas already streamed the text
            TurnEvent::ToolStarted(call) => {
                self.chat
                    .transcript
                    .start_tool(call.name.clone(), summarize_args(&call.arguments));
            }
            TurnEvent::ToolFinished(result) => {
                let status = match result.status {
                    lca_protocol::ToolResultStatus::Ok => ToolStatus::Ok,
                    lca_protocol::ToolResultStatus::Error => ToolStatus::Error,
                    lca_protocol::ToolResultStatus::Denied => ToolStatus::Denied,
                    lca_protocol::ToolResultStatus::Timeout => ToolStatus::Timeout,
                };
                self.chat
                    .transcript
                    .finish_tool(status, Some(result.content.clone()));
            }
            TurnEvent::Usage(usage) => {
                self.usage.input += usage.input;
                self.usage.output += usage.output;
                self.usage.cache_read += usage.cache_read;
                self.usage.cache_write += usage.cache_write;
                self.usage.cost += usage.cost;
                self.chat.footer.usage = self.usage.clone();
            }
            TurnEvent::Error { message, .. } => {
                self.chat.transcript.finish_assistant();
                self.chat.transcript.push_error(message);
            }
            TurnEvent::TurnEnded { status, .. } => {
                self.chat.transcript.finish_assistant();
                self.chat.turn_running = false;
                if status == TurnStatus::Error {
                    self.chat.notice = Some("turn ended with an error".to_string());
                }
            }
            _ => {}
        }
    }

    /// Refresh the footer's model label from the shared cell.
    pub fn refresh_model(&mut self) {
        if let Ok(label) = self.options.model_label.lock() {
            self.chat.footer.model = label.clone();
        }
    }

    /// Render the whole document at a width.
    pub fn render(&self, width: u16) -> Vec<String> {
        let mut lines = self.chat.render(width);
        if let Some(prompt) = &self.permission {
            lines.push(String::new());
            lines.push((self.chat.theme.warn)("Permission required:"));
            for line in prompt.action.lines() {
                lines.push(truncate_to_width(line, width as usize, "…", false));
            }
            lines.push((self.chat.theme.dim)("[enter] allow once   [esc] deny"));
        }
        lines
    }
}

fn summarize_args(args: &str) -> String {
    let compact = args.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_to_width(&compact, 60, "…", false)
}

/// Run the interactive interface until the user exits.
pub fn run(options: AppOptions, runner: TurnRunner) -> anyhow::Result<i32> {
    let theme = if options.plain {
        Theme::plain()
    } else {
        Theme::colored()
    };
    let mut app = App::new(options, theme);

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

    let mut renderer = lca_tui::engine::main_screen::MainScreenRenderer::new();
    let mut event_rx: Option<Receiver<TurnEvent>> = None;
    let mut prompt_rx: Option<Receiver<PromptRequest>> = None;
    let mut active: Option<std::thread::JoinHandle<TurnOutcome>> = None;
    let mut cancel: Option<CancelFlag> = None;
    let mut exit_code = 0;

    // Initial frame.
    let width = terminal.columns();
    let rows = terminal.rows();
    let lines = app.render(width);
    renderer.render(&mut terminal, lines, width, rows);

    'main: loop {
        // A finished turn.
        if active.as_ref().is_some_and(|handle| handle.is_finished()) {
            if let Some(handle) = active.take() {
                let _ = handle.join();
            }
            cancel = None;
            event_rx = None;
            prompt_rx = None;
            app.chat.turn_running = false;
        }

        // Drain turn events.
        if let Some(rx) = &event_rx {
            while let Ok(event) = rx.try_recv() {
                app.on_turn_event(event);
            }
        }
        // Drain permission requests.
        if let Some(rx) = &prompt_rx {
            while let Ok(request) = rx.try_recv() {
                app.permission = Some(request);
            }
        }

        // Input with a short timeout so turn events are polled.
        match input_rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(data) => {
                // Ctrl+C while a turn runs cancels it.
                if data == "\x03" && app.chat.turn_running {
                    if let Some(flag) = &cancel {
                        flag.cancel();
                    }
                    app.chat.notice = Some("cancelling…".to_string());
                } else if !app.chat.turn_running
                    && let Some(text) = app.handle_key(&data)
                {
                    if text == "/exit" {
                        break 'main;
                    }
                    if text.starts_with('/') {
                        let (name, argument) = split_command(&text);
                        match (app.options.invoke_command)(name, argument) {
                            CommandEffect::InsertText(t) => app.chat.editor.set_text(&t),
                            CommandEffect::SubmitPrompt(p) => {
                                app.chat.editor.set_text(&p);
                            }
                            CommandEffect::ShowWidget(w) => app.chat.notice = Some(w),
                            CommandEffect::None => {}
                        }
                    } else {
                        submit(
                            &mut app,
                            &runner,
                            &mut event_rx,
                            &mut prompt_rx,
                            &mut active,
                            &mut cancel,
                            text,
                        );
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break 'main,
        }

        let _ = resize_rx.try_recv();
        app.refresh_model();
        app.chat.footer.context_used = app.context_used;
        let width = terminal.columns();
        let rows = terminal.rows();
        let lines = app.render(width);
        renderer.render(&mut terminal, lines, width, rows);
    }

    renderer.finish(&mut terminal);
    terminal.drain_input(1000, 50);
    terminal.stop();
    if exit_code == 0 {
        exit_code = 0;
    }
    Ok(exit_code)
}

fn split_command(text: &str) -> (&str, &str) {
    let without = text.trim_start_matches('/');
    match without.split_once(char::is_whitespace) {
        Some((name, arg)) => (name, arg.trim()),
        None => (without, ""),
    }
}

fn submit(
    app: &mut App,
    runner: &TurnRunner,
    event_rx: &mut Option<Receiver<TurnEvent>>,
    prompt_rx: &mut Option<Receiver<PromptRequest>>,
    active: &mut Option<std::thread::JoinHandle<TurnOutcome>>,
    cancel: &mut Option<CancelFlag>,
    text: String,
) {
    let submitted = app.chat.editor.submit();
    let prompt = if text.trim().is_empty() {
        submitted
    } else {
        text
    };
    if prompt.trim().is_empty() {
        return;
    }
    app.chat.transcript.push_user(prompt.clone());
    app.chat.transcript.begin_assistant();
    app.chat.turn_running = true;
    app.chat.notice = None;

    let (event_tx, event_rx_new) = std::sync::mpsc::sync_channel::<TurnEvent>(1024);
    let (prompt_tx, prompt_rx_new) = std::sync::mpsc::sync_channel::<PromptRequest>(16);
    *event_rx = Some(event_rx_new);
    *prompt_rx = Some(prompt_rx_new);
    let flag = CancelFlag::new();
    *cancel = Some(flag.clone());
    let channels = TurnChannels {
        events: event_tx,
        prompt: prompt_tx,
    };
    *active = Some(runner(prompt, channels, flag));
}
