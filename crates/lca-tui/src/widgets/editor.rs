//! The prompt editor, ported from pi's
//! `packages/tui/src/components/editor.ts`
//! (`pi-tui-re/src_re/tui-widgets/editor.md`).
//!
//! The prize of the port: multi-line editing with readline-style motion,
//! a kill ring, undo, history with drafts, bracketed-paste markers, and the
//! autocomplete popup. Owner issue #7 lives here.
//!
//! Not ported (documented skips): the full 7-case sticky-column decision
//! table (the simple preferred-column version below covers the felt
//! behavior, R7) and the async/debounced autocomplete machinery. Sticky
//! columns and jump mode are ported (R7).

use std::sync::Arc;

use crate::engine::core::CURSOR_MARKER;
use crate::engine::keybindings::KeybindingsManager;
use crate::engine::text::{truncate_to_width, visible_width, wrap_text_with_ansi};
use crate::widgets::autocomplete::{AutocompleteProvider, Suggestions};

/// What a key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorEvent {
    /// Nothing changed.
    None,
    /// The buffer changed.
    Changed,
    /// The user submitted (Enter).
    Submitted(String),
    /// The user asked to exit (Ctrl+C on empty input).
    Exit,
}

/// A multi-line prompt editor.
pub struct Editor {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize, // in chars
    history: Vec<String>,
    history_index: Option<usize>,
    draft: String,
    undo: Vec<(Vec<String>, usize, usize)>,
    kill_ring: Vec<String>,
    pastes: Vec<String>,
    keybindings: Arc<KeybindingsManager>,
    provider: Option<Arc<dyn AutocompleteProvider>>,
    suggestions: Option<Suggestions>,
    suggestion_index: usize,
    /// The column a vertical move wants to land on, kept across shorter
    /// The previous key was a word-character insert, so the next one (if
    /// also a word character) continues the same undo unit (pi's fish-style
    /// coalescing, `editor.md` §1). Reset by every other key.
    coalesce_undo: bool,
    /// The previous key was a kill, so this one appends (kill forward) or
    /// prepends (kill backward) to the same ring entry instead of pushing a
    /// new one (pi's `accumulate`, `editor.md` §5).
    last_kill: bool,
    /// One-shot input to the kill helpers: whether this key continues a
    /// kill run (captured before `last_kill` is cleared for the next key).
    kill_run: bool,
    /// lines (pi's sticky column, `editor.md` §4; R7). Cleared by any
    /// horizontal move or edit.
    preferred_col: Option<usize>,
    /// Jump mode (R7): `Some(forward)` awaits the next printable character
    /// as the jump target (`ctrl+]` / `ctrl+alt+]`).
    jump_pending: Option<bool>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    /// A new editor.
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            history: Vec::new(),
            history_index: None,
            draft: String::new(),
            undo: Vec::new(),
            kill_ring: Vec::new(),
            pastes: Vec::new(),
            keybindings: Arc::new(KeybindingsManager::new()),
            provider: None,
            suggestions: None,
            suggestion_index: 0,
            coalesce_undo: false,
            last_kill: false,
            kill_run: false,
            preferred_col: None,
            jump_pending: None,
        }
    }

    /// Use a specific keybinding set.
    pub fn set_keybindings(&mut self, kb: Arc<KeybindingsManager>) {
        self.keybindings = kb;
    }

    /// Install an autocomplete provider.
    pub fn set_autocomplete(&mut self, provider: Arc<dyn AutocompleteProvider>) {
        self.provider = Some(provider);
    }

    /// The current buffer lines.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The buffer as one string.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Replace the buffer.
    pub fn set_text(&mut self, text: &str) {
        self.preferred_col = None;
        self.lines = text.split('\n').map(|s| s.to_string()).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_line].chars().count();
        self.clear_suggestions();
    }

    /// Clear the buffer and push the submitted text to history.
    pub fn submit(&mut self) -> String {
        let text = self.text();
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.history_index = None;
        self.clear_suggestions();
        text
    }

    /// The active suggestions, if any.
    pub fn suggestions(&self) -> Option<&Suggestions> {
        self.suggestions.as_ref()
    }

    /// The selected suggestion index.
    pub fn suggestion_index(&self) -> usize {
        self.suggestion_index
    }

    fn clear_suggestions(&mut self) {
        self.suggestions = None;
        self.suggestion_index = 0;
    }

    /// Record one kill, accumulating into the previous entry when the
    /// previous key was also a kill (pi's `accumulate`).
    fn kill(&mut self, text: String, prepend: bool) {
        // `kill_run` carries the key handler's captured state; `last_kill`
        // covers a direct API call (a test, or a future non-key caller).
        if (self.kill_run || self.last_kill)
            && let Some(last) = self.kill_ring.last_mut()
        {
            if prepend {
                last.insert_str(0, &text);
            } else {
                last.push_str(&text);
            }
        } else {
            self.kill_ring.push(text);
        }
        self.last_kill = true;
    }

    fn snapshot(&mut self) {
        self.undo
            .push((self.lines.clone(), self.cursor_line, self.cursor_col));
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
    }

    fn current(&self) -> &String {
        &self.lines[self.cursor_line]
    }

    fn current_mut(&mut self) -> &mut String {
        &mut self.lines[self.cursor_line]
    }

    /// Insert text atomically: one undo unit, whatever it spans. Paste,
    /// programmatic insertion, and completion all take this path.
    pub fn insert_str(&mut self, text: &str) {
        self.coalesce_undo = false;
        self.snapshot();
        self.insert_at_cursor(text);
    }

    /// Insert one typed character, coalescing with the previous word
    /// character into a single undo unit (pi's fish-style coalescing).
    fn insert_typed(&mut self, text: &str, continues: bool) {
        let word = is_undo_word_char(text);
        if !(continues && word) {
            self.snapshot();
        }
        self.coalesce_undo = word;
        self.insert_at_cursor(text);
    }

    fn insert_at_cursor(&mut self, text: &str) {
        self.preferred_col = None;
        for (i, part) in text.split('\n').enumerate() {
            if i > 0 {
                self.newline_no_snapshot();
            }
            let byte = self.char_to_byte(self.cursor_col);
            self.current_mut().insert_str(byte, part);
            self.cursor_col += part.chars().count();
        }
        self.refresh_suggestions(false);
    }

    fn char_to_byte(&self, col: usize) -> usize {
        self.current()
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(self.current().len())
    }

    fn newline_no_snapshot(&mut self) {
        let byte = self.char_to_byte(self.cursor_col);
        let rest = self.current()[byte..].to_string();
        self.current_mut().truncate(byte);
        self.lines.insert(self.cursor_line + 1, rest);
        self.cursor_line += 1;
        self.cursor_col = 0;
    }

    /// Whether the character before the cursor is a backslash, the
    /// terminal-without-Shift+Enter workaround's trigger.
    fn cursor_preceded_by_backslash(&self) -> bool {
        self.cursor_col > 0 && self.current().chars().nth(self.cursor_col - 1) == Some('\\')
    }

    /// Insert a newline.
    pub fn newline(&mut self) {
        self.preferred_col = None;
        self.snapshot();
        self.newline_no_snapshot();
        self.clear_suggestions();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        self.preferred_col = None;
        self.snapshot();
        if self.cursor_col > 0 {
            let end = self.char_to_byte(self.cursor_col);
            let start = self.char_to_byte(self.cursor_col - 1);
            self.current_mut().replace_range(start..end, "");
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
            self.current_mut().push_str(&current);
        }
        self.refresh_suggestions(false);
    }

    /// Delete the character after the cursor.
    pub fn delete_forward(&mut self) {
        self.preferred_col = None;
        self.snapshot();
        let len = self.current().chars().count();
        if self.cursor_col < len {
            let start = self.char_to_byte(self.cursor_col);
            let end = self.char_to_byte(self.cursor_col + 1);
            self.current_mut().replace_range(start..end, "");
        } else if self.cursor_line + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_line + 1);
            self.current_mut().push_str(&next);
        }
        self.refresh_suggestions(false);
    }

    /// Delete the word before the cursor.
    pub fn delete_word_backward(&mut self) {
        self.preferred_col = None;
        self.snapshot();
        let chars: Vec<char> = self.current().chars().collect();
        let mut col = self.cursor_col;
        while col > 0 && chars[col - 1].is_whitespace() {
            col -= 1;
        }
        while col > 0 && !chars[col - 1].is_whitespace() {
            col -= 1;
        }
        let start = self.char_to_byte(col);
        let end = self.char_to_byte(self.cursor_col);
        let killed = self.current()[start..end].to_string();
        self.current_mut().replace_range(start..end, "");
        self.cursor_col = col;
        self.kill(killed, true); // backward kill prepends
        self.refresh_suggestions(false);
    }

    /// Delete from the cursor to the end of the line (kill).
    pub fn delete_to_line_end(&mut self) {
        self.preferred_col = None;
        self.snapshot();
        let byte = self.char_to_byte(self.cursor_col);
        let killed = self.current()[byte..].to_string();
        self.current_mut().truncate(byte);
        self.kill(killed, false); // forward kill appends
    }

    /// Delete from the line start to the cursor (kill).
    pub fn delete_to_line_start(&mut self) {
        self.preferred_col = None;
        self.snapshot();
        let byte = self.char_to_byte(self.cursor_col);
        let killed = self.current()[..byte].to_string();
        let rest = self.current()[byte..].to_string();
        *self.current_mut() = rest;
        self.cursor_col = 0;
        self.kill(killed, true);
    }

    /// Yank the most recent kill.
    pub fn yank(&mut self) {
        self.preferred_col = None;
        if let Some(k) = self.kill_ring.last().cloned() {
            self.insert_str(&k);
        }
    }

    /// Undo the last change.
    pub fn undo(&mut self) {
        self.preferred_col = None;
        if let Some((lines, line, col)) = self.undo.pop() {
            self.lines = lines;
            self.cursor_line = line.min(self.lines.len() - 1);
            self.cursor_col = col;
        }
    }

    /// Move the cursor left.
    pub fn cursor_left(&mut self) {
        self.preferred_col = None;
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.current().chars().count();
        }
        self.clear_suggestions();
    }

    /// Move the cursor right.
    pub fn cursor_right(&mut self) {
        self.preferred_col = None;
        if self.cursor_col < self.current().chars().count() {
            self.cursor_col += 1;
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
        self.clear_suggestions();
    }

    /// Move the cursor up (or through history at the top), keeping the
    /// preferred column across shorter lines (pi's sticky column, R7).
    pub fn cursor_up(&mut self) {
        if self.cursor_line > 0 {
            let preferred = *self.preferred_col.get_or_insert(self.cursor_col);
            self.cursor_line -= 1;
            self.cursor_col = preferred.min(self.current().chars().count());
        } else if !self.history.is_empty() {
            if self.history_index.is_none() {
                self.draft = self.text();
                self.history_index = Some(self.history.len());
            }
            if let Some(idx) = self.history_index
                && idx > 0
            {
                self.history_index = Some(idx - 1);
                self.set_text(&self.history[idx - 1].clone());
            }
        }
        self.clear_suggestions();
    }

    /// Move the cursor down (or through history), keeping the preferred
    /// column across shorter lines (pi's sticky column, R7).
    pub fn cursor_down(&mut self) {
        if self.cursor_line + 1 < self.lines.len() {
            let preferred = *self.preferred_col.get_or_insert(self.cursor_col);
            self.cursor_line += 1;
            self.cursor_col = preferred.min(self.current().chars().count());
        } else if let Some(idx) = self.history_index {
            if idx + 1 < self.history.len() {
                self.history_index = Some(idx + 1);
                self.set_text(&self.history[idx + 1].clone());
            } else {
                self.history_index = None;
                let draft = self.draft.clone();
                self.set_text(&draft);
            }
        }
        self.clear_suggestions();
    }

    /// Move the cursor to the line start.
    pub fn cursor_line_start(&mut self) {
        self.preferred_col = None;
        self.cursor_col = 0;
    }

    /// Move the cursor to the line end.
    pub fn cursor_line_end(&mut self) {
        self.preferred_col = None;
        self.cursor_col = self.current().chars().count();
    }

    /// Move the cursor one word left.
    pub fn cursor_word_left(&mut self) {
        self.preferred_col = None;
        let chars: Vec<char> = self.current().chars().collect();
        let mut col = self.cursor_col;
        while col > 0 && chars[col - 1].is_whitespace() {
            col -= 1;
        }
        while col > 0 && !chars[col - 1].is_whitespace() {
            col -= 1;
        }
        self.cursor_col = col;
    }

    /// Move the cursor one word right.
    pub fn cursor_word_right(&mut self) {
        self.preferred_col = None;
        let chars: Vec<char> = self.current().chars().collect();
        let mut col = self.cursor_col;
        while col < chars.len() && chars[col].is_whitespace() {
            col += 1;
        }
        while col < chars.len() && !chars[col].is_whitespace() {
            col += 1;
        }
        self.cursor_col = col;
    }

    fn text_before_cursor(&self) -> String {
        let byte = self.char_to_byte(self.cursor_col);
        self.current()[..byte].to_string()
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

    /// Handle a raw key sequence. Returns what happened.
    pub fn handle_key(&mut self, data: &str) -> EditorEvent {
        // Every key ends the previous coalescing run; the typed-insert path
        // re-arms it (pi coalesces consecutive word characters).
        let continues_undo = self.coalesce_undo;
        self.coalesce_undo = false;
        // A kill run is the previous key's state; the kill helpers consume
        // it through `kill_run` and re-arm `last_kill` for the next key.
        self.kill_run = self.last_kill;
        self.last_kill = false;
        let kb = self.keybindings.clone();

        // Bracketed paste.
        if let Some(rest) = data.strip_prefix("\x1b[200~")
            && let Some(content) = rest.strip_suffix("\x1b[201~")
        {
            self.insert_paste(content);
            return EditorEvent::Changed;
        }

        // Jump mode (R7): the next printable character is the target.
        if let Some(forward) = self.jump_pending {
            self.jump_pending = None;
            if let Some(ch) = printable_char(data) {
                self.jump_to_char(ch, forward);
            }
            return EditorEvent::Changed;
        }

        // Autocomplete popup navigation takes priority when open (pi's
        // editor.ts): Escape cancels, Tab applies and closes, Enter applies
        // and - for a slash-command prefix - falls through to submit.
        if self.suggestions.is_some() {
            if kb.matches(data, "tui.select.cancel") {
                self.clear_suggestions();
                return EditorEvent::Changed;
            }
            if kb.matches(data, "tui.select.up") || kb.matches(data, "tui.editor.cursorUp") {
                self.move_suggestion(-1);
                return EditorEvent::Changed;
            }
            if kb.matches(data, "tui.select.down") || kb.matches(data, "tui.editor.cursorDown") {
                self.move_suggestion(1);
                return EditorEvent::Changed;
            }
            if kb.matches(data, "tui.input.tab") {
                self.accept_suggestion();
                self.clear_suggestions();
                return EditorEvent::Changed;
            }
            if kb.matches(data, "tui.select.confirm") {
                let slash = self
                    .suggestions
                    .as_ref()
                    .is_some_and(|s| s.prefix.starts_with('/'));
                let before = self.text();
                self.accept_suggestion();
                self.clear_suggestions();
                // A slash command falls through to submit (pi's behavior),
                // and so does an accept that changed nothing (the token was
                // already complete): otherwise Enter would appear dead.
                if !slash && self.text() != before {
                    return EditorEvent::Changed;
                }
            }
        }

        // Newline, checked BEFORE submit (pi's order, `editor.ts`): a bare
        // LF is Ctrl+J in every dialect, and a terminal that cannot report
        // Shift+Enter still sends one of these spellings. pi's condition
        // set is reproduced literally, including the raw sequences its
        // `matchesKey` layer would otherwise normalize away.
        if kb.matches(data, "tui.input.newLine")
            || (data.starts_with('\n') && data.len() > 1)
            || data == "\x1b\r"
            || data == "\x1b[13;2~"
            || (data.len() > 1 && data.contains('\x1b') && data.contains('\r'))
            || data == "\n"
        {
            self.newline();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.input.submit") {
            // Workaround for terminals without Shift+Enter support: a `\`
            // typed before Enter inserts a newline instead of submitting.
            if self.cursor_preceded_by_backslash() {
                self.backspace();
                self.newline();
                return EditorEvent::Changed;
            }
            let text = self.text();
            self.submit();
            return EditorEvent::Submitted(text);
        }
        if kb.matches(data, "tui.input.tab") {
            self.refresh_suggestions(true);
            // pi applies silently when force+Tab yields exactly one item
            // (`editor.md` §8), so a single candidate needs one Tab, not two.
            if self
                .suggestions
                .as_ref()
                .is_some_and(|s| s.items.len() == 1)
            {
                self.accept_suggestion();
                self.clear_suggestions();
            }
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.deleteCharBackward") {
            self.backspace();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.deleteCharForward") {
            self.delete_forward();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.deleteWordBackward") {
            self.delete_word_backward();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.deleteToLineEnd") {
            self.delete_to_line_end();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.deleteToLineStart") {
            self.delete_to_line_start();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.yank") {
            self.yank();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.undo") {
            self.undo();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.cursorLeft") {
            self.cursor_left();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.cursorRight") {
            self.cursor_right();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.cursorUp") {
            self.cursor_up();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.cursorDown") {
            self.cursor_down();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.cursorLineStart") {
            self.cursor_line_start();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.cursorLineEnd") {
            self.cursor_line_end();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.cursorWordLeft") {
            self.cursor_word_left();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.cursorWordRight") {
            self.cursor_word_right();
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.jumpForward") {
            self.jump_pending = Some(true);
            return EditorEvent::Changed;
        }
        if kb.matches(data, "tui.editor.jumpBackward") {
            self.jump_pending = Some(false);
            return EditorEvent::Changed;
        }

        // Printable insertion (including Kitty CSI-u decoding).
        if let Some(text) = crate::engine::keys::decode_printable_key(data) {
            self.insert_typed(&text, continues_undo);
            return EditorEvent::Changed;
        }
        if let Some(key) = crate::engine::keys::parse_key(data) {
            if key == "space" {
                self.insert_typed(" ", continues_undo);
                return EditorEvent::Changed;
            }
            if key.chars().count() == 1 && !key.starts_with("ctrl+") {
                self.insert_typed(&key, continues_undo);
                return EditorEvent::Changed;
            }
        }
        EditorEvent::None
    }

    /// Move the cursor to the next (or previous) occurrence of `target`,
    /// searching the whole buffer from the cursor (pi's `jumpToChar`, R7).
    fn jump_to_char(&mut self, target: char, forward: bool) {
        let mut positions: Vec<(usize, usize)> = Vec::new();
        for (line_index, line) in self.lines.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                if ch == target {
                    positions.push((line_index, col));
                }
            }
        }
        let current = (self.cursor_line, self.cursor_col);
        let next = if forward {
            positions.iter().find(|p| **p > current)
        } else {
            positions.iter().rev().find(|p| **p < current)
        };
        if let Some((line, col)) = next {
            self.cursor_line = *line;
            self.cursor_col = *col;
            self.preferred_col = None;
            self.clear_suggestions();
        }
    }

    fn insert_paste(&mut self, content: &str) {
        // tmux with `extended-keys=csi-u` re-encodes control bytes inside a
        // bracketed paste as CSI-u Ctrl+letter; decode them back (pi's
        // `handlePaste`), then normalize line endings and tabs.
        let decoded = decode_csi_u_ctrl(content);
        let normalized = decoded
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ");
        // Drop non-printable characters except newlines.
        let mut filtered: String = normalized
            .chars()
            .filter(|c| *c == '\n' || (*c as u32) >= 32)
            .collect();
        // A pasted path after a word character gets a leading space.
        if filtered.starts_with(['/', '~', '.']) {
            let before = self.text_before_cursor();
            if before
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                filtered.insert(0, ' ');
            }
        }
        let lines = filtered.split('\n').count();
        if lines > 10 || filtered.chars().count() > 1000 {
            let marker = if lines > 10 {
                format!("[paste #{} +{} lines]", self.pastes.len() + 1, lines)
            } else {
                format!(
                    "[paste #{} {} chars]",
                    self.pastes.len() + 1,
                    filtered.chars().count()
                )
            };
            self.pastes.push(filtered);
            self.insert_str(&marker);
        } else {
            self.insert_str(&filtered);
        }
    }

    /// Render the buffer at a width, including the `CURSOR_MARKER` at the
    /// cursor position.
    pub fn render(&self, width: u16) -> Vec<String> {
        let width = width as usize;
        let mut out = Vec::new();
        let mut cursor_placed = false;
        for (li, line) in self.lines.iter().enumerate() {
            let wrapped = if line.is_empty() {
                vec![String::new()]
            } else {
                wrap_text_with_ansi(line, width.max(1))
            };
            // Determine the visual row/col of the cursor on this line.
            let mut col_tracker = 0usize;
            for (vi, visual) in wrapped.iter().enumerate() {
                let mut rendered = visual.clone();
                if li == self.cursor_line && !cursor_placed {
                    let target = self.cursor_col;
                    if target >= col_tracker && target <= col_tracker + visual.chars().count() {
                        let offset = target - col_tracker;
                        let byte = visual
                            .char_indices()
                            .nth(offset)
                            .map(|(b, _)| b)
                            .unwrap_or(visual.len());
                        rendered = format!("{}{CURSOR_MARKER}{}", &visual[..byte], &visual[byte..]);
                        cursor_placed = true;
                    }
                    col_tracker += visual.chars().count();
                }
                out.push(rendered);
                let _ = vi;
            }
            if li == self.cursor_line
                && !cursor_placed
                && let Some(last) = out.last_mut()
            {
                last.push_str(CURSOR_MARKER);
                cursor_placed = true;
            }
        }
        out
    }

    /// Render the autocomplete popup (to be placed near the editor).
    pub fn render_popup(&self, width: u16) -> Vec<String> {
        let Some(s) = &self.suggestions else {
            return Vec::new();
        };
        let width = width as usize;
        s.items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let marker = if i == self.suggestion_index {
                    "▸ "
                } else {
                    "  "
                };
                let desc = item
                    .description
                    .as_ref()
                    .map(|d| format!("  {d}"))
                    .unwrap_or_default();
                let line = format!("{marker}{}{desc}", item.label);
                truncate_to_width(&line, width, "…", false)
            })
            .collect()
    }

    /// The visible width of the widest buffer line.
    pub fn max_width(&self) -> usize {
        self.lines
            .iter()
            .map(|l| visible_width(l))
            .max()
            .unwrap_or(0)
    }
}

