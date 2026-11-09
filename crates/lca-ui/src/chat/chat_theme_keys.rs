//! The theme and keybinding group: the palette in force, the setting that
//! selected it, and the one write path for the key map.
//!
//! Split from `chat.rs` at the 1,200-line ceiling (gh #574). This is a seam
//! the code already had: every method below either reads or replaces a theme
//! fact (`theme_setting`, `theme_auto`, `terminal_bg`, the palette itself) or
//! replaces the keybinding registry **together with** the labels that name its
//! keys - the gh #332/#410 invariant that keeps one fact from having two
//! owners. The theme picker's keys live in `chat_keys.rs`; this is the state
//! those keys act on.

use std::sync::Arc;

use lca_tui::engine::keybindings::KeybindingsManager;

use super::Chat;

impl Chat {
    /// The `ui.theme` setting in force, for `/reload` (gh #387 gap 5): it must
    /// re-apply what is active, not the picker default.
    pub(crate) fn theme_setting(&self) -> &str {
        &self.theme_setting
    }

    /// Whether the theme follows terminal detection: a `system` theme is rebuilt
    /// from the OSC 11 answer, so re-applying it by name on reload would drop a
    /// background the terminal already reported.
    pub(crate) fn theme_follows_detection(&self) -> bool {
        self.theme_auto
    }

    /// Install a new keybinding manager and keep every hint in step (gh #410).
    ///
    /// Assigning `self.keybindings` alone rebinds what the interface *accepts*
    /// while leaving the labels on the manager they were rendered with, which is
    /// the same lie in the opposite direction. The transcript is told too, and it
    /// drops its cached frames because a hint's text is part of one.
    pub fn set_keybindings(&mut self, keybindings: Arc<KeybindingsManager>) {
        self.transcript.set_keybindings(keybindings.clone());
        self.editor.set_keybindings(keybindings.clone());
        self.keybindings = keybindings;
    }

    /// The key map the interface is accepting keys with.
    ///
    /// The reader half of [`Self::set_keybindings`]: what `/hotkeys` prints and what
    /// the editor does both come from this one object, which is the promise gh #332
    /// broke when the field was public and could be replaced alone.
    pub fn keybindings_manager(&self) -> &Arc<KeybindingsManager> {
        &self.keybindings
    }

    /// Apply a theme by name and remember it (S5: built-in or custom file).
    /// Picking `system` re-arms detection-following (gh #10); any other
    /// explicit pick clears it.
    pub(crate) fn set_theme(&mut self, name: &str) {
        let (theme, notice) =
            crate::theme::load_all(name, crate::theme::detect_scheme(), &self.theme_extra_dirs);
        self.theme = theme;
        // The transcript caches styled lines per entry: a new palette
        // invalidates every one of them, or the bands keep the old colors.
        self.transcript.invalidate();
        self.theme_name = name.to_string();
        // gh #10: a `system` pick follows detection from here on, so a
        // later OSC 11 / DSR answer rebuilds it; the setting travels
        // with the name for the same reason.
        self.theme_setting = name.to_string();
        self.theme_auto = name == "system";
        if let Some(notice) = notice {
            self.say(notice);
        }
    }

    /// Apply a terminal color-scheme detection (R10): only while the theme
    /// still follows detection, so an explicit pick wins. A `system`
    /// theme rebuilds from the last reported background (gh #10), so a
    /// scheme-only answer never drops a known background.
    pub fn apply_detected_scheme(&mut self, scheme: lca_tui::engine::colors::ColorScheme) {
        if !self.theme_auto || !self.theme_query.accepts_reply() {
            return;
        }
        let (theme, _) = if self.theme_setting == "system" {
            (
                crate::theme::Theme::system_theme(self.terminal_bg, Some(scheme)),
                None,
            )
        } else {
            crate::theme::load_all(&self.theme_setting, Some(scheme), &self.theme_extra_dirs)
        };
        self.theme_name = theme.name.clone();
        self.theme = theme;
        self.transcript.invalidate();
    }

    /// Whether the startup colour query is still accepting replies (gh #379 gap 1).
    pub fn theme_query_open(&self) -> bool {
        self.theme_query.is_open()
    }

    /// Close the startup colour query on the `DA1` sentinel (gh #379 gap 1).
    pub fn close_theme_query(&mut self) {
        self.theme_query.close();
    }

    /// Apply a terminal background answer (gh #10): the OSC 11 reply's
    /// own color. A `system` theme rebuilds from it and repaints;
    /// anything else takes the reply's scheme through the existing
    /// detection path.
    pub fn apply_terminal_background(&mut self, background: lca_tui::engine::colors::RgbColor) {
        if !self.theme_query.accepts_reply() {
            return;
        }
        self.terminal_bg = Some(background);
        // Gh #387: `theme_auto` covers `""`/`auto`/`system`; testing the literal
        // `"system"` meant the default theme never rebuilt from a probed background.
        if self.theme_auto {
            self.theme = crate::theme::Theme::system_theme(Some(background), None);
            self.theme_name = self.theme.name.clone();
            self.transcript.invalidate();
        } else {
            self.apply_detected_scheme(background.scheme());
        }
    }

    /// Preview a theme without committing it (FR-UI-17).
    pub(crate) fn preview_theme(&mut self, index: usize) {
        if let Some(name) = self.theme_names.get(index).cloned() {
            let (theme, _) = crate::theme::load_all(
                &name,
                crate::theme::detect_scheme(),
                &self.theme_extra_dirs,
            );
            self.theme = theme;
            self.transcript.invalidate();
        }
    }
}
