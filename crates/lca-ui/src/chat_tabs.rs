//! Session tabs (gh #209): one tab per conversation session on the
//! #208 Tabs widget chrome, with per-tab composer and scroll state.
//! Split from `chat.rs` (the 1,200-line ceiling).
//!
//! The transcript is never *saved* per tab: switching replays the host's
//! records (the R3 switch machinery), so the log stays the single source of
//! truth and a switch is always fresh. What a tab keeps is the composer
//! draft and the scroll restore point.
//!
//! ## Background turns (gh #462)
//!
//! A turn belongs to the tab it started on, not to the interface. Three
//! facts make a mid-turn tab switch safe, and all three are needed:
//!
//! 1. **The turn's session binding is pinned when the turn starts.** The
//!    host's `switch_session` rewrites a shared "current session" cell; the
//!    turn worker reads it once, at start, in
//!    `turn_worker`/`establish_turn_binding`. Before #462 a switch mid-turn
//!    would have redirected the running turn's *writes* to the new session,
//!    which is silent cross-session corruption - the reason the refusal
//!    existed.
//! 2. **A tab owns its stream.** `SessionTab` carries the live transcript,
//!    pending steering queue, separator, turn-status line, and the metering
//!    that a turn writes into. The visible tab's copies are swapped in and
//!    out on a switch (`swap_tab_state`), so a background turn's deltas land
//!    in its own transcript and never in the tab the reader is looking at.
//! 3. **Cancellation is scoped to the visible tab.** `Ctrl+C` cancels a turn
//!    the user can see; a background tab's turn is cancelled by switching to
//!    it first. Silently killing work nobody is looking at is how a user
//!    loses twenty minutes of a `cargo test`.
//!
//! `open_tab` is allowed mid-turn (a fresh tab replays its own empty
//! records). `close_tab` refuses only for the *active* tab with a live turn:
//! removing the row the user's own stream is landing in would hide work in
//! progress. A background tab closes freely.

use lca_tui::engine::core::{MouseButton, MouseEvent};
use lca_tui::widgets::tabs::{TabHit, TabItem};

use super::chat::Chat;

/// One conversation tab (gh #209).
#[derive(Debug)]
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
    /// A turn is streaming into this tab (gh #462): the tab bar's pulsing
    /// marker, and the reason `Ctrl+C` here cancels *this* tab's turn.
    pub running: bool,
    /// A turn settled while this tab was unfocused (gh #462): the tab bar's
    /// unread marker, cleared by focusing the tab.
    pub unread: bool,
    /// This tab's own live transcript (gh #462). Present only while a turn
    /// is writing into it - an inactive tab with no turn in flight holds
    /// its records on disk, not a second copy in memory.
    pub(crate) stream: Option<TabStream>,
}

/// The per-tab state a background turn writes into (gh #462).
///
/// The visible tab's copies live on `Chat` itself, because every renderer,
/// key handler, and command reaches for `self.transcript`, `self.pending`,
/// and friends directly. Swapping them in and out on a switch keeps that
/// true - one code path, no "which transcript?" branch in forty call
/// sites - and confines the multiplexing to two functions.
pub struct TabStream {
    /// The turn's transcript.
    pub transcript: crate::transcript::Transcript,
    /// Steering messages queued into this tab's turn (ADR-0038).
    pub pending: Vec<crate::chat::PendingMessage>,
    /// The separator row (spinner, retry countdown, compaction reason).
    pub separator: crate::separator::Separator,
    /// The status line's turn cue.
    pub turn_status: Option<crate::state::TurnStatusLine>,
    /// The turn's prompt size, for the footer's context cell.
    pub context_used: u64,
    /// The turn's token metering (gh #464).
    pub meter: crate::footer::meter::StreamMeter,
    /// This turn's usage so far (gh #327's live view).
    pub turn_usage: lca_protocol::Usage,
    /// The tab's session totals (gh #327's authoritative sum). Per tab because
    /// it is per *session*: two tabs are two conversations, and a background
    /// turn's spend must land on its own session rather than on whatever the
    /// reader happens to be looking at (gh #462).
    pub usage: lca_protocol::Usage,
}

