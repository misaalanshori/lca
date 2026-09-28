//! The interactive chat composition, ported from pi's
//! `coding-agent/src/modes/interactive/chat-viewport.ts`
//! (`pi-tui-re/src_re/agent-components/utilities.md` §1).
//!
//! The product's frame: the transcript above, the editor dock and footer
//! below, overlays composited over the viewport. It owns the engine
//! widgets - `widgets::editor::Editor`, `Transcript`, `Footer`, `Theme` -
//! and routes raw input through the pi pipeline
//! (`parse_key` -> `KeybindingsManager` -> editor actions). There is one
//! editor and one key vocabulary (ADR-0037).

use std::sync::Arc;

use lca_protocol::{CommandEffect, StopReason, TurnEvent, TurnStatus, Usage};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::keys;
use lca_tui::widgets::autocomplete::{
    ArgumentCompletions, AutocompleteItem, CombinedAutocompleteProvider, SlashCommand,
};
use lca_tui::widgets::editor::{Editor, EditorEvent};

use crate::footer::Footer;

use crate::state::{Action, LoginNext, TurnStatusLine, UiOptions, UiState, widget_lines};
use crate::theme::Theme;
use crate::transcript::{ToolStatus, Transcript};

/// One queued message (steering, ADR-0038). Carried by `Chat`; the agent
/// semantics live in `lca-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMessage {
    /// The message text.
    pub text: String,
    /// How it was submitted while the turn ran.
    pub mode: lca_protocol::SubmitMode,
}

/// The interactive chat.
pub struct Chat {
    /// The transcript.
    pub transcript: Transcript,
    /// The prompt editor.
    pub editor: Editor,
    /// The footer data (cwd, session).
    pub footer: Footer,
    /// The theme.
    pub theme: Theme,
    /// Accumulated session usage.
    pub usage: Usage,
    /// Whether a turn is running.
    pub turn_running: bool,
    /// The status-line turn state.
    pub turn_status: Option<TurnStatusLine>,
    /// The ui-world adapter (options + modals).
    pub world: UiState,
    /// Messages queued while a turn runs (ADR-0038).
    pub pending: Vec<PendingMessage>,
    /// The shared queue the running turn drains at each boundary.
    pub current_steer: Option<lca_protocol::SteerQueue>,
    /// A submitted prompt awaiting the loop's handoff.
    pub submitted: Option<String>,
    /// Whether the fullscreen (alt-screen) renderer is active (FR-UI-21).
    pub screen_mode: bool,
    /// Ctrl+X seen, waiting for the chord's second key (external editor).
    pending_ctrl_x: bool,
    /// A pending prompt-jump target (a document line index).
    jump_target: Option<usize>,
    /// The open transcript search query (FR-UI-12), when any.
    pub search: Option<String>,
    /// Document lines matching the search query.
    search_matches: Vec<usize>,
    /// The current match.
    search_index: usize,
    /// The active theme's name (FR-UI-17).
    pub theme_name: String,
    /// The open `/theme` picker with live preview, when any.
    pub theme_picker: Option<ThemePicker>,
    /// The open `/tree` branch selector, when any (FR-UI-16).
    pub tree_picker: Option<TreePicker>,
}

/// The `/tree` branch selector (FR-UI-16).
pub struct TreePicker {
    /// `(session id, display label)` entries.
    pub entries: Vec<(String, String)>,
    /// The highlighted row.
    pub selected: usize,
}

/// The `/theme` picker: a live preview that restores on cancel (FR-UI-17).
pub struct ThemePicker {
    /// The highlighted row.
    pub selected: usize,
    /// The theme name to restore when the picker is cancelled.
    pub original: String,
}

impl Chat {
    /// Build the chat: widgets, theme, and the autocomplete chain.
    pub fn new(options: UiOptions, keybindings: Arc<KeybindingsManager>) -> Chat {
        let world = UiState::new(options);
        let theme_name = if world.options.plain {
            "plain".to_string()
        } else if matches!(
            crate::theme::detect_scheme(),
            Some(lca_tui::engine::colors::ColorScheme::Light)
        ) {
            "light".to_string()
        } else {
            "default".to_string()
        };
        let theme = Theme::named(&theme_name).unwrap_or_else(Theme::colored);
        let mut editor = Editor::new();
        editor.set_keybindings(keybindings.clone());
        editor.set_autocomplete(Arc::new(provider_for(&world.options)));
        let mut transcript = Transcript::new();
        for line in &world.options.initial_lines {
            transcript.push_raw(line.clone());
        }
        let footer = Footer {
            cwd: world.options.workspace.to_string_lossy().to_string(),
            ..Default::default()
        };
        let screen_mode = world.options.fullscreen;
        Chat {
            transcript,
            editor,
            footer,
            theme,
            usage: Usage::default(),
            turn_running: false,
            turn_status: None,
            world,
            pending: Vec::new(),
            current_steer: None,
            submitted: None,
            screen_mode,
            pending_ctrl_x: false,
            jump_target: None,
            search: None,
            search_matches: Vec::new(),
            search_index: 0,
            theme_name,
            theme_picker: None,
            tree_picker: None,
        }
    }

