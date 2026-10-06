//! The command language and the session-switch path, split from
//! `chat.rs` (the 1,200-line ceiling).

use std::sync::Arc;

use lca_protocol::CommandEffect;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::keys;
use lca_tui::widgets::autocomplete::{
    ArgumentCompletions, AutocompleteItem, CombinedAutocompleteProvider, SlashCommand,
};

use super::chat::Chat;
use crate::chat_pickers::{
    ModelPicker, SettingsPicker, ThemePicker, ThinkingPicker, TreePicker, TrustPicker, thinking_row,
};
use crate::state::{Action, UiOptions};

impl Chat {
    /// Open the theme picker on the current theme (the `/theme` command
    /// and the `/settings` selector's `ui.theme` row both land here).
    pub(super) fn open_theme_picker(&mut self) {
        self.theme_picker = Some(ThemePicker {
            selected: self
                .theme_names
                .iter()
                .position(|name| *name == self.theme_name)
                .unwrap_or(0),
            original: self.theme_name.clone(),
        });
    }

    /// Open the thinking picker on the current level (`/thinking` and
    /// the `/settings` selector's `thinking` row).
    pub(super) fn open_thinking_picker(&mut self) {
        let current = self.thinking_level();
        self.thinking_picker = Some(ThinkingPicker {
            selected: thinking_row(current.as_deref()),
        });
    }

