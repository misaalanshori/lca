//! Session tabs (gh #209): one tab per conversation session on the
//! #208 Tabs widget chrome, with per-tab composer and scroll state.
//! Split from `chat.rs` (the 1,200-line ceiling).
//!
//! The transcript is never saved per tab: switching replays the
//! host's records (the R3 switch machinery), so the log stays the
//! single source of truth and a switch is always fresh. What a tab
//! keeps is the composer draft and the scroll restore point.
//!
//! Background turns are refused, not multiplexed: a switch, open,
//! or close mid-turn names the running turn. Per-tab turn state
//! (draining each tab's stream into its own transcript) is the
//! documented follow-up, not this cycle.

use lca_tui::engine::core::{MouseButton, MouseEvent};
use lca_tui::widgets::tabs::{TabHit, TabItem};

use super::chat::Chat;

/// One conversation tab (gh #209).
#[derive(Debug, Clone)]
pub struct SessionTab {
    /// The session this tab shows.
    pub session_id: String,
    /// The tab bar title (resolved at open/switch time).
    pub title: String,
    /// The unsent composer draft.
    pub editor: String,
    /// The scroll restore point, bottom-relative like the screen
    /// coordinates (`0` follows the live bottom).
    pub scroll: u16,
}

impl Chat {
    /// Snapshot the active tab's composer and scroll point (gh #209).
    pub(crate) fn save_tab(&mut self) {
        let editor = self.editor.text();
        let scroll = self.last_scroll;
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        tab.editor = editor;
        tab.scroll = scroll;
    }

    /// Resolve a tab title (gh #209): the host's listing wins when
    /// it names the session; empty sessions never list (the resume
    /// picker hides them), so a fresh tab keeps its `untitled`
    /// until messages land. `None` keeps the current title.
    pub(crate) fn resolve_tab_title(&self, id: &str) -> Option<String> {
        let list = self.world.options.hooks.session_list.as_ref()?;
        let entry = list().into_iter().find(|entry| entry.id == id)?;
        (!entry.title.trim().is_empty()).then(|| entry.title.clone())
    }

    /// Open a fresh-session tab (gh #209: Ctrl+T / `[+]`). Refused
    /// mid-turn; without a host hook it names the absence.
    pub(crate) fn open_tab(&mut self) {
        if self.turn_running {
            self.world.notice =
                Some("a turn is running; finish or cancel it before opening a tab".to_string());
            return;
        }
        let Some(create) = self.world.options.hooks.new_session.clone() else {
            self.world.notice = Some("opening a tab is not available in this host".to_string());
            return;
        };
        let Some(id) = create() else {
            self.world.notice = Some("the host could not open a session".to_string());
            return;
        };
        self.save_tab();
        self.tabs.push(SessionTab {
            session_id: id.clone(),
            // Fresh by construction: the listing hides empty
            // sessions, so no lookup could name it yet.
            title: "untitled".to_string(),
            editor: String::new(),
            scroll: 0,
        });
        self.prev_tab = self.active_tab;
        self.active_tab = self.tabs.len() - 1;
        self.enter_tab_session(&id);
    }