    /// Consume a turn event into the transcript and the counters.
    pub fn on_turn_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::TextDelta(delta) => {
                self.transcript.append_text(&delta);
            }
            TurnEvent::ReasoningDelta(delta) => {
                self.transcript.append_reasoning(&delta);
            }
            TurnEvent::ToolStarted(call) => {
                self.transcript.start_tool(call.name, call.arguments);
            }
            TurnEvent::ToolFinished(result) => {
                let status = match result.status {
                    lca_protocol::ToolResultStatus::Ok => ToolStatus::Ok,
                    lca_protocol::ToolResultStatus::Error => ToolStatus::Error,
                    lca_protocol::ToolResultStatus::Denied => ToolStatus::Denied,
                    lca_protocol::ToolResultStatus::Timeout => ToolStatus::Timeout,
                };
                self.transcript
                    .finish_tool(status, Some(result.content.clone()));
            }
            TurnEvent::ToolOutputChunk { chunk, .. } => {
                self.transcript.append_tool_output(&chunk);
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
                self.world.notice = Some(format!("retry {attempt}/{max} after: {error}"));
            }
            TurnEvent::Error { message, .. } => {
                self.transcript.push_error(message);
            }
            TurnEvent::AssistantText(_) => {}
            TurnEvent::UserInjected { text, mode } => {
                // A steered message crossed the boundary: it leaves the
                // pending band and joins the transcript (ADR-0038).
                if let Some(pos) = self.pending.iter().position(|p| p.text == text) {
                    self.pending.remove(pos);
                }
                let _ = mode;
                self.transcript.push_user(text);
            }
            TurnEvent::ExtensionEvent {
                extension,
                event,
                detail,
            } => {
                self.world.notice = Some(format!("[{extension}] {event}: {detail}"));
            }
            TurnEvent::TurnEnded {
                status,
                stop_reason,
            } => {
                self.transcript.finish_assistant();
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

    /// Mark a turn as started, with the queue it drains at each boundary.
    pub fn begin_turn(&mut self, steer: lca_protocol::SteerQueue) {
        self.turn_running = true;
        self.current_steer = Some(steer);
        self.turn_status = Some(TurnStatusLine {
            text: "running...".into(),
        });
        self.world.notice = None;
        self.transcript.begin_assistant();
    }

    /// Take the submitted prompt the loop should run, if any.
    pub fn take_submitted(&mut self) -> Option<String> {
        self.submitted.take()
    }

    /// The current model label, resolved from the shared cell.
    fn model_label(&self) -> String {
        self.world
            .options
            .model_label
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Render the whole document (transcript + dock) at a width.
    pub fn render(&self, width: u16) -> Vec<String> {
        let mut out = self.transcript.render(width, &self.theme);

        // Extension footer regions (FR-UI-1).
        if let Some(render) = &self.world.options.render_regions {
            for (_name, tree) in render("footer") {
                for line in widget_lines(&tree.nodes).into_iter().take(3) {
                    out.push((self.theme.dim)(&line));
                }
            }
        }

        // A separator above the dock.
        out.push(String::new());
        out.push((self.theme.dim)(&"─".repeat(width as usize)));

        if let Some(notice) = &self.world.notice {
            // A notice can be multi-line (`/help`, `/hotkeys`, a command's
            // block output); render each line rather than embedding a
            // newline in one line string (which corrupts the screen).
            for (i, line) in notice.split('\n').enumerate() {
                let prefix = if i == 0 { "• " } else { "  " };
                for wrapped in lca_tui::engine::text::wrap_text_with_ansi(
                    &format!("{prefix}{line}"),
                    width as usize,
                ) {
                    out.push((self.theme.warn)(&wrapped));
                }
            }
        }

        // The pending-messages band (ADR-0038).
        for pending in &self.pending {
            let mark = match pending.mode {
                lca_protocol::SubmitMode::Steer => "steer",
                lca_protocol::SubmitMode::FollowUp => "next",
            };
            out.push((self.theme.dim)(&format!("  ⏳ [{mark}] {}", pending.text)));
        }
        if !self.pending.is_empty() {
            out.push((self.theme.dim)(&format!(
                "  {} queued · Alt+E restores them to the editor",
                self.pending.len()
            )));
        }

        // The autocomplete popup, when open.
        out.extend(self.editor.render_popup(width));

        // The editor, with its prompt marker on the first line.
        let mut editor_lines = self.editor.render(width.saturating_sub(2));
        if let Some(first) = editor_lines.first_mut() {
            *first = format!("{} {first}", (self.theme.accent)(">"));
        }
        out.extend(editor_lines);

        // The footer.
        out.extend(self.footer_lines(width));
        out
    }

    /// The footer lines, refreshed from the live model label and usage.
    fn footer_lines(&self, width: u16) -> Vec<String> {
        let mut footer = self.footer.clone();
        footer.usage = self.usage.clone();
        let model = self.model_label();
        footer.model = if model.trim().is_empty() {
            "no model".to_string()
        } else {
            model
        };
        if let Some(cue) = &self.turn_status {
            footer.statuses.push(cue.text.clone());
        }
        if let Some(notice) = self
            .world
            .options
            .update_notice
            .as_ref()
            .and_then(|cell| cell.get())
        {
            footer.statuses.push(notice.clone());
        }
        if !self.pending.is_empty() {
            footer
                .statuses
                .push(format!("{} queued", self.pending.len()));
        }
        if let Some(render) = &self.world.options.render_regions {
            for (_name, tree) in render("status-line") {
                for line in widget_lines(&tree.nodes).into_iter().take(1) {
                    footer.statuses.push(line);
                }
            }
        }
        footer.render(width, &self.theme)
    }

    /// The viewport the renderer paints: the document's tail, with overlays
    /// composited over it.
    pub fn viewport(&self, width: u16, height: u16, scroll: u16) -> Vec<String> {
        let document = self.render(width);
        let total = document.len();
        let end = total.saturating_sub(scroll as usize);
        let start = end.saturating_sub(height as usize);
        let mut viewport: Vec<String> = document[start..end].to_vec();
        if self.world.modal_active() || self.theme_picker.is_some() || self.tree_picker.is_some() {
            viewport.resize(height as usize, String::new());
        }
        self.compose_overlays(&mut viewport, width, height);
        viewport
    }

    /// Composite the modals and the side panel over the viewport.
    pub fn handle_key(&mut self, data: &str) -> Action {
        if let Some(action) = self.handle_modal_key(data) {
            return action;
        }

        let key = keys::parse_key(data);

        // The `/tree` selector owns the keyboard while open (FR-UI-16).
        if let Some(mut picker) = self.tree_picker.take() {
            match key.as_deref() {
                Some("escape") => {}
                Some("enter") => {
                    let (id, _) = picker.entries[picker.selected].clone();
                    self.world.notice = Some(format!("resume this branch with: lca --resume {id}"));
                }
                Some("up") | Some("k") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.tree_picker = Some(picker);
                }
                Some("down") | Some("j") => {
                    picker.selected = (picker.selected + 1).min(picker.entries.len() - 1);
                    self.tree_picker = Some(picker);
                }
                _ => self.tree_picker = Some(picker),
            }
            return Action::Continue;
        }

        // The `/theme` picker previews live and restores on cancel (FR-UI-17).
        if let Some(mut picker) = self.theme_picker.take() {
            match key.as_deref() {
                Some("escape") => {
                    self.set_theme(&picker.original);
                }
                Some("enter") => {
                    let name = crate::theme::THEMES[picker.selected].to_string();
                    self.set_theme(&name);
                }
                Some("up") | Some("k") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.preview_theme(picker.selected);
                    self.theme_picker = Some(picker);
                }
                Some("down") | Some("j") => {
                    picker.selected =
                        (picker.selected + 1).min(crate::theme::THEMES.len().saturating_sub(1));
                    self.preview_theme(picker.selected);
                    self.theme_picker = Some(picker);
                }
                _ => self.theme_picker = Some(picker),
            }
            return Action::Continue;
        }

        // The transcript search owns the keyboard while open (FR-UI-12).
        if self.search.is_some() {
            return self.handle_search_key(data, key.as_deref());
        }
        if key.as_deref() == Some("ctrl+r") {
            self.search = Some(String::new());
            self.search_matches.clear();
            self.search_index = 0;
            self.world.notice = Some("search: ".to_string());
            return Action::Continue;
        }
        // Ctrl+X Ctrl+E opens the external editor (FR-UI-15).
        if self.pending_ctrl_x {
            self.pending_ctrl_x = false;
            if key.as_deref() == Some("ctrl+e") {
                return Action::ExternalEditor;
            }
        }
        if key.as_deref() == Some("ctrl+x") {
            self.pending_ctrl_x = true;
            return Action::Continue;
        }

        // Ctrl+P toggles the side panel (a host binding, not an extension's).
        if key.as_deref() == Some("ctrl+p") {
            self.world.panel_open = !self.world.panel_open;
            return Action::Continue;
        }

        match self.editor.handle_key(data) {
            EditorEvent::Submitted(text) => self.on_submit(text),
            EditorEvent::Exit => Action::Exit,
            EditorEvent::Changed | EditorEvent::None => self.global_key(data),
        }
    }

