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

use lca_protocol::{Record, StopReason, TurnEvent, TurnStatus, Usage};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::keys;
use lca_tui::widgets::editor::{Editor, EditorEvent};

use crate::chat_commands::{paste_text, printable, provider_for};
use crate::chat_pickers::{
    GrantPicker, ModelPicker, ShellRun, ThemePicker, ThinkingPicker, TreePicker, TrustPicker,
};
use crate::footer::Footer;

use crate::separator::Separator;
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
    /// The keybinding registry (app actions are looked up here, so an
    /// embedder that rebinds is honored).
    pub keybindings: Arc<KeybindingsManager>,
    /// The theme.
    pub theme: Theme,
    /// Accumulated session usage.
    pub usage: Usage,
    /// Whether a turn is running.
    pub turn_running: bool,
    /// Whether a background `/compact` is summarizing (the interface stays
    /// responsive while it does; the separator says `Working`).
    pub compacting: bool,
    /// The separator row's state and spinner (chrome.md's
    /// spinner-in-the-border, R2).
    pub separator: Separator,
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
    /// How that prompt was queued (ADR-0038): `None` for an ordinary
    /// submit, `steer`/`follow-up` when it was queued while a turn ran and
    /// is now flushing as this turn - the record carries the marker
    /// (`docs/session-log-format.md`).
    pub(crate) submitted_queue: Option<lca_protocol::SubmitMode>,
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
    /// The configured theme setting (`ui.theme`, S5).
    theme_setting: String,
    /// Where custom theme files live.
    theme_dir: std::path::PathBuf,
    /// The picker's theme names (built-ins plus custom files).
    pub theme_names: Vec<String>,
    /// Whether the theme still follows the detected terminal scheme (R10);
    /// an explicit pick clears it.
    theme_auto: bool,
    /// The open `/theme` picker with live preview, when any.
    pub theme_picker: Option<ThemePicker>,
    /// The open `/thinking` level picker, when any (R1).
    pub thinking_picker: Option<ThinkingPicker>,
    /// The open `/model` picker, when any (R9).
    pub model_picker: Option<ModelPicker>,
    /// The open `/grants` view, when any (S8).
    pub grants_picker: Option<GrantPicker>,
    /// The open `/tree` branch selector, when any (FR-UI-16).
    pub tree_picker: Option<TreePicker>,
    /// The open `/trust` picker, when any (ADR-0039).
    pub trust_picker: Option<TrustPicker>,
    /// The running `!`/`!!` command, when any (R4).
    pub shell: Option<ShellRun>,
    /// The open `/resume` session picker (R2).
    pub resume_picker: Option<crate::resume::ResumePicker>,
}

