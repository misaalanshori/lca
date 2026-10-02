//! The transcript search keys (FR-UI-12), split from `chat.rs` under the
//! workspace's 1,200-line ceiling rule - the same split `chat_pickers` and
//! `chat_shell` already took.

use super::chat::Chat;
use crate::chat::strip_ansi;
use crate::chat_commands::{paste_text, printable};
use crate::state::Action;

impl Chat {
    /// Handle a key while the transcript search is open (FR-UI-12).
    pub(crate) fn handle_search_key(&mut self, data: &str, key: Option<&str>) -> Action {
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
}
