//! The picker key handling, split from `chat.rs` (the 1,200-line ceiling).
//! Each picker owns the keyboard while open (FR-UI-16/17, R1/R2/R9, S8).

use super::chat::Chat;
use super::chat_commands::{paste_text, printable};
use super::chat_pickers::TRUST_OPTIONS;
use super::state::{Action, TrustChoice};

impl Chat {
    /// Whether a picker owns the keyboard (the modal is `modal_active`'s
    /// own question): the overlay test the viewport draws with, and the
    /// gate every contextual key asks first - gh #9's copy key among
    /// them.
    pub(super) fn picker_open(&self) -> bool {
        self.theme_picker.is_some()
            || self.thinking_picker.is_some()
            || self.model_picker.is_some()
            || self.tree_picker.is_some()
            || self.trust_picker.is_some()
            || self.resume_picker.is_some()
            || self.grants_picker.is_some()
            || self.fork_picker.is_some()
            || self.scoped_models_picker.is_some()
            // Gh #165: the settings selector is a keyboard-owning
            // overlay like the rest (it draws through the same
            // composer), so the click and key gates count it.
            || self.settings_picker.is_some()
    }

    /// Whether this key is pi's message-copy key with the editor owning
    /// the keyboard (gh #9, pi 1.0.0's `app.message.copy`): while a
    /// picker or the transcript search has the keyboard, the key is
    /// theirs, not a copy request. Modals keep the keyboard - except
    /// the OAuth wait (gh #200): a waiting screen is not a question,
    /// and the copy target there is the sign-in URL itself.
    pub fn message_copy_key(&self, data: &str) -> bool {
        if self.picker_open() || self.search.is_some() {
            return false;
        }
        if self.world.modal_active() && self.world.login_waiting.is_none() {
            return false;
        }
        self.keybindings.matches(data, "app.message.copy")
    }

    /// Whether this key is pi's jump-to-bottom with the fullscreen
    /// viewport owning the keyboard (gh #35): `tui.altScreen.bottom`
    /// (End) acts only in fullscreen - main-screen mode keeps End on the
    /// editor, whose scrollback has nothing to jump to - and only when
    /// no modal, picker, or the transcript search has the keyboard,
    /// mirroring pi's `shouldDeferViewportInputToOverlay`.
    pub fn alt_screen_bottom(&self, data: &str) -> bool {
        if !self.screen_mode
            || self.world.modal_active()
            || self.picker_open()
            || self.search.is_some()
        {
            return false;
        }
        self.keybindings.matches(data, "tui.altScreen.bottom")
    }

    /// The text a copy request names when nothing is selected (gh #9,
    /// pi 1.0.0): the sign-in URL on a waiting login screen - what the
    /// person is actually waiting on - else the last assistant message
    /// as plain text (its stored source, escape sequences stripped).
    /// What reaches the clipboard, and with what honesty, is the loop's
    /// ladder in `run`.
    pub fn message_copy_text(&self) -> Option<String> {
        if let Some(label) = self.world.login_waiting.as_deref()
            && let Some(url) = Self::http_url(label)
        {
            return Some(url);
        }
        self.transcript
            .last_assistant_text()
            .map(lca_tui::engine::text::strip_terminal_sequences)
    }