impl Chat {
    /// Build the chat: widgets, theme, and the autocomplete chain.
    pub fn new(options: UiOptions, keybindings: Arc<KeybindingsManager>) -> Chat {
        let mut world = UiState::new(options);
        // The resumed session's records replay with the live rendering and
        // the trailing notices come after them, so both are taken out of the
        // options before `world` moves into the chat (FR-UI-7).
        let initial_records = std::mem::take(&mut world.options.initial_records);
        let initial_tail = std::mem::take(&mut world.options.initial_tail_lines);
        let attachment_loader = world.options.hooks.load_attachment.clone();
        let thinking_visibility = world.options.thinking_visibility;
        // S5: the configured theme resolves through the auto-pair grammar
        // (a built-in, a custom file, or `auto` following the detected
        // scheme); an invalid custom file keeps the last-good palette and
        // lands in the notice.
        let theme_setting = if world.options.plain {
            "plain".to_string()
        } else {
            world.options.theme.clone()
        };
        let theme_dir = world.options.theme_dir.clone();
        let theme_names = if world.options.themes.is_empty() {
            crate::theme::THEMES
                .iter()
                .map(|name| name.to_string())
                .collect()
        } else {
            world.options.themes.clone()
        };
        let (theme, theme_notice) =
            crate::theme::load(&theme_setting, crate::theme::detect_scheme(), &theme_dir);
        let theme_name = theme.name.clone();
        let theme_auto = !world.options.plain && matches!(theme_setting.as_str(), "" | "auto");
        world.notice = theme_notice;
        let mut editor = Editor::new();
        editor.set_keybindings(keybindings.clone());
        editor.set_autocomplete(Arc::new(provider_for(&world.options)));
        let mut transcript = Transcript::new();
        for line in &world.options.initial_lines {
            transcript.push_raw(line.clone());
        }
        let footer = Footer {
            cwd: world.options.workspace.to_string_lossy().to_string(),
            yolo: world.options.yolo,
            context_window: *world
                .options
                .context_window
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ..Default::default()
        };
        let screen_mode = world.options.fullscreen;
        // Pi's folder-trust prompt: open the picker at startup only when the
        // project has something to gate and no decision yet (ADR-0039).
        let trust_prompt = world
            .options
            .hooks
            .trust_needed
            .as_ref()
            .is_some_and(|needed| needed());
        let mut chat = Chat {
            keybindings,
            transcript,
            editor,
            footer,
            theme,
            usage: Usage::default(),
            turn_running: false,
            compacting: false,
            turn_status: None,
            separator: Separator::new(),
            world,
            pending: Vec::new(),
            current_steer: None,
            submitted: None,
            submitted_queue: None,
            screen_mode,
            pending_ctrl_x: false,
            jump_target: None,
            search: None,
            search_matches: Vec::new(),
            search_index: 0,
            theme_name,
            theme_setting,
            theme_dir,
            theme_names,
            theme_auto,
            theme_picker: None,
            thinking_picker: None,
            model_picker: None,
            grants_picker: None,
            tree_picker: None,
            trust_picker: trust_prompt.then_some(TrustPicker { selected: 0 }),
            shell: None,
            resume_picker: None,
        };
        chat.transcript.set_thinking_visibility(thinking_visibility);
        // FR-UI-7: a resumed transcript renders like the live one - user
        // band, markdown, tool cards - instead of `user:`/`assistant:` lines.
        chat.load_records(&initial_records, attachment_loader.as_ref());
        for line in &initial_tail {
            chat.transcript.push_raw(line.clone());
        }
        chat
    }