    /// Apply a theme by name and remember it.
    fn set_theme(&mut self, name: &str) {
        self.theme_name = name.to_string();
        self.theme = Theme::named(name).unwrap_or_else(Theme::colored);
    }

    /// Preview a theme without committing it (FR-UI-17).
    fn preview_theme(&mut self, index: usize) {
        if let Some(name) = crate::theme::THEMES.get(index) {
            self.theme = Theme::named(name).unwrap_or_else(Theme::colored);
        }
    }

    /// Handle a key while the transcript search is open (FR-UI-12).
    fn handle_search_key(&mut self, data: &str, key: Option<&str>) -> Action {
        let mut query = self.search.take().unwrap_or_default();
        match key {
            Some("escape") => {
                self.search = None;
                self.search_matches.clear();
                self.world.notice = None;
                return Action::Continue;
            }
            Some("enter") => {
                self.search_next();
                self.search = Some(query);
                return Action::Continue;
            }
            Some("backspace") => {
                query.pop();
            }
            _ => {
                if let Some(text) = printable(data) {
                    query.push_str(&text);
                }
            }
        }
        self.search = Some(query.clone());
        self.refresh_search();
        self.world.notice = Some(format!(
            "search: {query}{}",
            if self.search_matches.is_empty() {
                "  (no matches)".to_string()
            } else {
                format!(
                    "  [{}/{}]",
                    self.search_index + 1,
                    self.search_matches.len()
                )
            }
        ));
        Action::Continue
    }

