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
    /// Nothing asked: keep the selection **by key**, re-picking only if it vanished.
    ///
    /// This is the mode the **geometry refresh** uses (gh #376 fix F1). pi's `retain`
    /// is exactly that (`tui-alt-screen.ts:595`, called from its layout pass at 1685),
    /// so a resize, a tab switch or a `/fullscreen` flip re-queries without moving the
    /// reader's selection.
    Retain,
}

/// The search's **frame bookkeeping** (gh #376 fix): everything the paint and scroll
/// paths need that is derived per frame rather than owned by the reader.
///
/// One cell, three members, and each member exists because a measured defect needed it:
///
/// * `base` — the document row the last painted window started at. The paint is handed a
///   window slice while a match's spans are document rows, so the difference has to be
///   recorded where the frame resolves it.
/// * `key` — the geometry the match list was computed at: `(document rows, width, query)`.
///   `None` means "nothing to compare", and then the paint does not re-query. A resize, a
///   tab switch and a `/fullscreen` flip all change at least one member, which is why the
///   refresh lives in the paint path rather than in the SIGWINCH branch (F1: a resize used
///   to paint the *old* width's columns until the reader typed).
/// * `follow` — the selected match's `(first_row, last_row)`, held while the search owns
///   the scroll. **The row, not a scroll number** (F4): pi's `disableFollow` suppresses
///   follow-at-the-end and does not pin a coordinate, so a document that grows past the
///   window cannot push the match off the top while "locked".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SearchFrame {
    /// Document row of the painted window's first row.
    pub(crate) base: usize,
    /// `(document rows, width, query)` the match list was last computed at.
    pub(crate) key: Option<(usize, u16, String)>,
    /// The selected match's rows, while the search holds the scroll.
    pub(crate) follow: Option<(usize, usize)>,
}

impl SearchFrame {
    /// Keep the frame's `base`, drop everything that belongs to the previous query or
    /// geometry (a close, a tab switch, a new search).
    fn forget(mut self) -> Self {
        self.key = None;
        self.follow = None;
        self
    }
}