    /// The first `http(s)://` URL in a block of text: the waiting login
    /// screen carries the sign-in URL inside a sentence, and that URL is
    /// what the copy key copies there (gh #9). The stored label passed
    /// through `sanitize_block`, which escapes the real OSC 8 wrapper
    /// (gh #178) into literal `\x1b` / `\x07` text - so a backslash
    /// ends the URL too, else the copy carries wrapper bytes (gh #200).
    /// Raw ESC/BEL end it as well, for labels that never sanitized.
    fn http_url(text: &str) -> Option<String> {
        let start = text.find("https://").or_else(|| text.find("http://"))?;
        let rest = &text[start..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace() || c == '`' || c == '\\' || c == '\u{1b}' || c == '\u{7}'
            })
            .unwrap_or(rest.len());
        let url = rest[..end].trim_end_matches(|c: char| ",.;)]}\"".contains(c));
        (!url.is_empty()).then(|| url.to_string())
    }

    /// The open picker's key handling, if any (returns `None` when no
    /// Page/home/end for pickers (gh #226): the rolling window follows
    /// `selected`, so paging is a longer step. The stride matches the
    /// dialog list's ten rows.
    pub(super) const PICKER_PAGE_STRIDE: usize = 10;

    /// Move `selected` for a paging key, clamped into `0..=max`.
    /// `None` for any other key (the caller's match owns those).
    pub(super) fn page_selected(selected: usize, max: usize, key: &str) -> Option<usize> {
        // The parse layer yields camelCase for the tilde sequences
        // (`pageUp`/`pageDown`) and lowercase elsewhere: accept both.
        match key {
            "pageup" | "pageUp" => Some(selected.saturating_sub(Self::PICKER_PAGE_STRIDE)),
            "pagedown" | "pageDown" => {
                Some(selected.saturating_add(Self::PICKER_PAGE_STRIDE).min(max))
            }
            "home" => Some(0),
            "end" => Some(max),
            _ => None,
        }
    }

    /// picker is open, so the editor path runs).
    pub(super) fn handle_picker_key(&mut self, data: &str, key: Option<&str>) -> Option<Action> {
        // The `/tree` selector owns the keyboard while open (FR-UI-16).
        if let Some(mut picker) = self.tree_picker.take() {
            match key {
                Some("escape") => {}
                Some("enter") => {
                    // gh #37: tree rows are records now, so Enter
                    // branches in place (session switches live on
                    // `/resume`).
                    let (id, _) = picker.entries[picker.selected].clone();
                    self.branch_and_replay(&id);
                }
                Some("up") | Some("k") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.tree_picker = Some(picker);
                }
                Some("down") | Some("j") => {
                    picker.selected = (picker.selected + 1).min(picker.entries.len() - 1);
                    self.tree_picker = Some(picker);
                }
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(
                            picker.selected,
                            picker.entries.len().saturating_sub(1),
                            key,
                        )
                    }) {
                        picker.selected = next;
                    }
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
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(
                            picker.selected,
                            picker.entries.len().saturating_sub(1),
                            key,
                        )
                    }) {
                        picker.selected = next;
                    }
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
                        // The `/settings` selector that stepped aside for
                        // this picker answers with what just landed (gh #30).
                        self.refresh_settings_rows();
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
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(
                            picker.selected,
                            self.theme_names.len().saturating_sub(1),
                            key,
                        )
                    }) {
                        picker.selected = next;
                    }
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
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(picker.selected, TRUST_OPTIONS.len() - 1, key)
                    }) {
                        picker.selected = next;
                    }
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
                        picker.offered.get(picker.selected - 1).cloned()
                    };
                    match &self.world.options.hooks.set_thinking {
                        // gh #8 phase 4: the host clamps the pick to the
                        // current model's allowed set, stores what landed,
                        // and says so - a level this model refuses becomes
                        // its default, never a request that ignores it.
                        Some(set) => {
                            self.world.notice =
                                Some(crate::state::sanitize_block(&set(level.as_deref())));
                        }
                        // A host without the seam behaves as it always did.
                        None => {
                            *self
                                .world
                                .options
                                .thinking
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) = level.clone();
                            // E2: persist the pick to the config file (`unset`
                            // removes the key), so `/settings` and the next
                            // process agree.
                            if let Some(persist) = &self.world.options.hooks.persist_setting {
                                persist("thinking", level.clone());
                            }
                            self.world.notice = Some(match level {
                                Some(level) => format!("thinking: {level}"),
                                None => "thinking: provider default".to_string(),
                            });
                        }
                    }
                    // The `/settings` selector underneath answers with
                    // what just landed (gh #30).
                    self.refresh_settings_rows();
                }
                Some("up") | Some("k") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.thinking_picker = Some(picker);
                }
                Some("down") | Some("j") => {
                    picker.selected = (picker.selected + 1).min(picker.offered.len());
                    self.thinking_picker = Some(picker);
                }
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(picker.selected, picker.offered.len(), key)
                    }) {
                        picker.selected = next;
                    }
                    self.thinking_picker = Some(picker);
                }
                _ => self.thinking_picker = Some(picker),
            }
            return Some(Action::Continue);
        }

        // The `/settings` selector owns the keyboard while open (gh #30):
        // up/down move, Enter/→/← act (a sub-picker row opens its picker -
        // the selector steps aside and comes back when it closes - a
        // free-text row edits inline (gh #174), and anything else cycles
        // the value through the one persist seam), q/Escape close.
        // While a row edits, keystrokes land in its buffer; navigation
        // waits for Enter (apply) or Escape (cancel).
        if let Some(mut picker) = self.settings_picker.take() {
            if picker.editing.is_some() {
                match key {
                    Some("escape") => {
                        picker.editing = None;
                        self.settings_picker = Some(picker);
                    }
                    Some("enter") => {
                        let buffer = picker.editing.take().unwrap_or_default();
                        let row = picker.rows.get(picker.selected).cloned();
                        if let Some(row) = row {
                            // An untouched buffer writes nothing: the file
                            // keeps its comments and mtime.
                            let notice = if buffer.trim() == row.value {
                                format!("{} = {} (unchanged)", row.key, row.value)
                            } else {
                                self.apply_setting(&row.key, buffer.trim())
                            };
                            self.world.notice = Some(crate::state::sanitize_block(&notice));
                        }
                        // Re-read like the cycle path, so value and
                        // source show what actually won.
                        picker.rows = self.settings_rows();
                        picker.selected = picker.selected.min(picker.rows.len().saturating_sub(1));
                        self.settings_picker = Some(picker);
                    }
                    Some("backspace") => {
                        if let Some(buffer) = picker.editing.as_mut() {
                            buffer.pop();
                        }
                        self.settings_picker = Some(picker);
                    }
                    _ => {
                        if let Some(text) = printable(data).or_else(|| paste_text(data))
                            && let Some(buffer) = picker.editing.as_mut()
                        {
                            buffer.push_str(&text);
                        }
                        self.settings_picker = Some(picker);
                    }
                }
                return Some(Action::Continue);
            }
            match key {
                Some("escape") | Some("q") => {}
                Some("up") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.settings_picker = Some(picker);
                }
                Some("down") => {
                    picker.selected =
                        (picker.selected + 1).min(picker.rows.len().saturating_sub(1));
                    self.settings_picker = Some(picker);
                }
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(
                            picker.selected,
                            picker.rows.len().saturating_sub(1),
                            key,
                        )
                    }) {
                        picker.selected = next;
                    }
                    self.settings_picker = Some(picker);
                }
                Some("enter") | Some("right") | Some("left") => {
                    let forward = key != Some("left");
                    // The key is cloned out before the match so the row
                    // borrow ends: one arm re-borrows the picker mutably.
                    let row = picker.rows.get(picker.selected).cloned();
                    match row.as_ref().map(|row| row.key.as_str()) {
                        // A sub-picker row steps aside: the sub-picker's
                        // own key arm sits above this one, so the selector
                        // stays open underneath and is back on screen the
                        // moment it closes (pi's submenu shape - the list
                        // is never lost).
                        Some("ui.theme") => {
                            self.open_theme_picker();
                            self.settings_picker = Some(picker);
                        }
                        Some("thinking") => {
                            self.open_thinking_picker();
                            self.settings_picker = Some(picker);
                        }
                        _ => {
                            // A row with no cycle values and no sub-picker
                            // edits inline (gh #174): the buffer starts
                            // as the stored value.
                            if row.as_ref().is_some_and(|row| row.values.is_empty()) {
                                picker.editing = Some(row.map(|row| row.value).unwrap_or_default());
                                self.settings_picker = Some(picker);
                            } else {
                                let notice = self.cycle_setting(&mut picker, forward);
                                self.world.notice = Some(crate::state::sanitize_block(&notice));
                                self.settings_picker = Some(picker);
                            }
                        }
                    }
                }
                _ => {
                    self.settings_picker = Some(picker);
                }
            }
            return Some(Action::Continue);
        }

        if let Some(action) = self.handle_resume_picker_key(data, key) {
            return Some(action);
        }

        if let Some(action) = self.handle_model_picker_key(data, key) {
            return Some(action);
        }

        if let Some(action) = self.handle_fork_picker_key(data, key) {
            return Some(action);
        }

        if let Some(action) = self.handle_scoped_models_key(data, key) {
            return Some(action);
        }
        None
    }

    /// The `/scoped-models` checklist keys (gh #204): up/down move,
    /// space toggles the row, `a` flips the whole list, enter saves
    /// (an empty save refuses - an empty scope means no restriction),
    /// escape discards.
    pub(super) fn handle_scoped_models_key(
        &mut self,
        _data: &str,
        key: Option<&str>,
    ) -> Option<Action> {
        if let Some(mut picker) = self.scoped_models_picker.take() {
            match key {
                Some("escape") => {}
                Some("up") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.scoped_models_picker = Some(picker);
                }
                Some("down") => {
                    if !picker.rows.is_empty() {
                        picker.selected = (picker.selected + 1).min(picker.rows.len() - 1);
                    }
                    self.scoped_models_picker = Some(picker);
                }
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(
                            picker.selected,
                            picker.rows.len().saturating_sub(1),
                            key,
                        )
                    }) {
                        picker.selected = next;
                    }
                    self.scoped_models_picker = Some(picker);
                }
                Some("space") => {
                    picker.toggle_selected();
                    self.scoped_models_picker = Some(picker);
                }
                Some("a") => {
                    picker.toggle_all();
                    self.scoped_models_picker = Some(picker);
                }
                Some("enter") => {
                    if picker.checked.is_empty() {
                        self.world.notice = Some("keep at least one model in rotation".to_string());
                        self.scoped_models_picker = Some(picker);
                    } else if let Some(save) = self.world.options.hooks.save_scoped_models.clone() {
                        self.world.notice = Some(save(picker.checked.clone()));
                    } else {
                        self.world.notice =
                            Some("scoped models are not available in this host".to_string());
                    }
                }
                _ => {
                    self.scoped_models_picker = Some(picker);
                }
            }
            return Some(Action::Continue);
        }
        None
    }

    /// The `/fork` user-message picker's keys (gh #203): up/down move
    /// (wrapping, pi's shape), enter forks and switches in-process,
    /// escape closes.
    pub(super) fn handle_fork_picker_key(
        &mut self,
        _data: &str,
        key: Option<&str>,
    ) -> Option<Action> {
        if let Some(mut picker) = self.fork_picker.take() {
            match key {
                Some("escape") => {}
                Some("enter") => {
                    let index = picker.selected;
                    self.fork_and_switch(index);
                }
                Some("up") => {
                    picker.selected = picker
                        .selected
                        .checked_sub(1)
                        .unwrap_or_else(|| picker.messages.len().saturating_sub(1));
                    self.fork_picker = Some(picker);
                }
                Some("down") => {
                    if !picker.messages.is_empty() {
                        picker.selected = (picker.selected + 1) % picker.messages.len();
                    }
                    self.fork_picker = Some(picker);
                }
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(
                            picker.selected,
                            picker.messages.len().saturating_sub(1),
                            key,
                        )
                    }) {
                        picker.selected = next;
                    }
                    self.fork_picker = Some(picker);
                }
                _ => {
                    self.fork_picker = Some(picker);
                }
            }
            return Some(Action::Continue);
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
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(
                            picker.selected,
                            picker.matches.len().saturating_sub(1),
                            key,
                        )
                    }) {
                        picker.selected = next;
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
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(
                            picker.selected,
                            picker.matches.len().saturating_sub(1),
                            key,
                        )
                    }) {
                        picker.selected = next;
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

/// Prompt jumps (gh #173), split from `chat.rs` (the 1,200-line ceiling).
impl Chat {
    /// Move the prompt-jump target to the previous/next user message
    /// (FR-UI-11). The scroll is applied by the loop on the next frame.
    pub(crate) fn jump_prompt(&mut self, delta: i32) {
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

    /// The scroll value that top-pins a pending jump target, if any
    /// (FR-UI-11, gh #173): the prompt's first line lands on the
    /// viewport's row 0, its answer starting beneath it. Cleared once
    /// taken.
    pub fn take_jump_scroll(&mut self, width: u16, height: u16) -> Option<u16> {
        let target = self.jump_target.take()?;
        // gh #35: scroll is a transcript coordinate now - the dock below
        // it is not part of either count.
        let total = self.transcript_len(width);
        let window = self.window_height(width, height);
        let scroll = total.saturating_sub(target + window);
        Some(scroll.min(u16::MAX as usize) as u16)
    }
}