    /// Recompute the search matches against the current document.
    fn refresh_search(&mut self) {
        let query = self.search.clone().unwrap_or_default();
        self.search_matches.clear();
        self.search_index = 0;
        if query.is_empty() {
            return;
        }
        let width = self.world.size.0.max(1);
        let lower = query.to_lowercase();
        for (index, line) in self.render(width).iter().enumerate() {
            if strip_ansi(line).to_lowercase().contains(&lower) {
                self.search_matches.push(index);
            }
        }
        if let Some(&first) = self.search_matches.first() {
            self.jump_target = Some(first);
        }
    }

    /// Move to the next search match (FR-UI-12).
    fn search_next(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_index = (self.search_index + 1) % self.search_matches.len();
        self.jump_target = Some(self.search_matches[self.search_index]);
    }

    /// Keys the editor does not claim (the escape ladder, cancel, exit).
    fn global_key(&mut self, data: &str) -> Action {
        // Prompt jump (FR-UI-11): the authoritative matcher, since the
        // arrow-modifier dialects are not all round-tripped by `parse_key`.
        if keys::matches_key(data, "alt+up") || keys::matches_key(data, "ctrl+up") {
            self.jump_prompt(-1);
            return Action::Continue;
        }
        if keys::matches_key(data, "alt+down") || keys::matches_key(data, "ctrl+down") {
            self.jump_prompt(1);
            return Action::Continue;
        }
        match keys::parse_key(data).as_deref() {
            Some("ctrl+c") => {
                if self.turn_running {
                    Action::CancelTurn
                } else if self.world.ctrl_c_armed {
                    self.world.ctrl_c_armed = false;
                    Action::Exit
                } else {
                    self.world.ctrl_c_armed = true;
                    Action::Continue
                }
            }
            Some("escape") => {
                if self.turn_running {
                    Action::CancelTurn
                } else {
                    self.world.ctrl_c_armed = false;
                    Action::Continue
                }
            }
            Some("alt+e") if !self.pending.is_empty() => {
                // Edit-all-queued: return the queue to the editor (ADR-0038).
                self.restore_pending();
                Action::Continue
            }
            Some("alt+enter") if self.turn_running => {
                // Queue a follow-up for turn end (ADR-0038).
                let text = self.editor.submit();
                if !text.trim().is_empty() {
                    self.queue_submit(text, lca_protocol::SubmitMode::FollowUp);
                }
                Action::Continue
            }
            _ => Action::Continue,
        }
    }

