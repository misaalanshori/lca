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
    ModelPicker, ThemePicker, ThinkingPicker, TreePicker, TrustPicker, thinking_row,
};
use crate::state::{Action, UiOptions};

impl Chat {
    /// Dispatch a slash command line.
    pub(crate) fn dispatch_command(&mut self, line: &str) -> Action {
        let command_line = line.strip_prefix('/').unwrap_or(line);
        let mut parts = command_line.splitn(2, ' ');
        let name = parts.next().unwrap_or("").to_string();
        let argument = parts.next().unwrap_or("").to_string();
        // The live list, not the startup snapshot, so a login that
        // discovered models after its grant reaches the picker (pain point
        // #4). Computed once per command; the host hook is a cheap read.
        let live_models = self.model_ids();

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
                    selected: self
                        .theme_names
                        .iter()
                        .position(|name| *name == self.theme_name)
                        .unwrap_or(0),
                    original: self.theme_name.clone(),
                });
                return Action::Continue;
            }
            "thinking" => {
                let current = self.thinking_level();
                self.thinking_picker = Some(ThinkingPicker {
                    selected: thinking_row(current.as_deref()),
                });
                return Action::Continue;
            }
            "trust" => {
                self.trust_picker = Some(TrustPicker { selected: 0 });
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
                Some(lines) => {
                    self.transcript.replace(lines);
                    self.pending.clear();
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
