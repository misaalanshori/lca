//! The picker key handling, split from `chat.rs` (the 1,200-line ceiling).
//! Each picker owns the keyboard while open (FR-UI-16/17, R1/R2/R9, S8).

use super::chat::Chat;
use super::chat_commands::{paste_text, printable};
use super::chat_pickers::TRUST_OPTIONS;
use super::state::{Action, TrustChoice};

/// Which way the fullscreen viewport was asked to jump (gh #375 gap 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportScroll {
    /// `tui.altScreen.top` - the transcript's first row.
    Top,
    /// `tui.altScreen.bottom` - the live bottom.
    Bottom,
    /// `tui.altScreen.pageUp` - one page towards the top (gh #375 gap 5).
    PageUp,
    /// `tui.altScreen.pageDown` - one page towards the live bottom.
    PageDown,
}

/// Rows a page scroll overlaps so a reader keeps their place (pi's
/// `PAGE_SCROLL_OVERLAP`).
pub const PAGE_SCROLL_OVERLAP: usize = 4;

/// One page's scroll distance for a window of `window` rows: the whole window
/// less [`PAGE_SCROLL_OVERLAP`], never less than one row - pi's
/// `Math.max(1, viewportHeight - PAGE_SCROLL_OVERLAP)`.
pub fn page_scroll_rows(window: usize) -> u16 {
    window
        .saturating_sub(PAGE_SCROLL_OVERLAP)
        .max(1)
        .min(u16::MAX as usize) as u16
}