    /// Switch to tab `index` (gh #209): save, replay the host's
    /// records, restore the draft and scroll. Refused mid-turn.
    pub(crate) fn select_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        if self.turn_running {
            self.world.notice =
                Some("a turn is running; finish or cancel it before switching tabs".to_string());
            return;
        }
        self.save_tab();
        self.prev_tab = self.active_tab;
        self.active_tab = index;
        let id = self.tabs[index].session_id.clone();
        self.enter_tab_session(&id);
    }

    /// Cycle one tab forward (`dir` +1) or back (-1), wrapping.
    pub(crate) fn cycle_tab(&mut self, dir: i8) {
        if self.tabs.len() < 2 {
            return;
        }
        let next =
            (self.active_tab as i64 + dir as i64).rem_euclid(self.tabs.len() as i64) as usize;
        self.select_tab(next);
    }

    /// Close the active tab (gh #209: Ctrl+W on empty / `×`). The
    /// session persists on disk - only the unsent draft goes with
    /// the tab. One tab left refuses; the turn refusal guards a
    /// running stream. Lands on the previously active tab.
    pub(crate) fn close_tab(&mut self) {
        if self.turn_running {
            self.world.notice =
                Some("a turn is running; finish or cancel it before closing tabs".to_string());
            return;
        }
        if self.tabs.len() < 2 {
            self.world.notice = Some("only one tab open".to_string());
            return;
        }
        self.tabs.remove(self.active_tab);
        let landed = self.prev_tab.min(self.tabs.len() - 1);
        self.active_tab = landed;
        self.prev_tab = landed;
        let id = self.tabs[landed].session_id.clone();
        self.enter_tab_session(&id);
    }

    /// Enter a tab's session: replay the host's records, restore the
    /// draft and scroll, refresh the title (gh #209). A new session
    /// replays its (empty) records like any switch, so the
    /// transcript never shows the wrong session.
    fn enter_tab_session(&mut self, id: &str) {
        // gh #233: a tab can run another provider's session.
        self.invalidate_models();
        // A switch lands with a clean dock: pickers, search, and the
        // old notice belong to the tab left behind.
        self.close_pickers();
        self.world.notice = None;
        match self.world.options.hooks.switch_session.as_ref() {
            Some(switch) => match switch(id) {
                Some(records) => {
                    self.replay_records(&records);
                }
                None => {
                    self.world.notice = Some(format!("cannot open session {id}"));
                    return;
                }
            },
            None => {
                self.world.notice = Some(format!("resume this branch with: lca --resume {id}"));
                return;
            }
        }
        if let Some(title) = self.resolve_tab_title(id)
            && let Some(tab) = self.tabs.get_mut(self.active_tab)
        {
            title.clone_into(&mut tab.title);
        }
        let title = self
            .tabs
            .get(self.active_tab)
            .map(|tab| tab.title.clone())
            .unwrap_or_default();
        let saved = self
            .tabs
            .get(self.active_tab)
            .map(|tab| (tab.editor.clone(), tab.scroll));
        if let Some((editor, scroll)) = saved {
            self.editor.set_text(&editor);
            // Restore the reader's place against the fresh
            // transcript (saturating lands short views at bottom).
            let width = self.world.size.0;
            let height = self.world.size.1;
            let total = self.transcript_len(width);
            let window = self.window_height(width, height);
            let target = total.saturating_sub(window.saturating_add(scroll as usize));
            self.jump_target = Some(target);
        }
        // Scrollback mode cannot erase: the switch appends its
        // transition divider to the scrollback (the issue's rule).
        // Fullscreen swaps in place, so it stays quiet.
        if !self.screen_mode {
            self.transcript
                .push_raw(format!("── Switched to Session: {title} ({id}) ──"));
        }
    }

    /// Close every picker and the search (gh #209): transient UI
    /// belongs to the tab left behind.
    fn close_pickers(&mut self) {
        self.model_picker = None;
        self.tree_picker = None;
        self.resume_picker = None;
        self.fork_picker = None;
        self.scoped_models_picker = None;
        self.grants_picker = None;
        self.trust_picker = None;
        self.settings_picker = None;
        self.thinking_picker = None;
        self.theme_picker = None;
        self.search = None;
    }

    /// Build the widget from the tabs (gh #209): titles, the dirty
    /// dot for an unsent draft, closable past one tab. Pure - the bar
    /// renders from this every frame, no stored widget to drift.
    pub(crate) fn tabs_widget(&self) -> lca_tui::widgets::tabs::TabsWidget {
        let mut widget = lca_tui::widgets::tabs::TabsWidget {
            orientation: lca_tui::widgets::tabs::TabOrientation::Horizontal,
            items: Vec::with_capacity(self.tabs.len()),
            active_index: self.active_tab,
            show_add_button: self.world.options.hooks.new_session.is_some(),
            hovered: None,
        };
        for (index, tab) in self.tabs.iter().enumerate() {
            let draft = if index == self.active_tab {
                !self.editor.text().trim().is_empty()
            } else {
                !tab.editor.trim().is_empty()
            };
            widget.items.push(TabItem {
                id: tab.session_id.clone(),
                title: tab.title.clone(),
                badge: None,
                is_generating: false,
                is_dirty: draft,
                closable: self.tabs.len() > 1,
            });
        }
        widget
    }

    /// Route a mouse event to the tab bar (gh #209): row 0 presses
    /// preview, releases commit, anything else falls through.
    /// Returns `true` when the gesture landed on the bar.
    pub(crate) fn handle_tab_mouse(&mut self, event: &MouseEvent) -> bool {
        let width = self.world.size.0;
        match event {
            MouseEvent::Down {
                col,
                row,
                button: MouseButton::Left,
                ..
            } => self.tab_hit(*col, *row, width).is_some(),
            MouseEvent::Up {
                col,
                row,
                button: MouseButton::Left,
                ..
            } => match self.tab_hit(*col, *row, width) {
                Some(hit) => self.tab_gesture(hit),
                None => false,
            },
            _ => false,
        }
    }

    /// Handle a tab-bar gesture (gh #209): select, close, or add.
    /// Returns `true` when the gesture landed on the bar.
    pub(crate) fn tab_gesture(&mut self, hit: TabHit) -> bool {
        match hit {
            TabHit::Tab(index) => {
                self.select_tab(index);
                true
            }
            TabHit::Close(index) => {
                if index == self.active_tab {
                    self.close_tab();
                } else {
                    // A background tab closes without switching: its
                    // draft goes with it, the view never moves.
                    if !self.turn_running && self.tabs.len() > 1 {
                        self.tabs.remove(index);
                        if index < self.active_tab {
                            self.active_tab -= 1;
                        }
                        if index < self.prev_tab || self.prev_tab >= self.tabs.len() {
                            self.prev_tab = self.prev_tab.min(self.tabs.len() - 1);
                        }
                    }
                }
                true
            }
            TabHit::Add => {
                self.open_tab();
                true
            }
        }
    }

    /// The dock's tab row in scrollback mode (gh #209): below the
    /// footer, never in the scrollback. One tab shows a right-aligned
    /// `[+]`; more show the full bar.
    pub(crate) fn tab_bar_main(&self, width: u16) -> Vec<String> {
        if self.tabs.len() < 2 {
            if self.world.options.hooks.new_session.is_none() {
                return Vec::new();
            }
            let pad = (width as usize).saturating_sub(3);
            return vec![format!("{}{}", " ".repeat(pad), "[+]")];
        }
        self.tabs_widget().render(width)
    }

    /// The fullscreen tab row (gh #209): row 0, top-anchored. One tab
    /// shows a top-left `[+]`; more show the full bar.
    pub(crate) fn tab_bar_alt(&self, width: u16) -> Vec<String> {
        if self.tabs.len() < 2 {
            if self.world.options.hooks.new_session.is_none() {
                return Vec::new();
            }
            return vec!["[+]".to_string()];
        }
        self.tabs_widget().render(width)
    }

    /// Fullscreen rows above the transcript (gh #209): the tab bar
    /// owns row 0 when it shows, else zero. Mouse math subtracts
    /// this to reach transcript rows; the bar is dock-bottom in
    /// scrollback mode, so nothing shifts there.
    pub(crate) fn tab_top(&self, width: u16) -> u16 {
        if !self.screen_mode {
            return 0;
        }
        self.tab_bar_alt(width).len() as u16
    }

    /// Map a fullscreen cell to the tab bar (gh #209): row 0 only,
    /// `None` anywhere else (single `[+]` included - it spans the
    /// row's first cells).
    pub(crate) fn tab_hit(&self, col: u16, row: u16, width: u16) -> Option<TabHit> {
        if row != 0 {
            return None;
        }
        let can_add = self.world.options.hooks.new_session.is_some();
        if self.tabs.len() < 2 {
            if !can_add {
                return None;
            }
            // The lone `[+]` occupies the row's first three cells.
            return (col < 3).then_some(TabHit::Add);
        }
        self.tabs_widget().tab_at(col, 0, width)
    }

    /// Tab-bar keys (gh #209): new, close (empty composer only, so
    /// Ctrl+W keeps deleting words with text), cycle, and Alt+digit
    /// direct jumps. Pickers own the keyboard while open (above),
    /// the editor owns the rest below. Returns `true` when handled.
    pub(crate) fn handle_tab_key(&mut self, data: &str, key: Option<&str>) -> bool {
        if self.keybindings.matches(data, "app.tabs.new") {
            self.open_tab();
            return true;
        }
        if self.keybindings.matches(data, "app.tabs.close") && self.editor.text().trim().is_empty()
        {
            self.close_tab();
            return true;
        }
        if self.keybindings.matches(data, "app.tabs.next") {
            self.cycle_tab(1);
            return true;
        }
        if self.keybindings.matches(data, "app.tabs.prev") {
            self.cycle_tab(-1);
            return true;
        }
        if let Some(index) = tab_digit(key) {
            self.select_tab(index);
            return true;
        }
        false
    }

    /// Refresh the active tab's title (gh #209): a turn can land the
    /// session's first message, which is what titles resolve from.
    pub(crate) fn refresh_active_tab_title(&mut self) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let id = tab.session_id.clone();
            if let Some(title) = self.resolve_tab_title(&id)
                && let Some(tab) = self.tabs.get_mut(self.active_tab)
            {
                title.clone_into(&mut tab.title);
            }
        }
    }

    /// Seed the first tab (gh #209): the current session when the
    /// host names one, else an empty anchor. Called once from
    /// `Chat::new`; later tabs join through `open_tab`.
    pub(crate) fn seed_tabs(&mut self) {
        let id = self
            .world
            .options
            .hooks
            .current_session_id
            .as_ref()
            .map(|current| current())
            .unwrap_or_default();
        let title = if id.trim().is_empty() {
            "untitled".to_string()
        } else {
            self.resolve_tab_title(&id)
                .unwrap_or_else(|| "untitled".to_string())
        };
        self.tabs = vec![SessionTab {
            session_id: id,
            title,
            editor: String::new(),
            scroll: 0,
        }];
        self.active_tab = 0;
        self.prev_tab = 0;
    }
}

/// An Alt+digit direct jump (gh #209): `alt+1` selects tab 0.
/// Literals like the tree picker's keys - nine rebindable actions
/// would only rename them.
pub(crate) fn tab_digit(key: Option<&str>) -> Option<usize> {
    let name = key?;
    let digit = name.strip_prefix("alt+")?;
    if digit.len() == 1 && ('1'..='9').contains(&digit.chars().next()?) {
        digit.parse::<usize>().ok().map(|n| n - 1)
    } else {
        None
    }
}