impl std::fmt::Debug for TabStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabStream")
            .field("entries", &self.transcript.entries().len())
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl Default for TabStream {
    fn default() -> Self {
        TabStream {
            transcript: crate::transcript::Transcript::new(),
            pending: Vec::new(),
            separator: crate::separator::Separator::new(),
            turn_status: None,
            context_used: 0,
            meter: crate::footer::meter::StreamMeter::default(),
            turn_usage: lca_protocol::Usage::default(),
            usage: lca_protocol::Usage::default(),
        }
    }
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

    /// Open a fresh-session tab (gh #209: Ctrl+T / `[+]`).
    ///
    /// Allowed mid-turn (gh #462): opening replays the new session's own
    /// (empty) records into a fresh tab, and the running turn keeps its own
    /// `TabStream`, so nothing is shared and nothing is lost. Refusing here
    /// is what made a 30-second `cargo test` block the whole interface.
    pub(crate) fn open_tab(&mut self) {
        // Gh #462: minting a session necessarily makes it the host's current
        // one (that is what `new_session` does), so it cannot be deferred the
        // way a switch can - there is no id to defer to until it exists. A
        // narrow refusal, and the same one the tab bar names.
        if self.turn_running {
            self.say(
                "a turn is running; switch tabs to work elsewhere, or wait for it to finish"
                    .to_string(),
            );
            return;
        }
        let Some(create) = self.world.options.hooks.new_session.clone() else {
            self.say("opening a tab is not available in this host".to_string());
            return;
        };
        let Some(id) = create() else {
            self.say("the host could not open a session".to_string());
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
            running: false,
            unread: false,
            stream: None,
        });
        self.prev_tab = self.active_tab;
        self.active_tab = self.tabs.len() - 1;
        self.enter_tab_session(&id);
    }

    /// Switch to tab `index` (gh #209): save, replay the host's records,
    /// restore the draft and scroll.
    ///
    /// Allowed mid-turn (gh #462). The tab being left keeps the turn it owns:
    /// its live transcript, queue, separator, and meter travel with it, and
    /// the incoming tab's own state is swapped back in. Switching back later
    /// finds the turn exactly where it was.
    pub(crate) fn select_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        self.save_tab();
        // gh #462: park the visible tab's live state on the tab, take the
        // incoming tab's.
        self.live_into(self.active_tab);
        self.prev_tab = self.active_tab;
        self.active_tab = index;
        // Focusing a tab clears its unread marker (gh #462).
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.unread = false;
        }
        let id = self.tabs[index].session_id.clone();
        if self.turn_running {
            // Gh #462: the display moves, the host's binding does not. The
            // turn worker reads the host's current-session cell lazily on its
            // own thread; rewriting it here would land the running turn's
            // records in the other session's log. `enter_tab_session` is
            // deferred with it (its only job is the host replay), so the
            // incoming tab's own `TabStream` is what paints.
            self.deferred_host_session = Some(id);
            self.live_from(index);
            return;
        }
        self.enter_tab_session(&id);
        self.live_from(index);
    }

    /// Move `self`'s live turn state into `tab`'s slot (gh #462).
    ///
    /// Exactly one place in the interface may hold a tab's live transcript at
    /// a time: the active tab's is on `Chat` (so every renderer and key
    /// handler reaches it without a "which tab?" branch), and every other
    /// tab's is in its own `TabStream`. These two functions are the only
    /// transition, which is the whole of the multiplexing.
    ///
    /// The moves are `mem::replace`/`swap`, never clones: the transcript is
    /// the same allocation travelling, so a switch or a routed event costs a
    /// pointer swap rather than a copy of the conversation.
    fn live_into(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let mut stream = tab.stream.take().unwrap_or_default();
        std::mem::swap(&mut stream.transcript, &mut self.transcript);
        std::mem::swap(&mut stream.pending, &mut self.pending);
        std::mem::swap(&mut stream.separator, &mut self.separator);
        std::mem::swap(&mut stream.turn_status, &mut self.turn_status);
        std::mem::swap(&mut stream.context_used, &mut self.footer.context_used);
        std::mem::swap(&mut stream.meter, &mut self.meter);
        std::mem::swap(&mut stream.turn_usage, &mut self.turn_usage);
        std::mem::swap(&mut stream.usage, &mut self.usage);
        tab.stream = Some(stream);
    }

    /// Take `tab`'s live turn state back, leaving what `self` held in the tab
    /// (gh #462). The exact inverse of [`Chat::live_into`].
    fn live_from(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(mut stream) = tab.stream.take() else {
            // No live state: a tab that never ran a turn. The replay
            // `enter_tab_session` just performed stands, with the turn-scoped
            // fields at rest (they belong to the tab left behind).
            self.pending.clear();
            self.separator.idle();
            self.turn_status = None;
            self.footer.context_used = 0;
            self.meter = Default::default();
            self.turn_usage = Default::default();
            self.usage = Default::default();
            return;
        };
        std::mem::swap(&mut stream.transcript, &mut self.transcript);
        std::mem::swap(&mut stream.pending, &mut self.pending);
        std::mem::swap(&mut stream.separator, &mut self.separator);
        std::mem::swap(&mut stream.turn_status, &mut self.turn_status);
        std::mem::swap(&mut stream.context_used, &mut self.footer.context_used);
        std::mem::swap(&mut stream.meter, &mut self.meter);
        std::mem::swap(&mut stream.turn_usage, &mut self.turn_usage);
        std::mem::swap(&mut stream.usage, &mut self.usage);
        // What came back is the tab's previous live state; an exhausted one is
        // dropped rather than kept as a stale snapshot (its content is the
        // log's, and the next switch replays from there).
        if self.turn_running && self.turn_tab == Some(index) {
            tab.stream = Some(stream);
        } else {
            tab.stream = None;
        }
    }

    /// Run `f` against the state of tab `index`, whichever tab is visible
    /// (gh #462).
    ///
    /// This is what makes a background turn's deltas land in its own
    /// transcript. The visible tab's state is parked on itself, `index`'s is
    /// swapped in, `f` runs, and both are put back - so the reader's screen is
    /// byte-identical afterwards while the turn's owner moved forward.
    ///
    /// When `index` *is* the visible tab there is nothing to do and `f` runs
    /// in place, which is the ordinary case and costs nothing.
    pub(crate) fn with_tab_state<R>(&mut self, index: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        if index == self.active_tab || index >= self.tabs.len() {
            return f(self);
        }
        // Park the visible tab, load the owner, run, then reverse exactly.
        self.live_into(self.active_tab);
        self.live_from(index);
        let out = f(self);
        self.live_into(index);
        self.live_from(self.active_tab);
        out
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
    /// the tab. One tab left refuses.
    ///
    /// Refused while the *active* tab has a turn in flight (gh #462): its
    /// stream is landing in the row about to be removed, so closing would
    /// hide work the user asked for. That is the one narrow refusal left, and
    /// it names the way out. A background tab closes freely - see
    /// [`Chat::tab_gesture`].
    pub(crate) fn close_tab(&mut self) {
        if self.tabs.len() < 2 {
            self.say("only one tab open".to_string());
            return;
        }
        let running = self.turn_running || self.tabs[self.active_tab].running;
        if running {
            self.say(
                "this tab's turn is still running; Ctrl+C cancels it, or switch tabs first"
                    .to_string(),
            );
            return;
        }
        // The tab being removed cannot keep a live stream: its turn (if any)
        // is already settled, so the stream is only a cached transcript.
        self.tabs.remove(self.active_tab);
        let landed = self.prev_tab.min(self.tabs.len() - 1);
        self.active_tab = landed;
        self.prev_tab = landed;
        let id = self.tabs[landed].session_id.clone();
        self.enter_tab_session(&id);
        self.live_from(landed);
    }

    /// Enter a tab's session: replay the host's records, restore the
    /// draft and scroll, refresh the title (gh #209). A new session
    /// replays its (empty) records like any switch, so the
    /// transcript never shows the wrong session.
    fn enter_tab_session(&mut self, id: &str) {
        // gh #233: a tab can run another provider's session.
        self.invalidate_models();
        // A switch lands with a clean dock: pickers, search, and
        // toasts belong to the tab left behind.
        self.close_pickers();
        self.toasts.clear();
        match self.world.options.hooks.switch_session.as_ref() {
            Some(switch) => match switch(id) {
                Some(records) => {
                    self.replay_records(&records);
                }
                None => {
                    self.say(format!("cannot open session {id}"));
                    return;
                }
            },
            None => {
                self.say(format!("resume this branch with: lca --resume {id}"));
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

    /// Close every transient overlay (gh #209, gh #333): transient UI belongs to the
    /// tab left behind.
    ///
    /// The list lives in [`crate::overlay_registry`] and this function is generated
    /// from it, so it cannot hold a stale copy. See that module for why a
    /// hand-maintained list was the defect and what the guard checks.
    fn close_pickers(&mut self) {
        crate::overlay_registry::close_tab_overlays(self);
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
            // Gh #462: the running marker is per tab, so a background turn
            // pulses in the bar while the reader works elsewhere; the unread
            // marker (`badge`) says it settled off-screen.
            widget.items.push(TabItem {
                id: tab.session_id.clone(),
                title: tab.title.clone(),
                badge: tab.unread.then(|| "•".to_string()),
                is_generating: tab.running,
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
                    // A background tab closes without switching: its draft
                    // goes with it and the view never moves.
                    //
                    // Gh #462: allowed mid-turn. The one thing that would be
                    // lost is a *running* background turn - its stream keeps
                    // landing in a transcript about to be dropped, so the work
                    // would be invisible and unrecoverable. That is the same
                    // destructive question as closing the active tab, so it
                    // takes the same narrow refusal.
                    let runs = self.tabs.get(index).is_some_and(|tab| tab.running);
                    if runs {
                        self.toast(
                            "that tab's turn is still running; switch to it and press Ctrl+C first"
                                .to_string(),
                        );
                    } else if self.tabs.len() > 1 {
                        let owner = self.turn_tab;
                        self.tabs.remove(index);
                        if index < self.active_tab {
                            self.active_tab -= 1;
                        }
                        // The removed row shifts every index after it; the
                        // turn's owner index must move with it or a later
                        // routed event lands on the wrong tab (gh #462).
                        if let Some(owner) = owner {
                            self.turn_tab = match owner.cmp(&index) {
                                std::cmp::Ordering::Greater => Some(owner - 1),
                                std::cmp::Ordering::Equal => None,
                                std::cmp::Ordering::Less => Some(owner),
                            };
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
            running: false,
            unread: false,
            stream: None,
        }];
        self.active_tab = 0;
        self.prev_tab = 0;
    }
}

impl Chat {
    /// Record that the running turn has stopped (gh #462): the owning tab
    /// stops reporting a running marker, gains the unread marker when it
    /// settled off-screen, and the interface forgets the owner.
    ///
    /// Called from `TurnState::reap`, which is the one place that knows the
    /// worker thread has really finished - a turn can end without emitting
    /// `TurnEnded` (a join failure, an early error), and the tab bar must not
    /// pulse forever because of it.
    pub(crate) fn settle_turn_tab(&mut self) {
        let Some(owner) = self.turn_tab.take() else {
            return;
        };
        if let Some(tab) = self.tabs.get_mut(owner) {
            tab.running = false;
            if owner != self.active_tab {
                tab.unread = true;
            }
        }
    }

    /// Land a session switch that arrived while a turn was running (gh #462).
    ///
    /// Called at settle, once the worker has stopped writing. `switch_session`
    /// replays the records, which the incoming tab already holds in its
    /// `TabStream` - so the replay lands in the parked copy and is discarded,
    /// exactly as a normal switch would leave it. What matters is that the
    /// host's binding now names the visible tab's session, so the *next* turn
    /// writes where the user expects.
    pub(crate) fn apply_deferred_host_switch(&mut self) {
        let Some(id) = self.deferred_host_session.take() else {
            return;
        };
        if !self.turn_running {
            self.enter_tab_session(&id);
        }
    }

    /// Whether the visible tab owns the running turn (gh #462).
    ///
    /// `Ctrl+C` cancels a turn only when this is true. The rule exists because
    /// a background turn is invisible: cancelling it from another tab destroys
    /// work the user cannot see, and the brief would rather have a narrow
    /// refusal than a silent one. Switching to the owning tab makes it
    /// cancellable again, which is the discoverable answer.
    pub(crate) fn cancellable_turn_visible(&self) -> bool {
        self.turn_running && self.turn_tab.is_none_or(|owner| owner == self.active_tab)
    }
}

impl Chat {
    // ---------------------------------------------------------------------
    // Read-only accessors for the cross-crate guard
    // (`tests/regressions/462-background-turns-across-tabs.rs`).
    //
    // `UiOptions` has no `Default`, so a second copy of that forty-field
    // fixture in the regression crate would drift; the guard therefore drives
    // a real `Chat` and reads these. They expose state, never behaviour - no
    // caller inside the crate needs them, and every one is a field read.
    // ---------------------------------------------------------------------

    /// How many tabs are open.
    #[doc(hidden)]
    pub fn tabs_len(&self) -> usize {
        self.tabs.len()
    }

    /// The visible tab's index.
    #[doc(hidden)]
    pub fn active_tab_index(&self) -> usize {
        self.active_tab
    }

    /// Which tab owns the running turn, if one is running (gh #462).
    #[doc(hidden)]
    pub fn turn_tab_index(&self) -> Option<usize> {
        self.turn_tab
    }

    /// Whether a turn is running at all.
    #[doc(hidden)]
    pub fn turn_is_running_hint(&self) -> bool {
        self.turn_running
    }

    /// Whether the visible tab owns the running turn (gh #462).
    #[doc(hidden)]
    pub fn turn_is_owned_by_visible_tab_hint(&self) -> bool {
        self.cancellable_turn_visible()
    }

    /// Begin a turn without going through a submit (gh #462).
    #[doc(hidden)]
    pub fn begin_turn_pub(&mut self, steer: lca_protocol::SteerQueue) {
        self.begin_turn(steer);
    }

    /// Deliver one turn event (gh #462).
    #[doc(hidden)]
    pub fn event(&mut self, event: lca_protocol::TurnEvent) {
        self.on_turn_event(event);
    }

    /// Route one key and report the action it produced (gh #462).
    #[doc(hidden)]
    pub fn press(&mut self, data: &str) -> crate::state::Action {
        self.handle_key(data)
    }

    /// The rendered frame at 100 columns, ANSI stripped (gh #462).
    #[doc(hidden)]
    pub fn rendered_frame(&self) -> String {
        self.render(100)
            .iter()
            .map(|line| lca_tui::engine::text::strip_terminal_sequences(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The tab bar as text, ANSI stripped (gh #462).
    #[doc(hidden)]
    pub fn tab_bar_text(&self) -> String {
        self.tab_bar_main(100)
            .iter()
            .map(|line| lca_tui::engine::text::strip_terminal_sequences(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The visible tab's session cost (gh #462).
    #[doc(hidden)]
    pub fn session_cost_for_visible_tab(&self) -> f64 {
        self.usage.cost
    }

    /// Fold a completed turn's usage into the tab that owns it (gh #462):
    /// exactly what `TurnState::reap` does.
    #[doc(hidden)]
    pub fn fold_owned_turn_for_test(&mut self, usage: &lca_protocol::Usage) {
        let owner = self.turn_tab.unwrap_or(self.active_tab);
        self.with_tab_state(owner, |chat| chat.fold_turn_usage(usage));
    }

    /// Record that the running turn stopped (gh #462), as `reap` does.
    #[doc(hidden)]
    pub fn settle_owned_turn_for_test(&mut self) {
        self.settle_turn_tab();
        self.turn_running = false;
    }

    /// Every notice the interface has surfaced (gh #462): floating toasts and
    /// notice lines already in the visible transcript.
    #[doc(hidden)]
    pub fn notice_log(&self) -> Vec<String> {
        let mut out: Vec<String> = self.toasts.iter().map(|toast| toast.text.clone()).collect();
        out.extend(self.transcript.notices());
        out
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
