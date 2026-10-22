//! Modal and panel key handling: while a modal or the panel owns
//! the keyboard, keys route here instead of the editor. Split from
//! `chat.rs` for the workspace file ceiling (gate 11). Behaviour
//! unchanged.

use super::chat::Chat;
use crate::chat_commands::{paste_text, printable};
use crate::state::Action;
use lca_tui::engine::keys;

impl Chat {
    /// Handle a key while a modal or the panel owns the keyboard.
    pub(crate) fn handle_modal_key(&mut self, data: &str) -> Option<Action> {
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
                self.say("login cancelled".to_string());
            }
            return Some(Action::Continue);
        }
        // A host-rendered dialog owns the keyboard outright (gh #124):
        // nothing behind it hears a key while it is open.
        if self.world.dialog.is_some() {
            return crate::dialogs::handle_dialog(self, data, key);
        }
        // The panel toggle is a host binding (gh #172): it works while
        // the panel is open too, or the open panel would eat the very
        // key that closes it and trap `/exit` behind it.
        if self.keybindings.matches(data, "app.panel.toggle") {
            self.world.panel_open = !self.world.panel_open;
            return Some(Action::Continue);
        }
        self.handle_login_picker(key)
            .or_else(|| self.handle_login_secret(data, key))
            .or_else(|| self.handle_login_grant(key))
            .or_else(|| self.handle_switch_confirm(key))
            .or_else(|| self.handle_bug_review(key))
            .or_else(|| self.handle_permission(key))
            .or_else(|| self.handle_extension_modal(data, key))
            .or_else(|| self.handle_panel(data, key))
    }

    /// The `/login` list picker, while open.
    fn handle_login_picker(&mut self, key: Option<&str>) -> Option<Action> {
        let mut prompt = self.world.picker.take()?;
        match key {
            Some("escape") => self.say("login cancelled".to_string()),
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
            Some("escape") => self.say("login cancelled".to_string()),
            Some("enter") => {
                let typed = std::mem::take(&mut prompt.input);
                if typed.is_empty() {
                    self.say("nothing was entered".to_string());
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
    pub(super) fn apply_effect(&mut self, effect: lca_protocol::UiEffect) -> Action {
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
                self.say(crate::state::sanitize_block(&text));
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
}