    /// Handle a key while a modal or the panel owns the keyboard.
    fn handle_modal_key(&mut self, data: &str) -> Option<Action> {
        let key = keys::parse_key(data);

        // The list picker (`/login`).
        if let Some(mut prompt) = self.world.picker.take() {
            match key.as_deref() {
                Some("escape") => self.world.notice = Some("login cancelled".to_string()),
                Some("up") | Some("k") => {
                    prompt.selected = prompt.selected.saturating_sub(1);
                    self.world.picker = Some(prompt);
                }
                Some("down") | Some("j") => {
                    prompt.selected =
                        (prompt.selected + 1).min(prompt.options.len().saturating_sub(1));
                    self.world.picker = Some(prompt);
                }
                Some("enter") => {
                    if let Some(option) = prompt.options.get(prompt.selected).cloned()
                        && let Some(pick) = self.world.options.pick_login.clone()
                    {
                        let next = pick(&option.provider, &option.id);
                        self.apply_login_next(next);
                    }
                }
                _ => self.world.picker = Some(prompt),
            }
            return Some(Action::Continue);
        }

        // The single-line secret prompt (`/login`).
        if let Some(mut prompt) = self.world.secret.take() {
            match key.as_deref() {
                Some("escape") => self.world.notice = Some("login cancelled".to_string()),
                Some("enter") => {
                    let typed = std::mem::take(&mut prompt.input);
                    if typed.is_empty() {
                        self.world.notice = Some("nothing was entered".to_string());
                    } else if let Some(complete) = self.world.options.complete_login.clone() {
                        let next = complete(&prompt.provider, &typed);
                        self.apply_login_next(next);
                    }
                }
                Some("backspace") => {
                    prompt.input.pop();
                    self.world.secret = Some(prompt);
                }
                _ => match printable(data) {
                    Some(text) => {
                        prompt.input.push_str(&text);
                        self.world.secret = Some(prompt);
                    }
                    None => self.world.secret = Some(prompt),
                },
            }
            return Some(Action::Continue);
        }

        // The ad hoc-grant confirm (`/login`).
        if let Some(prompt) = self.world.grant.take() {
            match key.as_deref() {
                Some("y") | Some("Y") | Some("enter") => {
                    let message = self
                        .world
                        .options
                        .confirm_login_grant
                        .as_ref()
                        .map(|confirm| confirm(&prompt.provider, &prompt.host))
                        .unwrap_or_else(|| "nothing was changed".to_string());
                    self.world.notice = Some(crate::state::sanitize_block(&message));
                }
                Some("n") | Some("N") | Some("escape") => {
                    self.world.notice =
                        Some(format!("kept {} without the ad hoc grant", prompt.host));
                }
                _ => self.world.grant = Some(prompt),
            }
            return Some(Action::Continue);
        }

        // The permission modal.
        if let Some(modal) = self.world.permission.take() {
            use lca_permissions::Decision;
            let crate::state::PermissionModal {
                action,
                respond,
                deadline,
            } = modal;
            let _ = deadline;
            let answer = |decision: Decision| {
                if let Some(respond) = respond {
                    let _ = respond.send(decision);
                }
            };
            match key.as_deref() {
                Some("o") => answer(Decision::Once),
                Some("a") => answer(Decision::Always),
                Some("d") | Some("escape") | Some("enter") => answer(Decision::Denied),
                _ => {
                    // Any other key cancels the countdown but keeps the modal.
                    self.world.permission = Some(crate::state::PermissionModal {
                        action,
                        respond: None,
                        deadline: None,
                    });
                }
            }
            return Some(Action::Continue);
        }

        // The extension modal.
        if self.world.modal_open {
            if key.as_deref() == Some("escape") {
                self.world.modal_open = false;
                return Some(Action::Continue);
            }
            let input = if key.as_deref() == Some("enter") {
                lca_protocol::UiInput::Submit {
                    text: self.editor.text(),
                }
            } else {
                crate::state::key_input(key.as_deref().unwrap_or(data))
            };
            if let Some(interactor) = self.world.options.ui_events.clone()
                && let Some((_, effect)) = interactor("modal", &input)
            {
                return Some(self.apply_effect(effect));
            }
            return Some(Action::Continue);
        }

        // The side panel.
        if self.world.panel_open {
            if key.as_deref() == Some("ctrl+p") {
                return None;
            }
            let input = crate::state::key_input(key.as_deref().unwrap_or(data));
            if let Some(interactor) = self.world.options.ui_events.clone() {
                if let Some((_, effect)) = interactor("panel", &input) {
                    return Some(self.apply_effect(effect));
                }
                self.world.panel_open = false;
            }
        }

        None
    }

