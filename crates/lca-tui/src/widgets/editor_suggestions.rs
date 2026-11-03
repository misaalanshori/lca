//! The editor's autocomplete lifecycle: refresh, move, accept, and the
//! refresh-or-close rule a **caret move** follows (gh #382 gap 2).
//!
//! Split out of `editor.rs` when that file crossed the 1,200-line ceiling, the same
//! reason `editor_rows.rs`, `editor_atomic.rs` and `word.rs` were. The functions are
//! `impl Editor` methods and see the whole struct, so nothing moved out of reach.
//!
//! The single rule this module exists to hold: **a move re-derives the popup; it does
//! not clear it.** `clear_suggestions()` on every arrow press is what made arrowing
//! left from `/model` back into `/mod` shut the completion instead of narrowing it, and
//! it is why a `Tab` there concatenated a stale row onto the partial command.

use super::*;

impl Editor {
    /// Re-derive the popup after a **move**, or close it when there is nothing left to
    /// complete (gh #382 gap 2).
    ///
    /// The defect was `clear_suggestions()` on every horizontal move: arrowing left from
    /// `/model` back into `/mod` shut the completion instead of narrowing it, and a `Tab`
    /// there concatenated the stale row onto the partial command. The rule is pi's
    /// (`moveCursor`'s tail, `editor.ts:1946-1956`): ask the provider again at the new
    /// position and let **"no suggestions"** be what closes the popup - not the fact that
    /// the caret moved.
    ///
    /// `force = false` because a move is not an explicit completion request: a provider
    /// that only answers when asked (a bare `/`, an empty `@`) must not be made to fire
    /// by arrowing over it.
    pub(super) fn refresh_or_close_suggestions(&mut self) {
        if self.suggestions.is_none() {
            // Nothing was open, so a move has nothing to update - and re-querying could
            // *open* a popup the user did not ask for.
            return;
        }
        self.refresh_suggestions(false);
        if self.suggestions.as_ref().is_none_or(|s| s.items.is_empty()) {
            self.clear_suggestions();
        }
    }

    /// Refresh autocomplete suggestions.
    pub fn refresh_suggestions(&mut self, force: bool) {
        let Some(provider) = &self.provider else {
            self.clear_suggestions();
            return;
        };
        let before = self.text_before_cursor();
        self.suggestions = provider.get_suggestions(&before, force);
        self.suggestion_index = 0;
    }

    /// Select a suggestion by absolute index, clamped into range.
    pub fn select_suggestion(&mut self, index: usize) {
        if let Some(s) = &self.suggestions {
            self.suggestion_index = index.min(s.items.len().saturating_sub(1));
        }
    }

    /// Move the suggestion selection.
    pub fn move_suggestion(&mut self, delta: i32) {
        if let Some(s) = &self.suggestions {
            let n = s.items.len() as i32;
            if n == 0 {
                return;
            }
            self.suggestion_index = ((self.suggestion_index as i32 + delta).rem_euclid(n)) as usize;
        }
    }

    /// Accept the current suggestion.
    pub fn accept_suggestion(&mut self) {
        let Some(s) = self.suggestions.clone() else {
            return;
        };
        let Some(item) = s.items.get(self.suggestion_index).cloned() else {
            return;
        };
        // Replace the prefix with the value.
        let prefix_chars = s.prefix.chars().count();
        let start_col = self.cursor_col.saturating_sub(prefix_chars);
        let start = self.char_to_byte(start_col);
        let end = self.char_to_byte(self.cursor_col);
        self.current_mut().replace_range(start..end, "");
        self.cursor_col = start_col;
        self.insert_str(&item.value);
        // Continue completion for directories / trailing-space command names.
        if item.value.ends_with(' ') {
            self.clear_suggestions();
        } else {
            self.refresh_suggestions(false);
        }
    }
}
