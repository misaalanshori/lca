//! The transcript search (FR-UI-12), split from `chat.rs` under the workspace's
//! 1,200-line ceiling rule - the same split `chat_pickers` and `chat_shell` already
//! took.
//!
//! Gh #376 replaced the per-row `contains` with pi's shape (`alt-screen-search.ts`):
//! a **wrap-insensitive corpus** ([`crate::chat::chat_search_corpus`]) so a phrase
//! split across two rendered rows still matches, a selection held by **match identity**
//! rather than by list index, an anchored first pick instead of always the top of the
//! document, a reveal that leaves a third of the viewport above, and a scroll lock so a
//! streaming delta cannot take the selected match away.

use super::chat::Chat;
use crate::chat_commands::{paste_text, printable};
use crate::state::Action;

/// How the current selection was asked for, which decides how a re-query picks the
/// next one (pi's `selectionMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// A new query was typed: select the `anchor_row`'s first match, else the first.
    Query,
    /// Enter: step forward from the selected match, wrapping.
    Next,
    /// The previous key: step back, wrapping.
    Previous,
    /// Nothing asked: keep the selection by its key, re-picking only if it vanished.
    Retain,
}

impl Chat {
    /// Handle a key while the transcript search is open (FR-UI-12).
    pub(crate) fn handle_search_key(&mut self, data: &str, key: Option<&str>) -> Action {
        let mut query = self.search.take().unwrap_or_default();
        // Gh #376 gap 4: the direction keys, asked through the keybinding table so a
        // user's rebind reaches the search; the bindings have existed since the table
        // was written and only `enter` had a handler.
        if self.keybindings.matches(data, "tui.altScreen.searchClose") {
            self.search = None;
            self.search_matches.clear();
            self.search_selected = None;
            return Action::Continue;
        }
        if self
            .keybindings
            .matches(data, "tui.altScreen.searchPrevious")
        {
            self.search = Some(query);
            self.step_search(Mode::Previous);
            return Action::Continue;
        }
        if self.keybindings.matches(data, "tui.altScreen.searchNext") {
            self.search = Some(query);
            self.step_search(Mode::Next);
            return Action::Continue;
        }
        match key {
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
        if self.search_anchor.is_none() {
            self.search_anchor = Some(self.search_top_row());
        }
        self.refresh_search(Mode::Query);
        // The readout renders from state in the dock (gh #239);
        // nothing lands in history per keystroke.
        Action::Continue
    }

    /// The document row the reader is looking at: the top of the transcript window.
    ///
    /// Gh #376 gap 2's anchor. A reader who opened the search 300 rows into a
    /// transcript should not be teleported to row 0 because the first match happens
    /// to be there.
    pub(crate) fn search_top_row(&self) -> usize {
        let width = self.world.size.0.max(1);
        let height = self.world.size.1.max(1);
        let content = self.transcript_len(width);
        content
            .saturating_sub(self.window_height(width, height))
            .saturating_sub(usize::from(self.last_scroll))
    }

    /// Recompute the matches and select one, per `mode` (FR-UI-12, pi's `refreshSearch`).
    pub(crate) fn refresh_search(&mut self, mode: Mode) {
        let query = self.search.clone().unwrap_or_default();
        let width = self.world.size.0.max(1);
        // The **transcript**, not the whole frame: pi searches the scroll view's content,
        // and the dock carries the readout - which contains the query itself. Searching
        // the frame made the readout a match for its own query (measured: two hits for one
        // phrase). The transcript's rows are also the rows the viewport slices and the
        // paint ranges over, so one coordinate space serves all three.
        //
        // The whole document, not the visible window: matches above the window are part
        // of the answer ("wrap to the one above"), and a match keeps its document row
        // whatever the window later does.
        let lines = self.transcript.render(width, &self.theme);
        let corpus = super::chat::chat_search_corpus::build(&lines, 0);
        let matches = super::chat::chat_search_corpus::find(&corpus, &query);
        let previous = std::mem::take(&mut self.search_matches);
        let was_empty = previous.is_empty();
        self.search_matches = matches;

        if self.search_matches.is_empty() {
            self.search_index = 0;
            self.search_selected = None;
            self.search_follow_lock.set(None);
            return;
        }
        let exact = self
            .search_selected
            .and_then(|key| self.search_matches.iter().position(|m| m.key() == key));
        let selected = match mode {
            Mode::Query => {
                let anchor = self.search_anchor.unwrap_or_else(|| self.search_top_row());
                self.search_matches
                    .iter()
                    .position(|m| m.first_row() >= anchor)
                    .unwrap_or(0)
            }
            Mode::Next => exact.map_or(0, |index| (index + 1) % self.search_matches.len()),
            Mode::Previous => exact.map_or(self.search_matches.len() - 1, |index| {
                (index + self.search_matches.len() - 1) % self.search_matches.len()
            }),
            Mode::Retain => {
                exact.unwrap_or_else(|| self.search_index.min(self.search_matches.len() - 1))
            }
        };
        let reveal = !was_empty || mode != Mode::Retain;
        self.select_search_match(selected, reveal);
    }

    /// Enter / the previous key: step one match, wrapping (FR-UI-12).
    pub(crate) fn step_search(&mut self, mode: Mode) {
        self.refresh_search(mode);
    }

    /// The scroll a reveal asked for, taken by the loop (gh #376 gap 3).
    pub(crate) fn take_search_scroll(&mut self) -> Option<u16> {
        self.search_scroll.take()
    }

    /// Select match `index` and reveal it (FR-UI-12, gap 3).
    ///
    /// The reveal leaves **a third of the window above** the match (pi's
    /// `firstSegment.row - viewport/3`), so there is context on both sides rather than
    /// the match pinned to an edge, and it sets the scroll lock: while the lock holds,
    /// [`Self::clamp_scroll`] neither re-engages follow nor adds a stream delta, so a
    /// token arriving mid-read cannot take the match away.
    pub(crate) fn select_search_match(&mut self, index: usize, reveal: bool) {
        let Some(selected) = self.search_matches.get(index) else {
            return;
        };
        self.search_index = index;
        self.search_selected = Some(selected.key());
        if !reveal {
            return;
        }
        let width = self.world.size.0.max(1);
        let height = self.world.size.1.max(1);
        // Gh #376 gap 3: the reveal is **a scroll request, not a jump pin**. A
        // `JumpTarget::Row` top-pins (that is what a prompt jump means), and the
        // search wants a third of the window *above* the match; two mechanisms
        // writing the same position would leave whichever ran last to decide.
        let scroll = self.scroll_revealing(selected.first_row(), width, height);
        self.search_scroll = Some(scroll);
        self.search_follow_lock.set(Some(scroll));
    }

    /// The scroll that puts document row `row` a third of the way down the window.
    ///
    /// Bottom coordinates (`Scroll`), so this is pi's `row - viewport/3` re-expressed:
    /// the window's top row becomes `row - window/3`.
    pub(crate) fn scroll_revealing(&self, row: usize, width: u16, height: u16) -> u16 {
        let content = self.transcript_len(width);
        let window = self.window_height(width, height);
        let lead = window / 3;
        let top = row.saturating_sub(lead);
        let scroll = content.saturating_sub(window).saturating_sub(top);
        scroll.min(u16::MAX as usize) as u16
    }

    /// Paint one rendered row's search segments (FR-UI-12, gap 4).
    ///
    /// **Two styles, span-only, per-channel closes.** pi's defaults are
    /// `\x1b[4m…\x1b[24m` for the other matches and `\x1b[1;7m…\x1b[22;27m` for the
    /// current one; each closes exactly the channels it opened, never `\x1b[0m`
    /// (which would also drop the row's own background, its italic, and any
    /// hyperlink - this tree's law: a style resets only its own channel).
    ///
    /// The row is walked **once**, keeping every ANSI code in place and wrapping only
    /// the plain runs a match covers - pi's `applySearchTextHighlight` shape. That is
    /// why it does not go through `splice_segment`: that seam resets the whole line at
    /// its edges (it exists for overlays that *replace* a region), and a match is an
    /// addition, so a reset would be a fidelity leak rather than a repair.
    pub(crate) fn paint_search_row(&self, row: usize, line: &str) -> String {
        if self.search.is_none() || self.search_matches.is_empty() {
            return line.to_string();
        }
        let base = self.search_view_base.get();
        let document_row = base.saturating_add(row);
        let current_key = self.search_selected;
        let mut hits: Vec<(usize, usize, bool)> = self
            .search_matches
            .iter()
            .filter(|m| m.first_row() <= document_row && m.last_row() >= document_row)
            .flat_map(|m| {
                let current = Some(m.key()) == current_key;
                m.segments
                    .iter()
                    .copied()
                    .filter(move |s| s.row == document_row)
                    .map(move |s| (s.start_col, s.end_col, current))
            })
            .filter(|(start, end, _)| end > start)
            .collect();
        if hits.is_empty() {
            return line.to_string();
        }
        hits.sort_by_key(|(start, _, _)| *start);
        let mut out = String::new();
        let mut column = 0usize;
        let mut hit = 0usize;
        let mut open: Option<bool> = None;
        let mut at = 0usize;
        while at < line.len() {
            if let Some((code, len)) = lca_tui::engine::text::extract_ansi_code(line, at) {
                out.push_str(&code);
                at += len;
                continue;
            }
            let Some(ch) = line[at..].chars().next() else {
                break;
            };
            let width = lca_tui::engine::text::visible_width(&ch.to_string());
            // Close any span that ends here before this glyph, and open one that starts.
            if let Some(current) = open
                && hits[hit].1 <= column
            {
                out.push_str(close_for(current));
                open = None;
                hit += 1;
            }
            while hit < hits.len() && hits[hit].1 <= column {
                hit += 1;
            }
            if open.is_none() && hit < hits.len() && hits[hit].0 <= column {
                out.push_str(open_for(hits[hit].2));
                open = Some(hits[hit].2);
            }
            out.push(ch);
            column += width;
            at += ch.len_utf8();
        }
        if let Some(current) = open {
            out.push_str(close_for(current));
        }
        out
    }
}

/// The open half of a match's style: `(bold+inverse)` for the current match,
/// `(underline)` for the others (pi's `searchCurrentMatchStyle`/`searchMatchStyle`).
fn open_for(current: bool) -> &'static str {
    if current { "\x1b[1;7m" } else { "\x1b[4m" }
}

/// The close that undoes exactly what [`open_for`] set: `22;27` for bold+inverse,
/// `24` for underline. Never `0m` - see the module note.
fn close_for(current: bool) -> &'static str {
    if current { "\x1b[22;27m" } else { "\x1b[24m" }
}
