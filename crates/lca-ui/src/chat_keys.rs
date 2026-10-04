//! The picker key handling, split from `chat.rs` (the 1,200-line ceiling).
//! Each picker owns the keyboard while open (FR-UI-16/17, R1/R2/R9, S8).

use super::chat::Chat;
use super::chat_commands::{paste_text, printable};
use super::chat_pickers::{THINKING_LEVELS, TRUST_OPTIONS};
use super::state::{Action, TrustChoice};

impl Chat {
    /// The open picker's key handling, if any (returns `None` when no
    /// picker is open, so the editor path runs).
    pub(super) fn handle_picker_key(&mut self, data: &str, key: Option<&str>) -> Option<Action> {
        // The `/tree` selector owns the keyboard while open (FR-UI-16).
        if let Some(mut picker) = self.tree_picker.take() {
            match key {
                Some("escape") => {}
                Some("enter") => {
                    let (id, _) = picker.entries[picker.selected].clone();
                    self.switch_or_announce(&id);
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
            return Some(Action::Continue);
        }

        // The `/grants` view owns the keyboard while open (S8): Up/Down
        // move, Enter revokes a revocable row (or names the manual path for
        // an install-consent one), Esc closes.
        if let Some(mut picker) = self.grants_picker.take() {
            match key {
                Some("escape") => {}
                Some("enter") => {
                    if let Some(entry) = picker.entries.get(picker.selected).cloned() {
                        self.world.notice = Some(self.revoke_grant(&entry));
                        if let Some(list) = self.world.options.hooks.grants.as_ref()
                            && let Ok(entries) =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| list()))
                        {
                            if entries.is_empty() {
                                return Some(Action::Continue);
                            }
                            picker.selected = picker.selected.min(entries.len() - 1);
                            picker.entries = entries;
                        }
                    }
                    self.grants_picker = Some(picker);
                }
                Some("up") | Some("k") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.grants_picker = Some(picker);
                }
                Some("down") | Some("j") => {
                    picker.selected =
                        (picker.selected + 1).min(picker.entries.len().saturating_sub(1));
                    self.grants_picker = Some(picker);
                }
                _ => self.grants_picker = Some(picker),
            }
            return Some(Action::Continue);
        }