impl Chat {
    /// Handle a key while the transcript search is open (FR-UI-12).
    pub(crate) fn handle_search_key(&mut self, data: &str, key: Option<&str>) -> Action {
        let mut query = self.search.take().unwrap_or_default();
        // Gh #376 gap 4: the direction keys, asked through the keybinding table so a
        // user's rebind reaches the search; the bindings have existed since the table
        // was written and only `enter` had a handler.
        if self.keybindings.matches(data, "tui.altScreen.searchClose") {
            self.close_search();
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

        // Gh #376 fix F1: record the geometry this list was computed at, so the paint
        // path can tell "same frame" from "the pane changed under the search".
        let mut frame = self.frame();
        frame.key = Some((lines.len(), width, query.clone()));
        self.set_frame(frame);
        if self.search_matches.is_empty() {
            self.search_index = 0;
            self.search_selected = None;
            let mut frame = self.frame();
            frame.follow = None;
            self.set_frame(frame);
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

    /// Select match `index`, and reveal it when `reveal` is set (FR-UI-12, gap 3).
    ///
    /// The reveal leaves **a third of the window above** the match (pi's
    /// `firstSegment.row - viewport/3`), so there is context on both sides rather than
    /// the match pinned to an edge, and it arms the follow hold with the match's
    /// **rows**: while the hold is armed, [`Self::clamp_scroll`] keeps those rows inside
    /// the window rather than returning a frozen scroll number (gh #376 fix F4 - the
    /// number drifted away from the row in five streamed lines).
    pub(crate) fn select_search_match(&mut self, index: usize, reveal: bool) {
        let Some((key, first_row, last_row)) = self
            .search_matches
            .get(index)
            .map(|selected| (selected.key(), selected.first_row(), selected.last_row()))
        else {
            return;
        };
        self.search_index = index;
        self.search_selected = Some(key);
        let mut frame = self.frame();
        frame.follow = Some((first_row, last_row));
        self.set_frame(frame);
        let selected_first = first_row;
        if !reveal {
            return;
        }
        let width = self.world.size.0.max(1);
        let height = self.world.size.1.max(1);
        let scroll = self.scroll_revealing(selected_first, width, height);
        self.search_scroll = Some(scroll);
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

    /// Close the search and **release everything it owns** (gh #376 fix F2).
    ///
    /// The close arm used to clear `search`, the matches and the selection while
    /// leaving the follow hold armed, so `clamp_scroll` returned the stale reveal
    /// scroll for the rest of the session and the transcript never followed the turn
    /// again (measured: 11 rows streamed, scroll frozen at 23 while the live bottom
    /// moved 54 → 59). A close is the reader saying they are done, so follow-at-the-end
    /// resumes: the anchor and the frame key/hold go with it.
    pub(crate) fn close_search(&mut self) {
        self.search = None;
        self.search_matches.clear();
        self.search_index = 0;
        self.search_selected = None;
        self.search_anchor = None;
        self.search_scroll = None;
        self.set_frame(self.frame().forget());
    }

    /// Open the search at the geometry the reader is at **now** (gh #376 fix F3).
    ///
    /// `app.search` only cleared the query and the index, so the anchor written by the
    /// *first* search of the tab survived a close and a scroll: the next search then
    /// picked the first search's row instead of the reader's. Opening is the anchor's
    /// only honest writer, so it forgets the previous one here and the first keystroke
    /// re-reads [`Self::search_top_row`].
    pub(crate) fn open_search(&mut self) {
        self.search = Some(String::new());
        self.search_matches.clear();
        self.search_index = 0;
        self.search_selected = None;
        self.search_anchor = None;
        self.search_scroll = None;
        self.set_frame(self.frame().forget());
    }

    /// Record the document row the painted window starts at (gh #376 fix F1).
    ///
    /// Called by both renderers at the seam where a **window slice** is taken from a
    /// **document**, because the paint receives the slice while a match's spans are
    /// document rows. `&mut self` because the field is a plain `Option` the overlay
    /// registry can clear (F6), so interior mutability is not available here.
    pub(crate) fn set_search_base(&mut self, base: usize) {
        let mut frame = self.frame();
        frame.base = base;
        self.set_frame(frame);
    }

    /// A copy of the search's frame bookkeeping.
    pub(crate) fn frame(&self) -> SearchFrame {
        self.search_frame.clone().unwrap_or_default()
    }

    /// Replace the search's frame bookkeeping.
    pub(crate) fn set_frame(&mut self, frame: SearchFrame) {
        self.search_frame = Some(frame);
    }

    /// Re-query when the document or the geometry changed under an open search
    /// (gh #376 fix F1/F7).
    ///
    /// Called from the paint path - the one function both renderers pass through - so a
    /// resize, a tab switch and a `/fullscreen` flip all land here rather than needing
    /// three write sites. The comparison is what keeps an idle search free: an unchanged
    /// `(rows, width, query)` means no corpus build, no search, no reveal.
    ///
    /// `Mode::Retain` keeps the selected match **by key** and re-picks only if it
    /// vanished, which is pi's `retain` on its own layout pass.
    pub(crate) fn refresh_search_for_geometry(&mut self, rows: usize, width: u16) {
        let Some(query) = self.search.clone() else {
            return;
        };
        if query.is_empty() {
            return;
        }
        let key = (rows, width, query);
        if self.frame().key.as_ref().is_some_and(|last| last == &key) {
            return;
        }
        self.refresh_search(Mode::Retain);
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
    pub(crate) fn paint_search_row(&self, line: &str, hits: &[(usize, usize, bool)]) -> String {
        if hits.is_empty() {
            return line.to_string();
        }
        let mut hits = hits.to_vec();
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
            // The same discriminator that made the corpus fast (see
            // `chat_search_corpus::build`): a printable-ASCII character is one cell, so
            // the common row needs no per-glyph measurement. A 615-row scrollback paint
            // measured 55 ms with `visible_width(&ch.to_string())` on every character -
            // the paint's real cost, not the match scan.
            let width = if ch.is_ascii() {
                1
            } else {
                lca_tui::engine::text::visible_width(&ch.to_string())
            };
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

    /// Paint every visible row's segments, with the work **bounded by the window**
    /// (gh #376 fix F5).
    ///
    /// The old shape asked each painted row which matches covered it, which is
    /// `O(document matches × rows)` - measured **62.7 ms per frame** for 600 rows and
    /// 1,800 matches while the corpus build+find on the same document was 6.4 ms. This
    /// is pi's `applySearchHighlights` shape: binary-search the first match whose last
    /// row is visible, stop at the first whose first row is past the window, and build a
    /// per-row range map once, so the cost is bound by the window and by the matches
    /// *inside* it.
    pub(crate) fn paint_search_rows(&self, screen: &mut [String]) {
        if self.search.is_none() || self.search_matches.is_empty() {
            return;
        }
        let base = self.frame().base;
        let end = base + screen.len();
        let current_key = self.search_selected;
        // First match whose **last** row reaches the window's first row.
        let from = self.search_matches.partition_point(|m| m.last_row() < base);
        let mut by_row: Vec<Vec<(usize, usize, bool)>> = vec![Vec::new(); screen.len()];
        for matching in &self.search_matches[from..] {
            if matching.first_row() >= end {
                break;
            }
            let current = Some(matching.key()) == current_key;
            for segment in &matching.segments {
                if segment.row < base {
                    continue;
                }
                let Some(row) = by_row.get_mut(segment.row - base) else {
                    break;
                };
                if segment.end_col > segment.start_col {
                    row.push((segment.start_col, segment.end_col, current));
                }
            }
        }
        for (row, hits) in by_row.iter().enumerate() {
            if hits.is_empty() {
                continue;
            }
            if let Some(line) = screen.get_mut(row) {
                *line = self.paint_search_row(line, hits);
            }
        }
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