    /// Apply one extension effect (from real user input only, FR-UI-6).
    fn apply_effect(&mut self, effect: lca_protocol::UiEffect) -> Action {
        use lca_protocol::UiEffect;
        match effect {
            UiEffect::None => Action::Continue,
            UiEffect::CloseModal => {
                self.world.modal_open = false;
                Action::Continue
            }
            UiEffect::OpenModal => {
                self.world.modal_open = true;
                Action::Continue
            }
            UiEffect::ShowNotice(text) => {
                self.world.notice = Some(crate::state::sanitize_block(&text));
                Action::Continue
            }
            UiEffect::InsertText(text) => {
                self.editor.insert_str(&crate::state::sanitize_block(&text));
                Action::Continue
            }
            UiEffect::SubmitPrompt(text) => {
                self.submitted = Some(text);
                Action::Submit
            }
        }
    }

    /// Apply the CLI's next login step.
    fn apply_login_next(&mut self, next: LoginNext) {
        crate::state::apply_login_next(&mut self.world, next);
    }

    /// Handle a submitted editor buffer.
    fn on_submit(&mut self, text: String) -> Action {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Action::Continue;
        }
        // `!cmd` / `!!cmd` shell mode (FR-UI-14).
        if trimmed.starts_with('!') {
            return self.run_shell_mode(trimmed);
        }
        if trimmed.starts_with('/') {
            return self.dispatch_command(trimmed);
        }
        // A turn needs a model; say so rather than failing deep in the
        // provider with a transport error.
        if self.model_label().trim().is_empty() {
            self.world.notice = Some(
                "No model is active. Use /login to sign in, or /model to choose one.".to_string(),
            );
            return Action::Continue;
        }
        // ADR-0038: while a turn runs, a submitted message queues rather
        // than starting a second turn. Enter steers (injected at the next
        // boundary); Alt+Enter queues a follow-up for turn end.
        if self.turn_running {
            self.queue_submit(text, lca_protocol::SubmitMode::Steer);
            return Action::Continue;
        }
        self.transcript.push_user(text.clone());
        self.submitted = Some(text);
        Action::Submit
    }

    /// Queue a message submitted while a turn runs (ADR-0038). `Steer`
    /// entries also go to the running turn's boundary queue.
    pub fn queue_submit(&mut self, text: String, mode: lca_protocol::SubmitMode) {
        if mode == lca_protocol::SubmitMode::Steer
            && let Some(steer) = &self.current_steer
        {
            steer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(lca_protocol::QueuedMessage {
                    text: text.clone(),
                    mode,
                });
        }
        self.pending.push(PendingMessage { text, mode });
    }

    /// Pop the next queued message to auto-submit at turn end, in order.
    pub fn take_next_pending(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let next = self.pending.remove(0);
        self.transcript.push_user(next.text.clone());
        Some(next.text)
    }

    /// Run a `!`/`!!` shell command and show it as a bash card (FR-UI-14).
    fn run_shell_mode(&mut self, line: &str) -> Action {
        let excluded = line.starts_with("!!");
        let command = line.trim_start_matches('!').trim();
        if command.is_empty() {
            return Action::Continue;
        }
        let Some(run_shell) = self.world.options.hooks.run_shell.clone() else {
            self.world.notice = Some("shell mode is not available in this host".to_string());
            return Action::Continue;
        };
        // The command card is visible either way; the host records the
        // command for the model's context only when it is not `!!`.
        self.transcript.start_tool("bash", command.to_string());
        let output = run_shell(command, excluded);
        self.transcript.finish_tool(ToolStatus::Ok, Some(output));
        self.world.notice = Some(if excluded {
            format!("ran `{command}` (excluded from context)")
        } else {
            format!("ran `{command}`")
        });
        Action::Continue
    }

    /// Dispatch a slash command line.
    fn dispatch_command(&mut self, line: &str) -> Action {
        let command_line = line.strip_prefix('/').unwrap_or(line);
        let mut parts = command_line.splitn(2, ' ');
        let name = parts.next().unwrap_or("").to_string();
        let argument = parts.next().unwrap_or("").to_string();

        match name.as_str() {
            "help" => {
                self.world.notice = Some(help_notice(&self.world.options.slash_commands));
                return Action::Continue;
            }
            "hotkeys" => {
                self.world.notice = Some(hotkeys_notice());
                return Action::Continue;
            }
            "fullscreen" => {
                self.screen_mode = !self.screen_mode;
                if let Some(persist) = &self.world.options.hooks.persist_screen_mode {
                    persist(self.screen_mode);
                }
                self.world.notice = Some(format!(
                    "screen mode: {}",
                    if self.screen_mode {
                        "fullscreen"
                    } else {
                        "scrollback"
                    }
                ));
                return Action::Continue;
            }
            "theme" => {
                self.theme_picker = Some(ThemePicker {
                    selected: crate::theme::THEMES
                        .iter()
                        .position(|name| *name == self.theme_name)
                        .unwrap_or(0),
                    original: self.theme_name.clone(),
                });
                return Action::Continue;
            }
            "tree" => {
                let entries = self
                    .world
                    .options
                    .hooks
                    .session_tree
                    .as_ref()
                    .map(|tree| tree())
                    .unwrap_or_default();
                if entries.is_empty() {
                    self.world.notice = Some("no branches yet".to_string());
                } else {
                    self.tree_picker = Some(TreePicker {
                        entries,
                        selected: 0,
                    });
                }
                return Action::Continue;
            }
            "fork" => {
                let Some(fork_at) = self.world.options.hooks.fork_at.clone() else {
                    self.world.notice = Some("forking is not available in this host".to_string());
                    return Action::Continue;
                };
                match argument.trim().parse::<usize>() {
                    Ok(index) => self.world.notice = Some(fork_at(index)),
                    Err(_) => self.world.notice = Some("usage: /fork <message-index>".to_string()),
                }
                return Action::Continue;
            }
            "quit" | "exit" => return Action::Exit,
            _ => {}
        }

        if name == "login"
            && let Some(login) = self.world.options.login.clone()
        {
            let next = login(&argument);
            self.apply_login_next(next);
            return Action::Continue;
        }

        let full = format!("/{name}");
        if self
            .world
            .options
            .slash_commands
            .iter()
            .any(|command| command == &full)
        {
            match (self.world.options.invoke_command)(&name, &argument) {
                CommandEffect::ShowWidget(text) => {
                    self.world.notice = Some(crate::state::sanitize_block(&text))
                }
                CommandEffect::AttachImage {
                    media_type,
                    bytes,
                    note,
                } => {
                    self.transcript
                        .push_image(lca_tui::widgets::image::ImageInfo::new(media_type, &bytes));
                    self.world.notice = Some(crate::state::sanitize_block(&note));
                }
                CommandEffect::InsertText(text) => {
                    self.editor.insert_str(&crate::state::sanitize_block(&text))
                }
                CommandEffect::SubmitPrompt(text) => {
                    self.transcript.push_user(text.clone());
                    self.submitted = Some(text);
                    return Action::Submit;
                }
                CommandEffect::None => {}
            }
            return Action::Continue;
        }
        self.world.notice = Some(crate::state::sanitize_text(&format!(
            "unknown command /{name}"
        )));
        Action::Continue
    }

    /// Move the prompt-jump target to the previous/next user message
    /// (FR-UI-11). The scroll is applied by the loop on the next frame.
    fn jump_prompt(&mut self, delta: i32) {
        let width = self.world.size.0.max(1);
        let offsets = self.transcript.user_offsets(width, &self.theme);
        if offsets.is_empty() {
            return;
        }
        let current = self.jump_target.unwrap_or(0);
        let index = offsets.iter().position(|&o| o >= current).unwrap_or(0) as i32;
        let next = (index + delta).clamp(0, offsets.len() as i32 - 1) as usize;
        self.jump_target = Some(offsets[next]);
    }

    /// The scroll value that centers a pending jump target, if any
    /// (FR-UI-11). Cleared once taken.
    pub fn take_jump_scroll(&mut self, width: u16, height: u16) -> Option<u16> {
        let target = self.jump_target.take()?;
        let total = self.render(width).len();
        let scroll = total.saturating_sub(target + height as usize / 2);
        Some(scroll.min(u16::MAX as usize) as u16)
    }

    /// Restore the queued messages to the editor, in order (FR-CORE-12).
    pub fn restore_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        // A restored message must not also be injected at the next boundary.
        if let Some(steer) = &self.current_steer {
            steer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
        }
        let mut text = String::new();
        for (i, pending) in self.pending.drain(..).enumerate() {
            if i > 0 {
                text.push('\n');
            }
            text.push_str(&pending.text);
        }
        self.editor.insert_str(&text);
    }
}