        // The `/theme` picker previews live and restores on cancel (FR-UI-17).
        if let Some(mut picker) = self.theme_picker.take() {
            match key {
                Some("escape") => {
                    self.set_theme(&picker.original);
                }
                Some("enter") => {
                    if let Some(name) = self.theme_names.get(picker.selected).cloned() {
                        self.set_theme(&name);
                        // E2: the committed pick persists to the config file,
                        // the same write path `/thinking` uses.
                        if let Some(persist) = &self.world.options.hooks.persist_setting {
                            persist("ui.theme", Some(name));
                        }
                    }
                }
                Some("up") | Some("k") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.preview_theme(picker.selected);
                    self.theme_picker = Some(picker);
                }
                Some("down") | Some("j") => {
                    picker.selected =
                        (picker.selected + 1).min(self.theme_names.len().saturating_sub(1));
                    self.preview_theme(picker.selected);
                    self.theme_picker = Some(picker);
                }
                _ => self.theme_picker = Some(picker),
            }
            return Some(Action::Continue);
        }

        // The `/trust` picker owns the keyboard while open (ADR-0039).
        if let Some(mut picker) = self.trust_picker.take() {
            match key {
                Some("escape") => {}
                Some("enter") => {
                    let choice = match picker.selected {
                        0 => TrustChoice::Persist(true),
                        1 => TrustChoice::Session(true),
                        2 => TrustChoice::Persist(false),
                        _ => TrustChoice::Session(false),
                    };
                    self.world.notice = Some(match &self.world.options.hooks.trust_apply {
                        Some(apply) => apply(choice),
                        None => "trust is not configurable in this host".to_string(),
                    });
                }
                Some("up") | Some("k") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.trust_picker = Some(picker);
                }
                Some("down") | Some("j") => {
                    picker.selected = (picker.selected + 1).min(TRUST_OPTIONS.len() - 1);
                    self.trust_picker = Some(picker);
                }
                _ => self.trust_picker = Some(picker),
            }
            return Some(Action::Continue);
        }

        // The `/thinking` picker owns the keyboard while open (R1).
        if let Some(mut picker) = self.thinking_picker.take() {
            match key {
                Some("escape") => {}
                Some("enter") => {
                    let level = if picker.selected == 0 {
                        None
                    } else {
                        Some(THINKING_LEVELS[picker.selected - 1].0.to_string())
                    };
                    *self
                        .world
                        .options
                        .thinking
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = level.clone();
                    // E2: persist the pick to the config file (`unset` removes
                    // the key), so `/settings` and the next process agree.
                    if let Some(persist) = &self.world.options.hooks.persist_setting {
                        persist("thinking", level.clone());
                    }
                    self.world.notice = Some(match level {
                        Some(level) => format!("thinking: {level}"),
                        None => "thinking: provider default".to_string(),
                    });
                }
                Some("up") | Some("k") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.thinking_picker = Some(picker);
                }
                Some("down") | Some("j") => {
                    picker.selected = (picker.selected + 1).min(THINKING_LEVELS.len());
                    self.thinking_picker = Some(picker);
                }
                _ => self.thinking_picker = Some(picker),
            }
            return Some(Action::Continue);
        }

        if let Some(action) = self.handle_resume_picker_key(data, key) {
            return Some(action);
        }

        if let Some(action) = self.handle_model_picker_key(data, key) {
            return Some(action);
        }
        None
    }

    /// The `/resume` picker's keys (R2): a search box where up/down navigate and printable keys edit the query.
    pub(super) fn handle_resume_picker_key(
        &mut self,
        data: &str,
        key: Option<&str>,
    ) -> Option<Action> {
        // The `/resume` picker owns the keyboard while open (R2): a search
        // box where up/down navigate and printable keys edit the query.
        if let Some(mut picker) = self.resume_picker.take() {
            match key {
                Some("escape") => {}
                Some("enter") => {
                    if let Some(entry) = picker.selected_entry() {
                        let id = entry.id.clone();
                        self.switch_or_announce(&id);
                    }
                }
                Some("up") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.resume_picker = Some(picker);
                }
                Some("down") => {
                    if !picker.matches.is_empty() {
                        picker.selected = (picker.selected + 1).min(picker.matches.len() - 1);
                    }
                    self.resume_picker = Some(picker);
                }
                Some("backspace") => {
                    picker.query.pop();
                    picker.refilter();
                    self.resume_picker = Some(picker);
                }
                _ => {
                    if let Some(text) = printable(data).or_else(|| paste_text(data)) {
                        picker.query.push_str(&text);
                        picker.refilter();
                    }
                    self.resume_picker = Some(picker);
                }
            }
            return Some(Action::Continue);
        }
        None
    }

    /// The `/model` picker's keys (R9): a search box where up/down navigate and printable keys edit the query.
    pub(super) fn handle_model_picker_key(
        &mut self,
        data: &str,
        key: Option<&str>,
    ) -> Option<Action> {
        // The `/model` picker owns the keyboard while open (R9): a search box
        // where up/down navigate and printable keys edit the query.
        if let Some(mut picker) = self.model_picker.take() {
            match key {
                // pi's `app.models.save`: the highlighted row becomes the
                // default every new session resolves (gh #8). The host
                // writes it - the config file is its file - and answers
                // with what to show; the picker stays open, pi's shape.
                Some("ctrl+s") => {
                    if let Some(id) = picker.selected_model().map(str::to_string) {
                        let notice = match &self.world.options.hooks.save_default_model {
                            Some(save) => save(&id),
                            None => {
                                "saving a default model is not available in this host".to_string()
                            }
                        };
                        self.world.notice = Some(crate::state::sanitize_block(&notice));
                    }
                    self.model_picker = Some(picker);
                }
                Some("escape") => {}
                Some("enter") => {
                    if let Some(id) = picker.selected_model().map(str::to_string) {
                        // The row's id, raw: the label decoration never
                        // reaches this path (G2).
                        match (self.world.options.invoke_command)("model", &id) {
                            lca_protocol::CommandEffect::ShowWidget(text) => {
                                self.world.notice = Some(crate::state::sanitize_block(&text));
                            }
                            _ => self.world.notice = Some(format!("model: {id}")),
                        }
                    }
                }
                Some("up") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.model_picker = Some(picker);
                }
                Some("down") => {
                    if !picker.matches.is_empty() {
                        picker.selected = (picker.selected + 1).min(picker.matches.len() - 1);
                    }
                    self.model_picker = Some(picker);
                }
                Some("backspace") => {
                    picker.query.pop();
                    picker.refilter();
                    self.model_picker = Some(picker);
                }
                _ => {
                    if let Some(text) = printable(data).or_else(|| paste_text(data)) {
                        picker.query.push_str(&text);
                        picker.refilter();
                    }
                    self.model_picker = Some(picker);
                }
            }
            return Some(Action::Continue);
        }
        None
    }
}