    /// Replay persisted records into the transcript with the rendering the
    /// live path used, then fold their usage into the footer totals so a
    /// resumed session reports its own history (FR-UI-7, FR-UI-20).
    pub fn load_records(
        &mut self,
        records: &[Record],
        attachment: Option<&crate::state::LoadAttachment>,
    ) {
        self.transcript.replay_records(records, attachment);
        for record in records {
            if let Record::Assistant {
                usage: Some(usage), ..
            } = record
            {
                self.on_turn_event(TurnEvent::Usage(usage.clone()));
            }
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
                // R5: a tool that read an image shows it through the ladder.
                for image in &result.images {
                    let info =
                        lca_tui::widgets::image::ImageInfo::new(&image.media_type, &image.bytes);
                    self.transcript.push_image(info, image.bytes.clone());
                }
            }
            TurnEvent::ToolOutputChunk { chunk, .. } => {
                self.transcript.append_tool_output(&chunk);
            }
            TurnEvent::Usage(usage) => {
                // FR-UI-20: the live prompt size is this call's input side.
                self.footer.context_used = usage.input + usage.cache_read + usage.cache_write;
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
                delay_ms,
                ..
            } => {
                // The separator counts the backoff down (chrome.md's
                // `RetryStatusIndicator`); the error text stays a notice.
                self.separator.retrying(attempt, max, delay_ms);
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
                // pi clears the indicator on stop; the transcript's error
                // line and the footer's status carry a stop that was not
                // clean (chrome.md has no error kind).
                self.separator.idle();
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
        self.separator.working();
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

    /// Take the queue marker the submitted prompt was handed in, after
    /// [`Self::take_submitted`] (ADR-0038's record marker).
    pub(crate) fn take_submitted_queue(&mut self) -> Option<lca_protocol::SubmitMode> {
        self.submitted_queue.take()
    }

    /// The current model label, resolved from the shared cell.
    pub fn model_label(&self) -> String {
        self.world
            .options
            .model_label
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The models `/model` should offer: the host's live list when one is
    /// wired, else the startup snapshot. Live matters because a login's
    /// model discovery can only run once the endpoint's ad-hoc grant is
    /// approved, after the picker's snapshot was taken.
    pub fn model_ids(&self) -> Vec<String> {
        self.world
            .options
            .hooks
            .models
            .as_ref()
            .map(|list| list())
            .unwrap_or_else(|| self.world.options.models.clone())
    }

    /// The session's thinking level, resolved from the shared cell (R1).
    pub fn thinking_level(&self) -> Option<String> {
        self.world
            .options
            .thinking
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Poll a background `/compact` (the command summarizes on its own
    /// thread, so the interface never freezes for it - the failure this
    /// replaced typed a character and only saw it appear 3.5 s later).
    /// `Running` raises the working state and says so; `Done` posts the
    /// summary or the refusal and rests it (chrome.md's compaction
    /// indicator, in LCA's generic working state).
    ///
    /// Returns `true` when something on screen changed.
    pub fn poll_compact(&mut self) -> bool {
        let Some(poll) = self.world.options.hooks.poll_compact.clone() else {
            return false;
        };
        match poll() {
            crate::state::CompactState::Idle => {
                if self.compacting {
                    self.compacting = false;
                    self.separator.idle();
                    true
                } else {
                    false
                }
            }
            crate::state::CompactState::Running => {
                if self.compacting {
                    false
                } else {
                    self.compacting = true;
                    self.separator.working();
                    self.world.notice = Some("compacting this session…".to_string());
                    true
                }
            }
            crate::state::CompactState::Done(notice) => {
                self.compacting = false;
                self.separator.idle();
                self.world.notice = Some(notice);
                true
            }
        }
    }

    /// Advance the separator's spinner. Returns `true` when the frame
    /// moved, so the loop repaints; idle never wakes the renderer.
    pub fn tick(&mut self) -> bool {
        self.separator.tick()
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

        // A separator above the dock: border dashes, with pi's spinner set
        // into them while work runs (R2). The dashes carry the thinking
        // level, the way pi colors its editor border.
        out.push(String::new());
        let border =
            crate::separator::separator_border(&self.theme, self.thinking_level().as_deref());
        out.push(self.separator.render(width, &self.theme, &border));

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
        // FR-UI-20: the window follows the live model choice (a cell), and
        // the used side is the last call's prompt size.
        footer.context_window = *self
            .world
            .options
            .context_window
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let model = self.model_label();
        footer.model = if model.trim().is_empty() {
            "no model".to_string()
        } else {
            model
        };
        footer.thinking = self.thinking_level();
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
        if self.world.modal_active()
            || self.theme_picker.is_some()
            || self.thinking_picker.is_some()
            || self.model_picker.is_some()
            || self.tree_picker.is_some()
            || self.trust_picker.is_some()
            || self.resume_picker.is_some()
            || self.grants_picker.is_some()
        {
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

        if let Some(action) = self.handle_picker_key(data, key.as_deref()) {
            return action;
        }

        // Escape cancels a running `!`/`!!` command (R4), after the pickers
        // (a picker's Escape closes it instead).
        if key.as_deref() == Some("escape")
            && let Some(shell) = self.shell.as_ref()
        {
            (shell.cancel)();
            self.world.notice = Some(format!("cancelling `{}`", shell.command));
            return Action::Continue;
        }

        // The transcript search owns the keyboard while open (FR-UI-12).
        if self.search.is_some() {
            return self.handle_search_key(data, key.as_deref());
        }
        if self.keybindings.matches(data, "app.tools.expand") {
            self.transcript.toggle_tools_expanded();
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.thinking.toggle") {
            self.transcript.toggle_thinking_expanded();
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.search") {
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
        if self.keybindings.matches(data, "app.editor.external") {
            self.pending_ctrl_x = true;
            return Action::Continue;
        }

        // The extension panel toggle is a host binding, not an extension's.
        if self.keybindings.matches(data, "app.panel.toggle") {
            self.world.panel_open = !self.world.panel_open;
            return Action::Continue;
        }

        match self.editor.handle_key(data) {
            EditorEvent::Submitted(text) => self.on_submit(text),
            EditorEvent::Exit => Action::Exit,
            EditorEvent::Changed | EditorEvent::None => self.global_key(data),
        }
    }

    /// Revoke one grant (S8): the store's own write path for an ad hoc
    /// grant, or the manual path the install-consent group names.
    pub(super) fn revoke_grant(&self, entry: &crate::state::GrantEntry) -> String {
        if !entry.revocable {
            return format!(
                "`{}` is {subject}: the extension's approved set - revoke it with `lca ext disable {subject}` (or re-run /login)",
                entry.detail,
                subject = entry.subject
            );
        }
        match self.world.options.hooks.revoke_grant.as_ref() {
            Some(revoke) => revoke(entry),
            None => "revoking grants is not available in this host".to_string(),
        }
    }

    /// Apply a theme by name and remember it (S5: built-in or custom file).
    pub(super) fn set_theme(&mut self, name: &str) {
        let (theme, notice) =
            crate::theme::load(name, crate::theme::detect_scheme(), &self.theme_dir);
        self.theme = theme;
        // The transcript caches styled lines per entry: a new palette
        // invalidates every one of them, or the bands keep the old colors.
        self.transcript.invalidate();
        self.theme_name = name.to_string();
        self.theme_auto = false;
        self.world.notice = notice;
    }

    /// Apply a terminal color-scheme detection (R10): only while the theme
    /// still follows detection, so an explicit pick wins.
    pub fn apply_detected_scheme(&mut self, scheme: lca_tui::engine::colors::ColorScheme) {
        if !self.theme_auto {
            return;
        }
        let (theme, _) = crate::theme::load(&self.theme_setting, Some(scheme), &self.theme_dir);
        self.theme_name = theme.name.clone();
        self.theme = theme;
        self.transcript.invalidate();
    }

    /// Preview a theme without committing it (FR-UI-17).
    pub(super) fn preview_theme(&mut self, index: usize) {
        if let Some(name) = self.theme_names.get(index).cloned() {
            let (theme, _) =
                crate::theme::load(&name, crate::theme::detect_scheme(), &self.theme_dir);
            self.theme = theme;
            self.transcript.invalidate();
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
                if let Some(text) = printable(data).or_else(|| paste_text(data)) {
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
        if self.keybindings.matches(data, "app.prompt.previous") {
            self.jump_prompt(-1);
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.prompt.next") {
            self.jump_prompt(1);
            return Action::Continue;
        }
        if self.keybindings.matches(data, "app.clear") {
            return if self.turn_running {
                Action::CancelTurn
            } else if self.world.ctrl_c_armed {
                self.world.ctrl_c_armed = false;
                Action::Exit
            } else {
                self.world.ctrl_c_armed = true;
                Action::Continue
            };
        }
        if self.keybindings.matches(data, "app.interrupt") {
            return if self.turn_running {
                Action::CancelTurn
            } else {
                self.world.ctrl_c_armed = false;
                Action::Continue
            };
        }
        if !self.pending.is_empty() && self.keybindings.matches(data, "app.message.dequeue") {
            // Edit-all-queued: return the queue to the editor (ADR-0038).
            self.restore_pending();
            return Action::Continue;
        }
        if self.turn_running && self.keybindings.matches(data, "app.message.followUp") {
            // Queue a follow-up for turn end (ADR-0038).
            let text = self.editor.submit();
            if !text.trim().is_empty() {
                self.queue_submit(text, lca_protocol::SubmitMode::FollowUp);
            }
            return Action::Continue;
        }
        Action::Continue
    }

    /// Handle a key while a modal or the panel owns the keyboard.
    fn handle_modal_key(&mut self, data: &str) -> Option<Action> {
        let key = keys::parse_key(data);
        let key = key.as_deref();
        // R4: while a background login step runs, the waiting state owns
        // the keyboard. Escape cancels; every other key is swallowed so it
        // cannot leak into the editor behind the modal.
        if self.world.login_waiting.is_some() {
            if key == Some("escape") {
                if let Some(cancel) = self.world.options.hooks.cancel_login.clone() {
                    cancel();
                }
                self.world.login_waiting = None;
                self.world.notice = Some("login cancelled".to_string());
            }
            return Some(Action::Continue);
        }
        self.handle_login_picker(key)
            .or_else(|| self.handle_login_secret(data, key))
            .or_else(|| self.handle_login_grant(key))
            .or_else(|| self.handle_permission(key))
            .or_else(|| self.handle_extension_modal(data, key))
            .or_else(|| self.handle_panel(data, key))
    }

    /// The `/login` list picker, while open.
    fn handle_login_picker(&mut self, key: Option<&str>) -> Option<Action> {
        let mut prompt = self.world.picker.take()?;
        match key {
            Some("escape") => self.world.notice = Some("login cancelled".to_string()),
            Some("up" | "k") => {
                prompt.selected = prompt.selected.saturating_sub(1);
                self.world.picker = Some(prompt);
            }
            Some("down" | "j") => {
                prompt.selected = (prompt.selected + 1).min(prompt.options.len().saturating_sub(1));
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
        Some(Action::Continue)
    }

    /// The `/login` masked single-line prompt, while open.
    fn handle_login_secret(&mut self, data: &str, key: Option<&str>) -> Option<Action> {
        let mut prompt = self.world.secret.take()?;
        // R1: paste is a primitive of every text input. A pasted key,
        // base URL, or model id lands in the same buffer the typist
        // fills, so it is masked when the field is masked.
        if let Some(text) = paste_text(data) {
            prompt.input.push_str(&text);
            self.world.secret = Some(prompt);
            return Some(Action::Continue);
        }
        match key {
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
        Some(Action::Continue)
    }

    /// The ad hoc `net` grant confirm, while open.
    fn handle_login_grant(&mut self, key: Option<&str>) -> Option<Action> {
        let prompt = self.world.grant.take()?;
        match key {
            Some("y" | "Y" | "enter") => {
                let message = self.world.options.confirm_login_grant.as_ref().map_or_else(
                    || "nothing was changed".to_string(),
                    |confirm| confirm(&prompt.provider, &prompt.host),
                );
                self.world.notice = Some(crate::state::sanitize_block(&message));
            }
            Some("n" | "N" | "escape") => {
                self.world.notice = Some(format!("kept {} without the ad hoc grant", prompt.host));
            }
            _ => self.world.grant = Some(prompt),
        }
        Some(Action::Continue)
    }

    /// The permission modal, while open.
    fn handle_permission(&mut self, key: Option<&str>) -> Option<Action> {
        use lca_permissions::Decision;
        let crate::state::PermissionModal {
            action,
            respond,
            deadline,
        } = self.world.permission.take()?;
        let _ = deadline;
        let decision = match key {
            Some("o") => Some(Decision::Once),
            Some("a") => Some(Decision::Always),
            // Trust the folder for this session: later in-workspace commands
            // run without a prompt (ADR-0039).
            Some("t") => Some(Decision::TrustFolder),
            Some("d" | "escape" | "enter") => Some(Decision::Denied),
            _ => None,
        };
        match decision {
            Some(decision) => {
                if let Some(respond) = respond {
                    let _ = respond.send(decision);
                }
            }
            None => {
                // Any other key cancels the countdown but keeps the modal -
                // and its responder, so a later answer still reaches the
                // waiting worker (R11).
                self.world.permission = Some(crate::state::PermissionModal {
                    action,
                    respond,
                    deadline: None,
                });
            }
        }
        Some(Action::Continue)
    }

    /// An extension modal, while open.
    fn handle_extension_modal(&mut self, data: &str, key: Option<&str>) -> Option<Action> {
        if !self.world.modal_open {
            return None;
        }
        if key == Some("escape") {
            self.world.modal_open = false;
            return Some(Action::Continue);
        }
        let input = if key == Some("enter") {
            lca_protocol::UiInput::Submit {
                text: self.editor.text(),
            }
        } else {
            crate::state::key_input(key.unwrap_or(data))
        };
        if let Some(interactor) = self.world.options.ui_events.clone()
            && let Some((_, effect)) = interactor("modal", &input)
        {
            return Some(self.apply_effect(effect));
        }
        Some(Action::Continue)
    }

    /// The extension side panel, while open. Returns `None` when the key
    /// belongs to the panel toggle itself.
    fn handle_panel(&mut self, data: &str, key: Option<&str>) -> Option<Action> {
        if !self.world.panel_open {
            return None;
        }
        if key == Some("ctrl+p") {
            return None;
        }
        let input = crate::state::key_input(key.unwrap_or(data));
        if let Some(interactor) = self.world.options.ui_events.clone() {
            if let Some((_, effect)) = interactor("panel", &input) {
                return Some(self.apply_effect(effect));
            }
            self.world.panel_open = false;
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
                self.submitted_queue = None;
                self.submitted = Some(text);
                Action::Submit
            }
        }
    }

    /// Apply the CLI's next login step (the host's [`LoginNext`]).
    pub fn apply_login_next(&mut self, next: LoginNext) {
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
        self.submitted_queue = None;
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
        // The flush keeps the marker the message was queued with: a steer
        // that never reached a boundary is still "submitted while a turn
        // was running", and the record this turn writes has to say so.
        self.submitted_queue = Some(next.mode);
        Some(next.text)
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

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