/// The scroll a viewport-scroll key asks for, given the current scroll, the
/// content extent and the window height (gh #375 gaps 4 + 5).
///
/// One place decides what each target means, so the four cases cannot drift from
/// pi's arithmetic: `top` is content-less-window, `bottom` is the live end, and a
/// page is [`page_scroll_rows`] clamped to the scrollable range, so repeated
/// presses stop at the edge rather than running past it.
pub fn scroll_target(target: ViewportScroll, current: u16, content: usize, window: usize) -> u16 {
    let max = content.saturating_sub(window).min(u16::MAX as usize) as u16;
    match target {
        ViewportScroll::Bottom => 0,
        ViewportScroll::Top => max,
        ViewportScroll::PageUp => current.saturating_add(page_scroll_rows(window)).min(max),
        ViewportScroll::PageDown => current.saturating_sub(page_scroll_rows(window)),
    }
}

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
            || self.extension_hub.is_some()
            || self.extension_manager.is_some()
            || self.mcp_manager.is_some()
            || self.install_consent.is_some()
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
        // Gh #398 gap 1: the MCP sign-in screen is the *other* place a wrapped
        // authorization URL appears, and over SSH it is the only way to reach
        // the provider. It owns the keyboard like a picker, but the copy key
        // has an unambiguous target there (the URL it is showing), so it is
        // allowed through - the same exception the OAuth wait already gets.
        if self.mcp_sign_in_url().is_some() {
            return self.keybindings.matches(data, "app.message.copy");
        }
        if self.picker_open() || self.search.is_some() {
            return false;
        }
        if self.world.modal_active() && self.world.login_waiting.is_none() {
            return false;
        }
        self.keybindings.matches(data, "app.message.copy")
    }

    /// The authorization URL the MCP sign-in screen is showing, if any
    /// (gh #398 gap 1): the `/mcp` modal's URL is a second wrapped-URL trap
    /// over SSH, and it is the same bytes the login wait copies.
    pub fn mcp_sign_in_url(&self) -> Option<String> {
        let manager = self.mcp_manager.as_ref()?;
        match &manager.screen {
            crate::chat_mcp::McpScreen::SignIn { url: Some(url), .. } if !url.is_empty() => {
                Some(url.clone())
            }
            _ => None,
        }
    }

    /// Whether this key is pi's jump-to-bottom with the fullscreen
    /// viewport owning the keyboard (gh #35): `tui.altScreen.bottom`
    /// (End) acts only in fullscreen - main-screen mode keeps End on the
    /// editor, whose scrollback has nothing to jump to - and only when
    /// no modal, picker, or the transcript search has the keyboard,
    /// mirroring pi's `shouldDeferViewportInputToOverlay`.
    pub fn alt_screen_bottom(&self, data: &str) -> bool {
        self.viewport_owns(data) && self.keybindings.matches(data, "tui.altScreen.bottom")
    }

    /// Whether the fullscreen **viewport** owns this key at all (gh #35).
    ///
    /// One gate for every viewport-scroll action, so a new one cannot be wired
    /// without the fullscreen / modal / picker / search checks. Mirrors pi's
    /// `shouldDeferViewportInputToOverlay`.
    fn viewport_owns(&self, _data: &str) -> bool {
        self.screen_mode
            && !self.world.modal_active()
            && !self.picker_open()
            && self.search.is_none()
    }

    /// The viewport-scroll target this key names, if the viewport owns it.
    ///
    /// Gh #375 gap 4: one gate for every viewport-scroll action, so a new one
    /// cannot be wired without the fullscreen / modal / picker / search checks.
    /// `tui.altScreen.top` was declared but never acted on, which is how `Home`
    /// came to be claimed by two actions whose matchers disagreed.
    ///
    /// Returns `ViewportScroll::Top` for `tui.altScreen.top`,
    /// `ViewportScroll::Bottom` for `tui.altScreen.bottom`, and the paging
    /// variants for `tui.altScreen.pageUp`/`pageDown`. Named without intra-doc
    /// links because the module is private, so a link to a variant is a
    /// `private_intra_doc_links` error under `-D warnings`.
    pub fn viewport_scroll(&self, data: &str) -> Option<ViewportScroll> {
        if !self.viewport_owns(data) {
            return None;
        }
        if self.keybindings.matches(data, "tui.altScreen.top") {
            return Some(ViewportScroll::Top);
        }
        if self.keybindings.matches(data, "tui.altScreen.bottom") {
            return Some(ViewportScroll::Bottom);
        }
        if self.keybindings.matches(data, "tui.altScreen.pageUp") {
            return Some(ViewportScroll::PageUp);
        }
        if self.keybindings.matches(data, "tui.altScreen.pageDown") {
            return Some(ViewportScroll::PageDown);
        }
        None
    }

    /// The text a copy request names when nothing is selected (gh #9,
    /// pi 1.0.0): the sign-in URL on a waiting login screen - what the
    /// person is actually waiting on - else the last assistant message
    /// as plain text (its stored source, escape sequences stripped).
    /// What reaches the clipboard, and with what honesty, is the loop's
    /// ladder in `run`.
    pub fn message_copy_text(&self) -> Option<String> {
        // Gh #398 gap 1: the waiting screen's URL is captured when the step
        // arrives (`apply_login_next`), not re-parsed here. The label is prose
        // with the URL inside it, and the clipboard needs the URL alone.
        if let Some(url) = self.world.auth_url.as_deref() {
            return Some(url.to_string());
        }
        // Gh #398 gap 1: the MCP sign-in screen's URL, before the last
        // assistant message (which is what a reader is *not* trying to copy
        // while a sign-in is waiting).
        if let Some(url) = self.mcp_sign_in_url() {
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
    pub(crate) fn http_url_of(text: &str) -> Option<String> {
        Self::http_url(text)
    }

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
        // The `/tree` navigator owns the keyboard while open (FR-UI-16,
        // gh #231): Up/Down walk, Left/Right fold, `f` cycles filters,
        // `e` edits the bookmark, Enter branches in place.
        if let Some(mut picker) = self.tree_picker.take() {
            // A label edit owns every key until it commits (gh #231).
            if picker.editing.is_some() {
                match key {
                    Some("escape") => {
                        picker.editing = None;
                        self.tree_picker = Some(picker);
                    }
                    Some("enter") => {
                        let name = picker.editing.take().unwrap_or_default();
                        let id = picker.selected_id().unwrap_or_default();
                        self.tree_picker = Some(picker);
                        self.commit_tree_label(&id, name.trim());
                    }
                    Some("backspace") => {
                        if let Some(buffer) = picker.editing.as_mut() {
                            buffer.pop();
                        }
                        self.tree_picker = Some(picker);
                    }
                    _ => {
                        if let Some(text) = printable(data).or_else(|| paste_text(data))
                            && let Some(buffer) = picker.editing.as_mut()
                        {
                            buffer.push_str(&text);
                        }
                        self.tree_picker = Some(picker);
                    }
                }
                return Some(Action::Continue);
            }
            // gh #394: the dual-action branch keys are read through the
            // binding table, because the arrow-modifier dialects
            // (`CSI 1;3D`, `ESC b`) do not all round-trip through
            // `parse_key`. Fold the fork you are on, else walk the segment;
            // plain Left/Right keep their gh #231 meanings.
            let word_left = self.keybindings.matches(data, "tui.editor.cursorWordLeft");
            let word_right = self.keybindings.matches(data, "tui.editor.cursorWordRight");
            if word_left {
                picker.fold_or_up();
                self.tree_picker = Some(picker);
                return Some(Action::Continue);
            }
            if word_right {
                picker.unfold_or_down();
                self.tree_picker = Some(picker);
                return Some(Action::Continue);
            }
            match key {
                Some("escape") => {}
                Some("enter") => {
                    // gh #37: tree rows are records now, so Enter
                    // branches in place (session switches live on
                    // `/resume`).
                    if let Some(id) = picker.selected_id() {
                        self.branch_and_replay(&id);
                    } else {
                        self.tree_picker = Some(picker);
                    }
                }
                Some("up") | Some("k") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.tree_picker = Some(picker);
                }
                Some("down") | Some("j") => {
                    picker.selected =
                        (picker.selected + 1).min(picker.visible_len().saturating_sub(1));
                    self.tree_picker = Some(picker);
                }
                // gh #231: Left folds (or climbs to the parent),
                // Right unfolds (or drops to the first child).
                Some("left") => {
                    let at = picker.selected;
                    if !picker.toggle_fold_at(at) {
                        picker.move_to_parent();
                    }
                    self.tree_picker = Some(picker);
                }
                Some("right") => {
                    if !picker.move_to_first_child() {
                        let at = picker.selected;
                        picker.toggle_fold_at(at);
                    }
                    self.tree_picker = Some(picker);
                }
                // gh #231: `f` cycles the filter forward, wrapping.
                Some("f") => {
                    picker.set_filter(picker.filter.cycle_forward());
                    self.tree_picker = Some(picker);
                }
                // gh #231: `e` edits the selected row's bookmark.
                Some("e") => {
                    let current = picker.visible_label(picker.selected).unwrap_or_default();
                    picker.editing = Some(current);
                    self.tree_picker = Some(picker);
                }
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(
                            picker.selected,
                            picker.visible_len().saturating_sub(1),
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
                        let report = self.revoke_grant(&entry);
                        self.say(report);
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
                    self.say(match &self.world.options.hooks.trust_apply {
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
                // Gh #396: pi's `app.thinking.save` - the highlighted level
                // becomes the default for new sessions, mirroring the `/model`
                // picker's `app.models.save`. The host writes it (the config file
                // is its file) and answers with what to show; the picker stays
                // open, pi's shape.
                Some("ctrl+s") => {
                    let level = if picker.selected == 0 {
                        None
                    } else {
                        picker.offered.get(picker.selected - 1).cloned()
                    };
                    let notice = match &self.world.options.hooks.save_default_thinking {
                        Some(save) => save(level.as_deref()),
                        None => "saving a default thinking level is not available in this host"
                            .to_string(),
                    };
                    self.say(crate::state::sanitize_block(&notice));
                    self.thinking_picker = Some(picker);
                }
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
                            self.say(crate::state::sanitize_block(&set(level.as_deref())));
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
                            self.say(match level {
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
        // Gh #397: a stepped wizard owns the keyboard while it walks, and it
        // paints above the `/settings` list it was opened from, so cancelling
        // puts the reader back on the opener row rather than at the top.
        //
        // Nothing is written here except on the finishing `enter`: every step is
        // a *proposal* until then, which is what makes `esc` at the last step
        // apply nothing (not "the steps you already confirmed").
        if let Some(mut wizard) = self.world.wizard.take() {
            match key {
                Some("escape") => {
                    // Cancel the whole wizard. The opener key puts the cursor
                    // back where it started, so the reader is not dumped.
                    wizard.cancel();
                    let opener = wizard.opener_key.clone();
                    if let Some(picker) = self.settings_picker.as_mut()
                        && let Some(index) = picker.rows.iter().position(|row| row.key == opener)
                    {
                        picker.selected = index;
                    }
                    self.say("wizard cancelled - nothing changed".to_string());
                }
                Some("left") => {
                    wizard.step_back();
                    self.world.wizard = Some(wizard);
                }
                Some("enter") if wizard.current + 1 >= wizard.steps.len() => {
                    // Last step: commit every pending choice through the shared
                    // `persist_setting` hook `/settings` uses (one writer).
                    let commits = wizard.commit();
                    let opener = wizard.opener_key.clone();
                    let mut changed = Vec::new();
                    for (wizard_key, value) in commits {
                        let Some(value) = value else { continue };
                        let notice = self.apply_setting(&wizard_key, &value);
                        changed.push(notice);
                    }
                    // Re-read before the mutable borrow, so the rows reflect
                    // what the writes actually did (the borrow checker is
                    // pointing at the same ordering `/settings` uses).
                    let refreshed = self.settings_rows();
                    if let Some(picker) = self.settings_picker.as_mut() {
                        picker.selected = refreshed
                            .iter()
                            .position(|row| row.key == opener)
                            .unwrap_or(picker.selected);
                        picker.rows = refreshed;
                    }
                    if changed.is_empty() {
                        self.say("wizard finished - nothing changed".to_string());
                    } else {
                        for notice in changed {
                            self.say(crate::state::sanitize_block(&notice));
                        }
                    }
                }
                Some("enter") | Some("right") => {
                    // Advance; the choice was already recorded by up/down.
                    wizard.step_forward();
                    self.world.wizard = Some(wizard);
                }
                Some("up" | "k") | Some("down" | "j") => {
                    // Choose the neighbouring value on the current step.
                    let step = wizard.current_step().cloned();
                    if let Some(step) = step
                        && !step.values.is_empty()
                    {
                        let current = wizard
                            .pending_value(&step.key)
                            .unwrap_or(step.value.as_str())
                            .to_string();
                        let forward = key == Some("down") || key == Some("j");
                        let next = Self::next_cycle_value(&step.values, &current, forward);
                        wizard.choose(&next);
                    }
                    self.world.wizard = Some(wizard);
                }
                _ => {
                    self.world.wizard = Some(wizard);
                }
            }
            return Some(Action::Continue);
        }

        if let Some(mut picker) = self.settings_picker.take() {
            if picker.editing.is_some() {
                match key {
                    Some("escape") => {
                        picker.editing = None;
                        self.settings_picker = Some(picker);
                    }
                    Some("enter") => {
                        let buffer = picker.editing.take().unwrap_or_default();
                        let row = picker.selected_row().cloned();
                        if let Some(row) = row {
                            // An untouched buffer writes nothing: the file
                            // keeps its comments and mtime.
                            let notice = if buffer.trim() == row.value {
                                format!("{} = {} (unchanged)", row.key, row.value)
                            } else {
                                self.apply_setting(&row.key, buffer.trim())
                            };
                            self.say(crate::state::sanitize_block(&notice));
                        }
                        // Re-read like the cycle path, so value and
                        // source show what actually won.
                        picker.rows = self.settings_rows();
                        picker.clamp_selection();
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
            // Gh #272 parity pass: the list filters as you type (pi's
            // `ConfigSelector`). Every navigation step is bounded by the
            // *visible* count, and each selection change maps back to a
            // row index, so a filter that shrinks the list can never leave
            // the cursor on a row that is not on screen.
            let visible_len = picker.visible().len();
            match key {
                Some("escape") if !picker.query.is_empty() => {
                    // Pi's Esc clears the query before it closes the list.
                    picker.query.clear();
                    picker.clamp_selection();
                    self.settings_picker = Some(picker);
                }
                Some("escape") | Some("q") => {}
                Some("up") => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.settings_picker = Some(picker);
                }
                Some("down") => {
                    picker.selected = (picker.selected + 1).min(visible_len.saturating_sub(1));
                    self.settings_picker = Some(picker);
                }
                Some("pageup" | "pageUp" | "pagedown" | "pageDown" | "home" | "end") => {
                    if let Some(next) = key.and_then(|key| {
                        Self::page_selected(picker.selected, visible_len.saturating_sub(1), key)
                    }) {
                        picker.selected = next;
                    }
                    self.settings_picker = Some(picker);
                }
                Some("backspace") if !picker.query.is_empty() => {
                    picker.query.pop();
                    picker.clamp_selection();
                    self.settings_picker = Some(picker);
                }
                // Gh #397: `Tab` cycles the **project override**, the
                // three-state control (`inherit → on → off → inherit`) that a
                // session cycle cannot express. A separate key on purpose: the
                // environment cycle writes the user file and is what `Enter`
                // has always done, while `Tab` states something about *this
                // project* - a different file with different consequences.
                Some("tab") => {
                    let row = picker.selected_row().cloned();
                    if row.as_ref().is_some_and(|row| row.project.is_none()) {
                        self.settings_picker = Some(picker);
                        return Some(Action::Continue);
                    }
                    let notice = self.cycle_project_override(&mut picker, true);
                    self.say(crate::state::sanitize_block(&notice));
                    self.settings_picker = Some(picker);
                }
                Some("enter") | Some("right") | Some("left") => {
                    let forward = key != Some("left");
                    // The key is cloned out before the match so the row
                    // borrow ends: one arm re-borrows the picker mutably.
                    let row = picker.selected_row().cloned();
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
                            // Gh #397: the thinking row opens the **stepped
                            // wizard** - the model, then its thinking level -
                            // rather than the flat level picker. This is the
                            // issue's own example of a group a flat list cannot
                            // express, and it is why the wizard exists at all: a
                            // step two that depends on step one, with nothing
                            // written until both are confirmed.
                            self.open_thinking_wizard();
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
                                self.say(crate::state::sanitize_block(&notice));
                                self.settings_picker = Some(picker);
                            }
                        }
                    }
                }
                _ => {
                    // A printable keystroke is a search term, not a dead
                    // key (gh #272 parity pass). Rows still need Enter to
                    // change, so typing to filter can never edit a value.
                    if let Some(text) = printable(data).or_else(|| paste_text(data)) {
                        picker.query.push_str(&text);
                        picker.selected = 0;
                        picker.clamp_selection();
                    }
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

        if let Some(action) = self.handle_install_consent_key(data, key) {
            return Some(action);
        }

        if let Some(action) = self.handle_extension_hub_key(data, key) {
            return Some(action);
        }

        if let Some(action) = self.handle_extension_manager_key(data, key) {
            return Some(action);
        }

        if let Some(action) = self.handle_mcp_manager_key(data, key) {
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
                        self.say("keep at least one model in rotation".to_string());
                        self.scoped_models_picker = Some(picker);
                    } else if let Some(save) = self.world.options.hooks.save_scoped_models.clone() {
                        // gh #233: the rotation scope shapes the offered set.
                        self.invalidate_models();
                        self.say(save(picker.checked.clone()));
                    } else {
                        self.say("scoped models are not available in this host".to_string());
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
            // A delete confirm owns every key until it resolves (gh #75).
            if picker.confirming.is_some() {
                match key {
                    Some("escape") | Some("n") => {
                        picker.confirming = None;
                        self.resume_picker = Some(picker);
                    }
                    Some("y") => {
                        let at = picker.confirming.take();
                        self.resume_picker = Some(picker);
                        self.commit_resume_delete(at);
                    }
                    _ => {
                        self.resume_picker = Some(picker);
                    }
                }
                return Some(Action::Continue);
            }
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
                // gh #75: Ctrl+D trashes (with a confirm); the live
                // session refuses - trash it and the transcript loses
                // its floor.
                Some("ctrl+d") => {
                    if self.world.options.hooks.delete_session.is_none() {
                        self.say("deleting sessions is not available in this host".to_string());
                        self.resume_picker = Some(picker);
                        return Some(Action::Continue);
                    }
                    let current = self
                        .world
                        .options
                        .hooks
                        .current_session_id
                        .as_ref()
                        .map(|current| current());
                    let target = picker
                        .matches
                        .get(picker.selected)
                        .and_then(|index| picker.entries.get(*index));
                    match target {
                        Some(entry) if current.as_deref() == Some(entry.id.as_str()) => {
                            self.say("that session is open here - switch away first".to_string());
                            self.resume_picker = Some(picker);
                        }
                        Some(_) => {
                            picker.confirming = Some(picker.selected);
                            self.resume_picker = Some(picker);
                        }
                        None => {
                            self.resume_picker = Some(picker);
                        }
                    }
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
                        self.say(crate::state::sanitize_block(&notice));
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
                                self.say(crate::state::sanitize_block(&text));
                            }
                            _ => self.say(format!("model: {id}")),
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
        let scroll = total.saturating_sub(target.saturating_add(window));
        Some(scroll.min(u16::MAX as usize) as u16)
    }
}