    /// Dispatch a slash command line.
    pub(crate) fn dispatch_command(&mut self, line: &str) -> Action {
        let command_line = line.strip_prefix('/').unwrap_or(line);
        let mut parts = command_line.splitn(2, ' ');
        let name = parts.next().unwrap_or("").to_string();
        let argument = parts.next().unwrap_or("").to_string();
        // gh #43: pi's `/skill:name` colon form routes like the space
        // form (`/skill name`), so both reach the host's `skill` command.
        // Only the `skill` head splits: other commands keep their colons.
        let (name, argument) = match name.split_once(':') {
            Some(("skill", rest)) => {
                let argument = if argument.is_empty() {
                    rest.to_string()
                } else {
                    format!("{rest} {argument}")
                };
                ("skill".to_string(), argument)
            }
            _ => (name, argument),
        };
        // The live list, not the startup snapshot, so a login that
        // discovered models after its grant reaches the picker (pain point
        // #4). Computed once per command; the host hook is a cheap read.
        let live_models = self.model_rows();

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
                        "app-owned screen (fullscreen)"
                    } else {
                        "terminal scrollback"
                    }
                ));
                return Action::Continue;
            }
            "theme" => {
                self.open_theme_picker();
                return Action::Continue;
            }
            "thinking" => {
                self.open_thinking_picker();
                return Action::Continue;
            }
            "trust" => {
                self.trust_picker = Some(TrustPicker { selected: 0 });
                return Action::Continue;
            }
            // gh #30 (EFG-030): the interactive selector, pi's shape on
            // our picker chrome. The guard keeps the host's read-only
            // dump reachable for a host that ships no rows (FR-CFG-2's
            // command half).
            "settings" if self.world.options.hooks.settings_rows.is_some() => {
                let rows = self.settings_rows();
                self.settings_picker = Some(SettingsPicker { rows, selected: 0 });
                return Action::Continue;
            }
            "model" if argument.trim().is_empty() && !live_models.is_empty() => {
                self.model_picker = Some(ModelPicker::new(live_models));
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
            "resume" => {
                let entries = self
                    .world
                    .options
                    .hooks
                    .session_list
                    .as_ref()
                    .map(|list| list())
                    .unwrap_or_default();
                if entries.is_empty() {
                    self.world.notice = Some("no sessions yet".to_string());
                } else {
                    self.resume_picker = Some(crate::resume::ResumePicker::new(entries));
                }
                return Action::Continue;
            }
            "grants" => {
                let Some(list) = self.world.options.hooks.grants.as_ref() else {
                    self.world.notice = Some("grants are not available in this host".to_string());
                    return Action::Continue;
                };
                let entries = list();
                if entries.is_empty() {
                    self.world.notice = Some("no grants recorded for this project".to_string());
                } else {
                    self.grants_picker = Some(crate::chat_pickers::GrantPicker {
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

        // gh #25: only the bare `/login` belongs to the options seam. A
        // namespaced `<provider>.login` is that provider's identity `login`
        // export (FR-PROV-10), which the host's command dispatch below owns -
        // intercepting it here sent every provider through the preset picker,
        // including the ones that have no presets to show.
        if name == "login"
            && let Some(login) = self.world.options.login.clone()
        {
            let next = login(argument.as_str());
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
                    self.world.notice = Some(crate::state::sanitize_block(&text));
                }
                CommandEffect::AttachImage {
                    media_type,
                    bytes,
                    note,
                } => {
                    let info = lca_tui::widgets::image::ImageInfo::new(&media_type, &bytes);
                    self.transcript.push_image(info, bytes);
                    self.world.notice = Some(crate::state::sanitize_block(&note));
                }
                CommandEffect::InsertText(text) => {
                    self.editor.insert_str(&crate::state::sanitize_block(&text));
                }
                CommandEffect::SubmitPrompt(text) => {
                    self.transcript.push_user(text.clone());
                    self.submitted_queue = None;
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

    /// Switch to a session in place when the host supports it (R3), else
    /// print the resume command. Refused while a turn runs: the agent and
    /// its stream must not be swapped mid-flight.
    pub(crate) fn switch_or_announce(&mut self, id: &str) {
        if self.turn_running {
            self.world.notice =
                Some("a turn is running; finish or cancel it before switching".to_string());
            return;
        }
        match self.world.options.hooks.switch_session.as_ref() {
            Some(switch) => match switch(id) {
                Some(records) => {
                    // Replayed with the live rendering (FR-UI-7), not as
                    // plain lines: same transcript machinery as a live turn.
                    self.transcript.clear();
                    self.pending.clear();
                    let loader = self.world.options.hooks.load_attachment.clone();
                    self.load_records(&records, loader.as_ref());
                    self.world.notice = Some(format!("switched to session {id}"));
                }
                None => self.world.notice = Some(format!("cannot open session {id}")),
            },
            None => {
                self.world.notice = Some(format!("resume this branch with: lca --resume {id}"));
            }
        }
    }
}

/// Decode the printable text a key inserts, if any.
pub(crate) fn printable(data: &str) -> Option<String> {
    if let Some(text) = keys::decode_printable_key(data) {
        return Some(text);
    }
    match keys::parse_key(data) {
        Some(key) if key.chars().count() == 1 => Some(key),
        Some(key) if key == "space" => Some(" ".to_string()),
        _ => None,
    }
}

/// The flattened text of a bracketed paste, if `data` is one (R1). Every
/// single-line surface — the masked secret field, the base-URL and
/// model-id fields, the picker search boxes, the transcript search —
/// routes through this, so a paste behaves the same everywhere and a
/// multi-line paste is flattened rather than dropped.
pub(crate) fn paste_text(data: &str) -> Option<String> {
    lca_tui::widgets::paste::bracketed_paste_content(data).map(lca_tui::widgets::paste::flatten)
}

/// Build the autocomplete chain: commands, arguments, and file paths.
pub(crate) fn provider_for(options: &UiOptions) -> CombinedAutocompleteProvider {
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
                            .filter(|(id, _)| id.starts_with(prefix))
                            .map(|(id, label)| AutocompleteItem {
                                // The insert is the raw id; only the menu
                                // row shows the label (G2).
                                value: id.clone(),
                                label: label.clone(),
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
        "/fullscreen" => "toggle terminal scrollback and app-owned screen (fullscreen)",
        "/theme" => "pick a theme with live preview",
        "/thinking" => "set the reasoning level",
        "/tree" => "browse session branches",
        "/resume" => "search and reopen a session",
        "/fork" => "fork a branch at a message (usage: /fork <n>)",
        "/exit" => "leave the interface",
        "/login" => "sign in to a provider",
        "/logout" => "clear the provider's stored key",
        "/usage" => "show the provider's usage, when it has one",
        "/model" => "list or switch the session's model",
        "/compact" => "summarize the session to free context",
        "/attach" => "attach an image to the next message",
        "/trust" => "trust the project folder (auto-approve in-workspace commands)",
        "/permissions" => "manage allow/deny rules (session, project, global)",
        "/grants" => "review this project's grants and rules",
        "/settings" => "show the merged configuration and where each value came from",
        "/session" => "show this session's tokens, cache, and cost",
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
    // R2: the main screen never captures the mouse; `/fullscreen` does, so
    // the terminal's own bypass is worth naming for the fullscreen case.
    lines.push(
        "  Shift+click (or your terminal's bypass key) selects text in fullscreen".to_string(),
    );
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

impl Chat {
    /// The `/settings` rows (gh #30): the host's hook - key, value,
    /// winning source, cycle values - with the three session-owned
    /// values mirrored from where they actually live (the theme, the
    /// thinking level, the transcript's thinking visibility), the same
    /// live-over-file rule `settings_text` had.
    pub(super) fn settings_rows(&self) -> Vec<crate::state::SettingRow> {
        let rows = self
            .world
            .options
            .hooks
            .settings_rows
            .as_ref()
            .map(|rows| rows())
            .unwrap_or_default();
        rows.into_iter()
            .map(|mut row| {
                match row.key.as_str() {
                    "ui.theme" => row.value = self.theme_name.clone(),
                    "thinking" => {
                        row.value = self
                            .thinking_level()
                            .unwrap_or_else(|| "provider default".to_string());
                    }
                    "ui.thinking" => {
                        row.value = match self.transcript.thinking_visibility() {
                            crate::transcript::ThinkingVisibility::Snippet => "snippet",
                            crate::transcript::ThinkingVisibility::Full => "full",
                            crate::transcript::ThinkingVisibility::Hidden => "hidden",
                        }
                        .to_string();
                    }
                    _ => {}
                }
                row
            })
            .collect()
    }

    /// Re-read the `/settings` rows (gh #30): a sub-picker's write
    /// (theme, thinking) lands outside the cycle path, so the selector
    /// restored underneath would otherwise show the value it had before
    /// it stepped aside. Value and source both come from the fresh read.
    pub(super) fn refresh_settings_rows(&mut self) {
        let rows = self.settings_rows();
        if let Some(picker) = self.settings_picker.as_mut() {
            picker.rows = rows;
            picker.selected = picker.selected.min(picker.rows.len().saturating_sub(1));
        }
    }

    /// One `/settings` edit (gh #30): apply it to the running session
    /// where a seam exists, persist it through the one seam
    /// (`persist_setting_at`, QA-015), and return the notice - honest
    /// about what did not happen: `ui.color` and `shell.tool` resolve at
    /// startup, and `ui.fullscreen` lives in `ui.json`, not the config
    /// file (which is also where this writes it).
    pub(super) fn apply_setting(&mut self, key: &str, value: &str) -> String {
        let persisted = Some(value.to_string());
        match key {
            "ui.thinking" => {
                if let Some(visibility) = crate::transcript::ThinkingVisibility::parse(value) {
                    self.transcript.set_thinking_visibility(visibility);
                }
                if let Some(persist) = &self.world.options.hooks.persist_setting {
                    persist(key, persisted);
                }
                format!("ui.thinking = {value}")
            }
            "thinking" => {
                *self
                    .world
                    .options
                    .thinking
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value.to_string());
                if let Some(persist) = &self.world.options.hooks.persist_setting {
                    persist(key, persisted);
                }
                format!("thinking: {value}")
            }
            "ui.fullscreen" => {
                let on = value.parse::<bool>().unwrap_or(false);
                self.screen_mode = on;
                // The runtime choice's own file (FR-UI-21): `ui.json`,
                // written by the same seam `/fullscreen` uses.
                if let Some(persist) = &self.world.options.hooks.persist_screen_mode {
                    persist(on);
                }
                format!(
                    "fullscreen {} (stored in ui.json)",
                    if on { "on" } else { "off" }
                )
            }
            "permissions.mode" => {
                // ADR-0042: the mode is grant-store state as much as
                // config state - the host's persist hook applies it to
                // the store, the footer marker follows here.
                let yolo = value == "yolo";
                self.world.options.yolo = yolo;
                self.footer.yolo = yolo;
                if let Some(persist) = &self.world.options.hooks.persist_setting {
                    persist(key, persisted);
                }
                if yolo {
                    "permissions.mode = yolo - every prompt is auto-approved and \
                     recorded, explicit deny rules still deny"
                        .to_string()
                } else {
                    "permissions.mode = ask - prompts are asked again".to_string()
                }
            }
            "ui.color" | "shell.tool" => {
                if let Some(persist) = &self.world.options.hooks.persist_setting {
                    persist(key, persisted);
                }
                format!("{key} = {value} (applies to the next session)")
            }
            other => {
                if let Some(persist) = &self.world.options.hooks.persist_setting {
                    persist(other, Some(value.to_string()));
                }
                format!("{other} = {value}")
            }
        }
    }

    /// One step on a `/settings` row (gh #30): the next value in the
    /// row's list (wrapping), applied and persisted, then the rows are
    /// re-read so value and source show what actually won.
    pub(super) fn cycle_setting(
        &mut self,
        picker: &mut crate::chat_pickers::SettingsPicker,
        forward: bool,
    ) -> String {
        let Some(row) = picker.rows.get(picker.selected) else {
            return String::new();
        };
        if row.values.is_empty() {
            return format!("{} is edited through its picker", row.key);
        }
        let (key, values, current) = (row.key.clone(), row.values.clone(), row.value.clone());
        let position = values
            .iter()
            .position(|value| *value == current)
            .unwrap_or(0);
        let next = if forward {
            (position + 1) % values.len()
        } else {
            (position + values.len() - 1) % values.len()
        };
        let notice = self.apply_setting(&key, &values[next]);
        picker.rows = self.settings_rows();
        picker.selected = picker.selected.min(picker.rows.len().saturating_sub(1));
        notice
    }
}
