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

/// Expand `[paste #N …]` markers to the content they stand for (pi's
/// `expandPasteMarkers`). A marker whose id has no live entry, or one with
/// no closing bracket, is left literal so a user who types the shape by hand
/// is not surprised. Markers are produced by [`Editor::insert_paste`].
fn expand_paste_markers(text: &str, pastes: &[String]) -> String {
    const OPEN: &str = "[paste #";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let marker = &rest[start..];
        let digits: String = marker[OPEN.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        match (digits.parse::<usize>(), marker.find(']')) {
            (Ok(id), Some(close)) if id >= 1 && id <= pastes.len() => {
                out.push_str(&pastes[id - 1]);
                rest = &marker[close + 1..];
            }
            _ => {
                // Not a marker this editor wrote: keep it and move past the
                // opening bracket so the search cannot loop.
                out.push('[');
                rest = &marker[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A multi-line prompt editor.
pub struct Editor {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize, // in chars
    /// Whether the caret is painted into the row (reverse video). The
    /// plain theme renders no escapes at all (FR-UI-5), so there the
    /// hardware cursor - positioned at the marker - stays the caret.
    paint_caret: bool,
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
            paint_caret: true,
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

    /// Choose how the caret is shown: painted into the row (colored
    /// themes) or left to the hardware cursor (the plain theme, which
    /// renders no escapes - FR-UI-5).
    pub fn set_paint_caret(&mut self, on: bool) {
        self.paint_caret = on;
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
        // A new buffer owns no pastes; a stale registry would make the next
        // marker reuse an id whose content no longer belongs to it.
        self.pastes.clear();
        self.clear_suggestions();
    }

    /// The buffer with paste markers expanded (pi's `getExpandedText`):
    /// what the external editor and any other full-text consumer needs.
    pub fn expanded_text(&self) -> String {
        expand_paste_markers(&self.text(), &self.pastes)
    }

    /// Clear the buffer and push the submitted text to history. Paste
    /// markers expand to their content first (pi's `submitValue`), so the
    /// model receives what the user pasted, not `[paste #1 …]`.
    pub fn submit(&mut self) -> String {
        let text = expand_paste_markers(&self.text(), &self.pastes);
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.history_index = None;
        self.pastes.clear();
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
            // Delete the whole cluster before the cursor (pi's snap).
            let boundary = Self::prev_cluster_boundary(self.current(), self.cursor_col);
            let end = self.char_to_byte(self.cursor_col);
            let start = self.char_to_byte(boundary);
            self.current_mut().replace_range(start..end, "");
            self.cursor_col = boundary;
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
            // Delete the whole cluster after the cursor (pi's snap).
            let boundary = Self::next_cluster_boundary(self.current(), self.cursor_col);
            let start = self.char_to_byte(self.cursor_col);
            let end = self.char_to_byte(boundary);
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
            // Grapheme-snapped (pi): step over a whole cluster.
            self.cursor_col = Self::prev_cluster_boundary(self.current(), self.cursor_col);
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
            // Grapheme-snapped (pi): a cluster entered by a stale column
            // is left whole.
            self.cursor_col = Self::next_cluster_boundary(self.current(), self.cursor_col);
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
        self.clear_suggestions();
    }

    /// Move the cursor up: inside the buffer first, and into history only
    /// at the boundary pi defines (`editor.ts` cursorUp) - on the first
    /// line when the buffer is empty, when already browsing history, or
    /// with the caret at column 0. Anywhere else on the first line it
    /// jumps to the start of the line instead (issue #12).
    pub fn cursor_up(&mut self) {
        if self.cursor_line > 0 {
            let preferred = *self.preferred_col.get_or_insert(self.cursor_col);
            self.cursor_line -= 1;
            self.cursor_col = preferred.min(self.current().chars().count());
        } else {
            let empty = self.lines.len() == 1 && self.lines[0].is_empty();
            let at_start = self.cursor_col == 0;
            if (empty || self.history_index.is_some() || at_start) && !self.history.is_empty() {
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
            } else {
                // pi: top line, not browsing, caret off the start - move to
                // the start of the line, do not jump into history.
                self.cursor_col = 0;
                self.preferred_col = None;
            }
        }
        self.clear_suggestions();
    }

    /// Move the cursor down: inside the buffer first, then - only while
    /// already browsing - through history (`editor.ts` cursorDown). At the
    /// last line without a history open, pi jumps to the end of the line;
    /// this used to do nothing (issue #12).
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
        } else {
            // pi: bottom line, no history open - end of the line.
            self.cursor_col = self.current().chars().count();
            self.preferred_col = None;
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

        // Bracketed paste (the shared primitive, R1).
        if let Some(content) = crate::widgets::paste::bracketed_paste_content(data) {
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
            // `submit` returns the expanded text (its paste markers become
            // their content) and resets the buffer; use its value rather
            // than reading the buffer first, which would keep a marker.
            let text = self.submit();
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
        // The shared primitive does the tmux CSI-u decode and the
        // CRLF/tab/non-printable normalization (TUI cycle 7, R1) so the
        // editor and every single-line field agree on what a paste is.
        let mut filtered = crate::widgets::paste::normalize(content);
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

    /// Render the buffer at a width. The caret is *painted into the row*
    /// (pi's model, `editor.md` §5): the grapheme cluster under it in
    /// reverse video, an end-of-line caret as a reverse-video space, and
    /// the zero-width `CURSOR_MARKER` immediately before either form, so
    /// the engine can still position the (hidden) hardware cursor for IME
    /// candidate windows while the painted caret is what the user sees.
    ///
    /// The cursor is cluster-snapped and row-boundary-snapped exactly as
    /// pi's `layoutText`: on a middle wrapped row the caret sits strictly
    /// inside it (a boundary lands at the start of the next row), and only
    /// the line's last row owns the end-of-line position.
    pub fn render(&self, width: u16) -> Vec<String> {
        let width = width as usize;
        let mut out: Vec<String> = Vec::new();
        let mut cursor_placed = false;
        for (li, line) in self.lines.iter().enumerate() {
            let wrapped = if line.is_empty() {
                vec![String::new()]
            } else {
                wrap_text_with_ansi(line, width.max(1))
            };
            let last_row = wrapped.len().saturating_sub(1);
            let mut col_tracker = 0usize;
            for (ri, visual) in wrapped.iter().enumerate() {
                let mut rendered = visual.clone();
                if li == self.cursor_line && !cursor_placed {
                    let target = self.cursor_col;
                    let row_chars = visual.chars().count();
                    let on_row = if ri == last_row {
                        target >= col_tracker && target <= col_tracker + row_chars
                    } else {
                        target >= col_tracker && target < col_tracker + row_chars
                    };
                    if on_row {
                        let offset = target - col_tracker;
                        let (before, cluster, after) = Self::caret_split(visual, offset);
                        rendered = if self.paint_caret {
                            match cluster {
                                Some(g) => {
                                    format!("{before}{CURSOR_MARKER}\x1b[7m{g}\x1b[0m{after}")
                                }
                                None => format!("{before}{CURSOR_MARKER}\x1b[7m \x1b[0m"),
                            }
                        } else {
                            // The cluster is still text: keep it, and put
                            // the marker where the caret is.
                            match cluster {
                                Some(g) => format!("{before}{CURSOR_MARKER}{g}{after}"),
                                None => format!("{before}{CURSOR_MARKER}{after}"),
                            }
                        };
                        cursor_placed = true;
                    }
                    col_tracker += row_chars;
                }
                out.push(rendered);
            }
            if li == self.cursor_line
                && !cursor_placed
                && let Some(last) = out.last_mut()
            {
                // The cursor landed past every wrapped row (a trimmed
                // tail): park the painted caret at the line's end anyway.
                last.push_str(CURSOR_MARKER);
                if self.paint_caret {
                    last.push_str("\x1b[7m \x1b[0m");
                }
                cursor_placed = true;
            }
        }
        out
    }

    /// The char index of the previous grapheme-cluster boundary strictly
    /// before `col` (pi's grapheme-snapped movement: clusters are stepped over
    /// whole, never entered).
    fn prev_cluster_boundary(line: &str, col: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation as _;
        let mut boundary = 0usize;
        let mut pos = 0usize;
        for cluster in line.graphemes(true) {
            let next = pos + cluster.chars().count();
            if next >= col {
                break;
            }
            boundary = next;
            pos = next;
        }
        boundary
    }

    /// The char index of the next grapheme-cluster boundary after `col`.
    fn next_cluster_boundary(line: &str, col: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation as _;
        let mut pos = 0usize;
        for cluster in line.graphemes(true) {
            pos += cluster.chars().count();
            if pos > col {
                return pos;
            }
        }
        pos
    }

    /// Split a visual row at the grapheme cluster containing the char
    /// `offset`, snapped to the cluster's start (pi's grapheme-snapped
    /// caret: a multi-code-point cluster is painted whole, never in half).
    /// `(before, cluster, after)`; `cluster` is `None` at end of line.
    fn caret_split(visual: &str, offset: usize) -> (&str, Option<&str>, &str) {
        use unicode_segmentation::UnicodeSegmentation as _;
        let mut char_pos = 0usize;
        let mut byte_pos = 0usize;
        for cluster in visual.graphemes(true) {
            if offset < char_pos + cluster.chars().count() {
                return (
                    &visual[..byte_pos],
                    Some(cluster),
                    &visual[byte_pos + cluster.len()..],
                );
            }
            char_pos += cluster.chars().count();
            byte_pos += cluster.len();
        }
        (visual, None, "")
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
#[path = "editor_tests.rs"]
mod tests;