/// The visible text of a line with ANSI stripped (for search matching).
pub(super) fn strip_ansi(line: &str) -> String {
    lca_tui::engine::text::strip_terminal_sequences(line)
}

/// Wrap case-insensitive occurrences of `query` in inverse video
/// (char-boundary-safe; skips a line when case folding changes its length).
pub(super) fn highlight_matches(line: &str, query: &str) -> String {
    let line_chars: Vec<char> = line.chars().collect();
    let query_chars: Vec<char> = query.to_lowercase().chars().collect();
    if query_chars.is_empty() {
        return line.to_string();
    }
    let lower: Vec<char> = line.to_lowercase().chars().collect();
    if lower.len() != line_chars.len() {
        return line.to_string();
    }
    let mut out = String::new();
    let mut i = 0;
    while i + query_chars.len() <= line_chars.len() {
        if lower[i..i + query_chars.len()] == query_chars[..] {
            out.push_str("\x1b[7m");
            out.extend(&line_chars[i..i + query_chars.len()]);
            out.push_str("\x1b[27m");
            i += query_chars.len();
        } else {
            out.push(line_chars[i]);
            i += 1;
        }
    }
    out.extend(&line_chars[i..]);
    out
}

/// Decode the printable text a key inserts, if any.
fn printable(data: &str) -> Option<String> {
    if let Some(text) = keys::decode_printable_key(data) {
        return Some(text);
    }
    match keys::parse_key(data) {
        Some(key) if key.chars().count() == 1 => Some(key),
        Some(key) if key == "space" => Some(" ".to_string()),
        _ => None,
    }
}