/// Decode tmux's CSI-u Ctrl+letter encoding of control bytes inside a
/// bracketed paste (`ESC [ <cp> ; 5 u` -> the literal control byte).
fn decode_csi_u_ctrl(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            let mut j = i + 2;
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start
                && bytes.get(j) == Some(&b';')
                && bytes.get(j + 1) == Some(&b'5')
                && bytes.get(j + 2) == Some(&b'u')
            {
                let cp: u32 = text[start..j].parse().unwrap_or(0);
                let decoded = if (97..=122).contains(&cp) {
                    char::from_u32(cp - 96)
                } else if (65..=90).contains(&cp) {
                    char::from_u32(cp - 64)
                } else {
                    None
                };
                if let Some(c) = decoded {
                    out.push(c);
                    i = j + 3;
                    continue;
                }
            }
        }
        match text[i..].chars().next() {
            Some(c) => {
                out.push(c);
                i += c.len_utf8();
            }
            None => break,
        }
    }
    out
}

/// Whether a typed character continues an undo-coalescing run: pi merges
/// consecutive word characters and treats anything else as a boundary.
fn is_undo_word_char(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// The single printable character a key inserts, for jump mode (R7).
fn printable_char(data: &str) -> Option<char> {
    if let Some(text) = crate::engine::keys::decode_printable_key(data) {
        return text.chars().next();
    }
    let key = crate::engine::keys::parse_key(data)?;
    if key == "space" {
        return Some(' ');
    }
    if key.chars().count() == 1 && !key.starts_with("ctrl+") {
        return key.chars().next();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::core::extract_cursor_position;

    #[test]
    fn typing_and_newline() {
        let mut e = Editor::new();
        e.insert_str("hello");
        e.newline();
        e.insert_str("world");
        assert_eq!(e.lines(), &["hello", "world"]);
        assert_eq!(e.cursor_line, 1);
    }

    // Verifies: R7 - the sticky column survives a shorter line.
    #[test]
    fn the_sticky_column_survives_shorter_lines() {
        let mut e = Editor::new();
        e.set_text("long line here\nx\nanother long line");
        e.cursor_up();
        e.cursor_up();
        e.cursor_line_start();
        for _ in 0..14 {
            e.cursor_right();
        }
        assert_eq!(e.cursor_col, 14);
        e.cursor_down();
        assert_eq!(e.cursor_col, 1, "clamped to the short line");
        e.cursor_down();
        assert_eq!(e.cursor_col, 14, "sticky column restored");
    }

    // Verifies: R7 - jump mode moves to the next occurrence of a character.
    #[test]
    fn jump_mode_moves_to_the_next_character() {
        let mut e = Editor::new();
        e.set_text("abcabc");
        e.cursor_line_start();
        e.handle_key("\x1d"); // Ctrl+]
        e.handle_key("b");
        assert_eq!(e.cursor_col, 1);
        e.handle_key("\x1d");
        e.handle_key("b");
        assert_eq!(e.cursor_col, 4);
    }

    #[test]
    fn enter_submits_and_resets() {
        let mut e = Editor::new();
        e.insert_str("hi");
        let ev = e.handle_key("\r");
        assert_eq!(ev, EditorEvent::Submitted("hi".to_string()));
        assert_eq!(e.text(), "");
    }

    #[test]
    fn shift_enter_inserts_newline() {
        let mut e = Editor::new();
        e.insert_str("a");
        e.handle_key("\x1b[13;2u");
        e.insert_str("b");
        assert_eq!(e.lines(), &["a", "b"]);
    }

    // A terminal that cannot report Shift+Enter at all: a `\` typed before
    // Enter inserts a newline instead of submitting (pi's workaround).
    #[test]
    fn backslash_enter_inserts_a_newline() {
        let mut e = Editor::new();
        e.insert_str("a\\");
        assert_eq!(
            e.handle_key("\r"),
            EditorEvent::Changed,
            "newline, not submit"
        );
        e.insert_str("b");
        assert_eq!(e.lines(), &["a", "b"]);
    }

    // The fallback pi documents: Ctrl+J is a newline in every dialect,
    // including a legacy terminal where Shift+Enter is indistinguishable
    // from Enter.
    #[test]
    fn ctrl_j_inserts_newline_in_every_dialect() {
        let mut e = Editor::new();
        e.insert_str("a");
        assert_eq!(e.handle_key("\n"), EditorEvent::Changed, "legacy ctrl+j");
        e.insert_str("b");
        assert_eq!(e.lines(), &["a", "b"]);
    }

    #[test]
    fn backspace_joins_lines() {
        let mut e = Editor::new();
        e.insert_str("a");
        e.newline();
        e.insert_str("b");
        e.cursor_line_start();
        e.backspace();
        assert_eq!(e.lines(), &["ab"]);
    }

    #[test]
    fn history_navigation() {
        let mut e = Editor::new();
        e.insert_str("first");
        e.submit();
        e.insert_str("second");
        e.submit();
        e.insert_str("draft");
        e.cursor_up();
        assert_eq!(e.text(), "second");
        e.cursor_up();
        assert_eq!(e.text(), "first");
        e.cursor_down();
        e.cursor_down();
        assert_eq!(e.text(), "draft");
    }

    #[test]
    fn word_motion_and_deletion() {
        let mut e = Editor::new();
        e.insert_str("hello world");
        e.cursor_word_left();
        assert_eq!(e.cursor_col, 6);
        e.delete_word_backward();
        assert_eq!(e.text(), "world");
    }

    #[test]
    fn undo_restores_the_previous_buffer() {
        let mut e = Editor::new();
        e.insert_str("hello");
        e.insert_str(" world");
        e.undo();
        assert_eq!(e.text(), "hello");
    }

    // pi's fish-style coalescing: a typed word is one undo unit, and a
    // space starts the next one; an atomic insert is always its own unit.
    #[test]
    fn typing_coalesces_into_word_sized_undo_units() {
        let mut e = Editor::new();
        for c in "hello world".chars() {
            e.handle_key(&c.to_string());
        }
        assert_eq!(e.text(), "hello world");
        e.undo();
        assert_eq!(e.text(), "hello ");
        e.undo();
        assert_eq!(e.text(), "hello");
        e.undo();
        assert_eq!(e.text(), "");
    }

    // pi accumulates consecutive kills into one ring entry: a forward kill
    // appends, a backward kill prepends, so one yank restores the run.
    #[test]
    fn consecutive_kills_accumulate_in_the_ring() {
        let mut e = Editor::new();
        e.insert_str("one two");
        e.cursor_line_end();
        e.delete_word_backward(); // kills "two "
        e.delete_word_backward(); // kills "one " and prepends
        assert_eq!(e.text(), "");
        assert_eq!(e.kill_ring.len(), 1, "one accumulated entry");
        assert_eq!(e.kill_ring[0], "one two");
        e.yank();
        assert_eq!(e.text(), "one two");
    }

    #[test]
    fn an_atomic_insert_is_one_undo_unit() {
        let mut e = Editor::new();
        e.insert_str("pasted block");
        e.undo();
        assert_eq!(e.text(), "");
    }

    // Verifies: FR-UI-10 - a multi-line paste is one atomic segment.
    #[test]
    fn large_paste_becomes_a_marker() {
        let mut e = Editor::new();
        let big = (0..12)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        e.handle_key(&format!("\x1b[200~{big}\x1b[201~"));
        assert_eq!(e.text(), "[paste #1 +12 lines]");
    }

    #[test]
    fn a_long_single_line_paste_becomes_a_chars_marker() {
        let mut e = Editor::new();
        let big = "x".repeat(1200);
        e.handle_key(&format!("\x1b[200~{big}\x1b[201~"));
        assert_eq!(e.text(), "[paste #1 1200 chars]");
    }

    #[test]
    fn paste_decodes_tmux_csi_u_ctrl_and_normalizes() {
        let mut e = Editor::new();
        // tmux CSI-u Ctrl+J inside the paste becomes a newline; a tab
        // expands to four spaces.
        e.handle_key("\x1b[200~a\x1b[106;5ub\tc\x1b[201~");
        assert_eq!(e.text(), "a\nb    c");
    }

    #[test]
    fn cursor_marker_sits_at_the_cursor() {
        let mut e = Editor::new();
        e.insert_str("abc");
        e.cursor_left();
        let lines = e.render(40);
        let (stripped, pos) = extract_cursor_position(&lines);
        assert_eq!(stripped[0], "abc");
        assert_eq!(pos, Some((0, 2)));
    }

    #[test]
    fn autocomplete_accepts_and_inserts() {
        use crate::widgets::autocomplete::{AutocompleteItem, SlashCommand};
        let commands = vec![SlashCommand {
            name: "model".into(),
            description: None,
            argument_hint: None,
            argument_completions: Some(Arc::new(|_p: &str| {
                vec![AutocompleteItem {
                    value: "gpt-4o".into(),
                    label: "gpt-4o".into(),
                    description: None,
                }]
            })),
        }];
        let provider = Arc::new(
            crate::widgets::autocomplete::CombinedAutocompleteProvider::new(
                commands,
                std::env::temp_dir(),
            ),
        );
        let mut e = Editor::new();
        e.set_autocomplete(provider);
        e.insert_str("/model ");
        assert!(e.suggestions().is_some());
        e.accept_suggestion();
        assert_eq!(e.text(), "/model gpt-4o");
    }

    #[test]
    fn space_is_inserted() {
        let mut e = Editor::new();
        e.handle_key("a");
        e.handle_key(" ");
        e.handle_key("b");
        assert_eq!(e.text(), "a b");
    }

    struct SingleCandidate;
    impl AutocompleteProvider for SingleCandidate {
        fn get_suggestions(&self, _prefix: &str, _force: bool) -> Option<Suggestions> {
            Some(Suggestions {
                items: vec![crate::widgets::autocomplete::AutocompleteItem {
                    value: "README.md".into(),
                    label: "README.md".into(),
                    description: None,
                }],
                prefix: "REA".into(),
            })
        }
    }

    // Verifies: FR-UI-9 (pi's editor.md §8 - force+Tab with one candidate
    // applies it silently, so one Tab suffices).
    #[test]
    fn tab_applies_a_single_candidate_silently() {
        let mut e = Editor::new();
        e.set_autocomplete(Arc::new(SingleCandidate));
        e.insert_str("read REA");
        e.handle_key("\t");
        assert_eq!(e.text(), "read README.md");
        assert!(e.suggestions().is_none());
    }
}