/// Build the autocomplete chain: commands, arguments, and file paths.
fn provider_for(options: &UiOptions) -> CombinedAutocompleteProvider {
    let models = options.models.clone();
    let providers: Vec<String> = options
        .slash_commands
        .iter()
        .filter_map(|c| c.strip_prefix('/'))
        .filter_map(|c| c.strip_suffix(".login"))
        .map(str::to_string)
        .collect();
    let commands: Vec<SlashCommand> = options
        .slash_commands
        .iter()
        .map(|command| {
            let name = command.strip_prefix('/').unwrap_or(command).to_string();
            let argument_completions: Option<ArgumentCompletions> = match name.as_str() {
                "model" => {
                    let models = models.clone();
                    Some(Arc::new(move |prefix: &str| {
                        models
                            .iter()
                            .filter(|m| m.starts_with(prefix))
                            .map(|m| AutocompleteItem {
                                value: m.clone(),
                                label: m.clone(),
                                description: None,
                            })
                            .collect()
                    }))
                }
                "login" => {
                    let providers = providers.clone();
                    Some(Arc::new(move |prefix: &str| {
                        providers
                            .iter()
                            .filter(|p| p.starts_with(prefix))
                            .map(|p| AutocompleteItem {
                                value: p.clone(),
                                label: p.clone(),
                                description: None,
                            })
                            .collect()
                    }))
                }
                _ => None,
            };
            SlashCommand {
                name,
                description: None,
                argument_hint: None,
                argument_completions,
            }
        })
        .collect();
    CombinedAutocompleteProvider::new(commands, options.workspace.clone())
}

/// One-line descriptions for the interface's own commands.
fn command_help(command: &str) -> &'static str {
    match command {
        "/help" => "list commands and keys",
        "/hotkeys" => "list every key binding",
        "/fullscreen" => "toggle fullscreen and scrollback renderers",
        "/theme" => "pick a theme with live preview",
        "/tree" => "browse session branches",
        "/fork" => "fork a branch at a message (usage: /fork <n>)",
        "/exit" => "leave the interface",
        "/login" => "sign in to a provider",
        "/logout" => "clear the provider's stored key",
        "/usage" => "show the provider's usage, when it has one",
        "/model" => "list or switch the session's model",
        "/compact" => "summarize the session to free context",
        "/attach" => "attach an image to the next message",
        _ => "",
    }
}

/// The `/hotkeys` text: the binding registry prints itself.
fn hotkeys_notice() -> String {
    let kb = KeybindingsManager::new();
    let mut lines = vec!["keys:".to_string()];
    for (action, keys) in kb.resolved_bindings() {
        if keys.is_empty() {
            continue;
        }
        let description = kb.description(&action).unwrap_or("");
        lines.push(format!("  {} - {description}", keys.join(", ")));
    }
    lines.join("\n")
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
        let help = command_help(command);
        if !help.is_empty() {
            out.push_str(" - ");
            out.push_str(help);
        }
        out.push('\n');
    }
    out.push_str("Enter sends, Shift+Enter adds a line, Tab completes, Ctrl+C cancels");
    out
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
