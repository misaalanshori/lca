//! Session tab rows (gh #209): open/switch/close, per-tab composer
//! isolation, anchoring both modes, and the scrollback divider.
//!
//! Split from `chat_tests.rs` for the workspace's 1,200-line file
//! ceiling. Fixtures come from the parent.

use super::super::Chat;
use super::{chat, options, strip};
use std::sync::Arc;

/// A two-session host: switch replays per-session records, titles
/// resolve, new sessions mint `s3`, `s4`, ...
fn tabbed_options() -> crate::state::UiOptions {
    use std::sync::Mutex;
    let mut opts = options();
    let minted = Arc::new(Mutex::new(3u32));
    let next = minted.clone();
    opts.hooks.new_session = Some(Arc::new(move || {
        let mut n = next.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let id = format!("s{}", *n);
        *n += 1;
        Some(id)
    }));
    opts.hooks.switch_session = Some(Arc::new(|id: &str| {
        Some(vec![lca_protocol::Record::User {
            v: lca_protocol::FORMAT_VERSION,
            ts: 1,
            id: format!("{id}-u"),
            parent: None,
            content: format!("hello from {id}"),
            attachments: Vec::new(),
            queue: None,
        }])
    }));
    opts.hooks.session_list = Some(Arc::new(|| {
        ["s1", "s2", "s3"]
            .into_iter()
            .map(|id| crate::resume::SessionEntry {
                id: id.to_string(),
                title: format!("title {id}"),
                messages: 1,
                age: "now".to_string(),
                labels: Vec::new(),
            })
            .collect()
    }));
    opts.hooks.current_session_id = Some(Arc::new(|| "s1".to_string()));
    opts
}

// Verifies: gh #209 - Ctrl+T mints a tab through the host: two tabs,
// the bar shows, the new tab is active with a fresh composer.
#[test]
fn ctrl_t_opens_a_second_tab() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let mut chat = Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()));
    chat.handle_key("\x14");
    assert_eq!(chat.tabs.len(), 2, "a tab opened");
    assert_eq!(chat.active_tab, 1, "the new tab is active");
    assert_eq!(chat.tabs[1].session_id, "s3");
    assert_eq!(chat.tabs[1].title, "title s3");
    assert_eq!(chat.editor.text(), "", "fresh composer");
    let bar = chat.tab_bar_main(80).join("\n");
    assert!(
        bar.contains("title s1") && bar.contains("title s3"),
        "both show:\n{bar}"
    );
}

// Verifies: gh #209 - without a host hook the key names the absence
// instead of inventing a session.
#[test]
fn ctrl_t_without_a_host_names_the_absence() {
    let mut chat = chat();
    chat.handle_key("\x14");
    assert_eq!(chat.tabs.len(), 1, "the seed tab stands, no tab invented");
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("opening a tab is not available in this host")
    );
}

// Verifies: gh #209 - switching preserves each tab's composer: drafts
// ride their tab, transcripts replay from the host.
#[test]
fn switch_preserves_each_tabs_composer() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let mut chat = Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()));
    // Tab one starts seeded; draft on it.
    for c in "draft one".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\x14"); // tab two
    for c in "draft two".chars() {
        chat.handle_key(&c.to_string());
    }
    // Back to tab one (Alt+1), then forward (Alt+2).
    chat.handle_key("\x1b1");
    assert_eq!(chat.active_tab, 0);
    assert_eq!(chat.editor.text(), "draft one", "tab one's draft");
    let text = strip(&chat.render(80)).join("\n");
    assert!(text.contains("hello from s1"), "tab one's records:\n{text}");
    chat.handle_key("\x1b2");
    assert_eq!(chat.active_tab, 1);
    assert_eq!(chat.editor.text(), "draft two", "tab two's draft");
}

// Verifies: gh #209 - closing returns to the previous tab; the last
// close collapses the bar; Ctrl+W with text deletes words instead.
#[test]
fn close_returns_to_previous_and_collapses() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let mut chat = Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()));
    chat.handle_key("\x14"); // tab two active
    chat.handle_key("\x1b1"); // back to one
    chat.handle_key("\x1b2"); // forward to two (previous = one)
    chat.handle_key("\x17"); // Ctrl+W on empty: close
    assert_eq!(chat.tabs.len(), 1, "tab two closed");
    assert_eq!(chat.active_tab, 0, "back on the previous tab");
    assert!(
        chat.tab_bar_main(80).join("\n").contains("[+]"),
        "collapsed to the add button"
    );
    chat.handle_key("\x17");
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("only one tab open"),
        "the last tab refuses"
    );
    // With text, Ctrl+W deletes a word - the tab survives.
    for c in "hello world".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\x17");
    assert_eq!(chat.tabs.len(), 1);
    assert_eq!(chat.editor.text(), "hello ", "word deleted, tab kept");
}

// Verifies: gh #462 - the mid-turn tab contract, replacing the refusal this
// row used to pin (SANCTIONED CHANGE, named in this cycle's brief).
//
// What it asserted: `open_tab`, `select_tab`, and `close_tab` each refuse
// mid-turn with a notice naming the turn. That refusal defeated the primary
// reason tabs exist - a 30-120 s `cargo test` blocked the whole interface - so
// gh #462 replaced it with a per-tab turn.
//
// What it asserts now: a switch and an open are allowed mid-turn and cancel
// nothing; the one narrow refusal left is closing a tab whose *own* turn is
// running, because that stream is landing in the row about to be removed.
#[test]
fn tab_moves_during_a_turn() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let mut chat = Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()));
    // A second tab first, so the switch half of this row has somewhere to go.
    chat.handle_key("\x14");
    assert_eq!(chat.tabs.len(), 2);
    chat.handle_key("\x1b1");
    chat.begin_turn(lca_protocol::steer_queue()); // tab 0 owns it
    // Switching away mid-turn is allowed, and cancels nothing.
    chat.handle_key("\x1b2");
    assert_eq!(chat.active_tab, 1, "switched mid-turn");
    assert!(chat.turn_running, "and the turn kept running");
    // Opening mid-turn is REFUSED (gh #462): minting a session necessarily
    // makes it the host's current one, and the host's current-session cell is
    // what the running turn worker reads. Switching can be deferred; creating
    // cannot.
    chat.handle_key("\x14");
    assert_eq!(chat.tabs.len(), 2, "no tab opened mid-turn");
    assert!(
        chat.last_notice()
            .as_deref()
            .is_some_and(|notice| notice.contains("switch tabs")),
        "it names the alternative: {:?}",
        chat.last_notice()
    );
    // The narrow refusal: closing the tab that owns the running turn. The
    // owning tab here is tab 0 (where `begin_turn` ran), so switch to it and
    // try to close it.
    chat.handle_key("\x1b1");
    chat.handle_key("\x17");
    assert_eq!(chat.tabs.len(), 2, "the owning tab was not closed");
    assert!(
        chat.last_notice()
            .as_deref()
            .is_some_and(|notice| notice.contains("still running")),
        "it names the reason: {:?}",
        chat.last_notice()
    );
}

// Verifies: gh #209 - fullscreen paints the bar on row 0 and shrinks
// the transcript window past it; single shows top-left `[+]`.
#[test]
fn fullscreen_bar_anchors_row_zero() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let mut chat = Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()));
    chat.screen_mode = true;
    let frame = chat.viewport(80, 24, 0);
    assert_eq!(
        frame[0],
        "[+]",
        "single: top-left add:\n{}",
        frame.join("\n")
    );
    chat.handle_key("\x14");
    let frame = chat.viewport(80, 24, 0);
    assert!(
        frame[0].contains("title s1") && frame[0].contains("title s3"),
        "row zero carries the bar:\n{}",
        frame.join("\n")
    );
}

// Verifies: gh #209 - scrollback mode pins the bar below the footer
// (dock rows never scroll) and a switch appends the divider.
#[test]
fn scrollback_bar_pins_below_the_footer() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let mut chat = Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()));
    chat.screen_mode = false;
    chat.handle_key("\x14");
    chat.handle_key("\x1b1");
    let text = strip(&chat.render(100)).join("\n");
    assert!(
        text.contains("── Switched to Session: title s1 (s1) ──"),
        "the divider appends:\n{text}"
    );
    // The bar is the dock's last row: footer above it, nothing after.
    let rows = strip(&chat.render(100));
    let bar_at = rows.iter().rposition(|row| row.contains("title s3"));
    let foot_at = rows.iter().rposition(|row| row.contains("p/m"));
    assert!(
        bar_at.is_some() && foot_at.is_some() && bar_at.unwrap() > foot_at.unwrap(),
        "bar below the footer"
    );
}

// Verifies: gh #209 - cycling wraps around the tab strip.
#[test]
fn cycle_wraps_around_the_strip() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let mut chat = Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()));
    chat.handle_key("\x14");
    assert_eq!(chat.active_tab, 1);
    chat.cycle_tab(1);
    assert_eq!(chat.active_tab, 0, "wraps forward");
    chat.cycle_tab(-1);
    assert_eq!(chat.active_tab, 1, "wraps back");
}

// Verifies: gh #209 - the fullscreen hit map: row 0 hits tabs,
// row 1 never does, far-right hits nothing.
#[test]
fn tab_hit_maps_row_zero_only() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    use lca_tui::widgets::tabs::TabHit;
    let mut chat = Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()));
    chat.screen_mode = true;
    chat.handle_key("\x14");
    assert_eq!(chat.tab_hit(0, 0, 80), Some(TabHit::Tab(0)));
    assert_eq!(chat.tab_hit(0, 1, 80), None);
    assert_eq!(chat.tab_hit(200, 0, 80), None);
}

// =============================================================================
// gh #462: a background turn keeps running across a tab switch.
//
// The refusal these rows replace is sanctioned to change by this cycle's brief
// (`tab_moves_refuse_mid_turn` is named in the report). The design: the turn's
// session binding is pinned when the turn starts, the stream keeps landing in
// the tab it started on, and tab moves are therefore safe.
// =============================================================================

/// A chat whose host reports the session the turn is running in, the way the
/// real host does (gh #462): `current_session_id` answers with the tab's own
/// session, and the two tabs replay distinct records so a mixed-up stream shows
/// up immediately.
fn two_tab_chat() -> Chat {
    use lca_tui::engine::keybindings::KeybindingsManager;
    Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()))
}

// Verifies: gh #462 (the defect's row) - selecting another tab mid-turn does
// not refuse and does not cancel. The old code said "a turn is running; finish
// or cancel it before switching tabs".
#[test]
fn selecting_another_tab_does_not_refuse_or_cancel_mid_turn() {
    let mut chat = two_tab_chat();
    chat.handle_key("\x14"); // two tabs, active = 1
    chat.select_tab(0);
    chat.begin_turn(lca_protocol::steer_queue());
    assert!(chat.turn_running);
    chat.select_tab(1);
    assert_eq!(chat.active_tab, 1, "the switch happened");
    assert!(chat.turn_running, "and the turn kept running");
    chat.select_tab(0);
    assert_eq!(chat.active_tab, 0, "and it switches back");
    assert!(chat.turn_running, "still running");
    assert!(
        !chat
            .last_notice()
            .as_deref()
            .is_some_and(|notice| notice.contains("before switching tabs")),
        "no refusal: {:?}",
        chat.last_notice()
    );
}

// Verifies: gh #462 (the corruption row) - a background turn's events land in
// the transcript of the tab the turn started on, even while another tab is
// visible. Switching back shows the complete content.
#[test]
fn a_background_turns_events_land_in_its_own_transcript() {
    let mut chat = two_tab_chat();
    chat.handle_key("\x14"); // two tabs
    chat.select_tab(0);
    // Start a turn on tab 0.
    chat.begin_turn(lca_protocol::steer_queue());
    let owning = chat.active_tab;
    assert_eq!(owning, 0);
    chat.on_turn_event(lca_protocol::TurnEvent::TextDelta("tab one says".into()));
    // Switch to tab 1 while the turn runs.
    chat.select_tab(1);
    assert_eq!(chat.active_tab, 1);
    assert!(chat.turn_running, "still running");
    // The stream keeps arriving while tab 1 is visible.
    chat.on_turn_event(lca_protocol::TurnEvent::TextDelta(" this too".into()));
    chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
        status: lca_protocol::TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
    });
    // Tab 1's visible transcript must not carry tab 0's stream.
    let visible = strip(&chat.render(100)).join("\n");
    assert!(
        !visible.contains("tab one says"),
        "the background stream must not leak into the visible tab:\n{visible}"
    );
    // Switch back: the whole answer is there.
    chat.select_tab(0);
    let back = strip(&chat.render(100)).join("\n");
    assert!(
        back.contains("tab one says this too"),
        "the background turn's content is complete on return:\n{back}"
    );
}

// Verifies: gh #462 - the tab item reports a running state for a background
// turn (the pulsing indicator #209 promised), and an unread marker once it
// settles while unfocused.
#[test]
fn the_tab_item_shows_running_then_unread() {
    let mut chat = two_tab_chat();
    chat.handle_key("\x14"); // tab 1
    chat.select_tab(0);
    chat.begin_turn(lca_protocol::steer_queue());
    // Move away: tab 0 is now the background turn.
    chat.select_tab(1);
    let widget = chat.tabs_widget();
    assert!(
        widget.items[0].is_generating,
        "the background tab reports generating"
    );
    assert!(!widget.items[1].is_generating, "the visible tab does not");
    let bar = widget.render(100).join("\n");
    assert!(bar.contains('◌'), "the generating marker paints:\n{bar}");
    // Settle it while unfocused.
    chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
        status: lca_protocol::TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
    });
    let widget = chat.tabs_widget();
    assert!(
        !widget.items[0].is_generating,
        "settled: no longer generating"
    );
    assert!(
        widget.items[0].badge.is_some(),
        "an unread marker until focused"
    );
    // Focusing clears it.
    chat.select_tab(0);
    let widget = chat.tabs_widget();
    assert!(widget.items[0].badge.is_none(), "focus clears the marker");
}

// Verifies: gh #462 - cancellation stays obvious: Ctrl+C in the visible tab
// must not silently cancel an invisible turn. The visible tab has no turn of
// its own, so the key does nothing (it does not reach the background turn).
#[test]
fn cancelling_the_visible_tab_does_not_cancel_a_background_turn() {
    use crate::state::Action;
    let mut chat = two_tab_chat();
    chat.handle_key("\x14");
    chat.select_tab(0);
    chat.begin_turn(lca_protocol::steer_queue());
    chat.select_tab(1); // now the turn is invisible
    let action = chat.handle_key("\x03"); // Ctrl+C
    assert_ne!(
        action,
        Action::CancelTurn,
        "a visible tab with no turn of its own must not cancel an invisible one"
    );
    assert!(chat.turn_running, "the background turn survives");
    // But on the tab that owns a turn, Ctrl+C still cancels.
    chat.select_tab(0);
    let action = chat.handle_key("\x03");
    assert_eq!(
        action,
        Action::CancelTurn,
        "the owning tab cancels its own turn"
    );
}

// Verifies: gh #462 - closing a background tab is safe; closing the ACTIVE tab
// mid-turn still refuses, because that is where the user's own stream is
// landing and removing its row would hide work in progress. An honest narrow
// refusal, as the brief allows.
#[test]
fn closing_the_active_tab_mid_turn_still_refuses() {
    let mut chat = two_tab_chat();
    chat.handle_key("\x14"); // two tabs, active = 1
    chat.begin_turn(lca_protocol::steer_queue());
    chat.handle_key("\x17"); // Ctrl+W on the active tab, empty composer
    assert_eq!(chat.tabs.len(), 2, "the active tab was not closed");
    assert!(
        chat.last_notice()
            .as_deref()
            .is_some_and(|notice| notice.contains("still running")),
        "it names the reason: {:?}",
        chat.last_notice()
    );
}

// Verifies: gh #462 - an idle background tab closes freely while another tab
// is mid-turn: nothing it owns is in flight, so nothing is lost.
#[test]
fn an_idle_background_tab_closes_mid_turn() {
    let mut chat = two_tab_chat();
    chat.handle_key("\x14"); // two tabs, active = 1
    chat.begin_turn(lca_protocol::steer_queue()); // tab 1 owns the turn
    // Tab 0 is the idle background tab; close it via the widget's close cell.
    chat.tab_gesture(lca_tui::widgets::tabs::TabHit::Close(0));
    assert_eq!(chat.tabs.len(), 1, "the idle background tab closed");
    assert!(chat.turn_running, "the turn on the surviving tab runs on");
    assert_eq!(chat.active_tab, 0, "the surviving tab is now index 0");
    assert_eq!(
        chat.turn_tab,
        Some(0),
        "the turn's owner index moved with the row it was on"
    );
}

// Verifies: gh #462 - a background tab whose OWN turn is running refuses to
// close: its stream is landing in a transcript that would be dropped, so the
// work would be invisible. Same destructive question as the active tab.
#[test]
fn a_running_background_tab_refuses_to_close() {
    let mut chat = two_tab_chat();
    chat.handle_key("\x14"); // two tabs, active = 1
    chat.select_tab(0);
    chat.begin_turn(lca_protocol::steer_queue()); // tab 0 owns the turn
    chat.select_tab(1); // tab 0 runs in the background
    chat.tab_gesture(lca_tui::widgets::tabs::TabHit::Close(0));
    assert_eq!(chat.tabs.len(), 2, "the running tab stayed");
    assert!(
        chat.toasts
            .iter()
            .any(|toast| toast.text.contains("still running")),
        "and it says why: {:?}",
        chat.toasts
    );
}

// Verifies: gh #462 - a background turn's cost lands on its own session, not
// on the visible tab's footer. The session total is per session; folding a
// turn tab 0 ran into tab 1's totals would report one conversation's spend
// under another's name.
#[test]
fn a_background_turns_cost_stays_off_the_visible_tabs_footer() {
    let mut chat = two_tab_chat();
    chat.handle_key("\x14"); // two tabs
    chat.select_tab(0);
    chat.begin_turn(lca_protocol::steer_queue());
    let spend = lca_protocol::Usage {
        output: 40,
        cost: 0.5,
        ..Default::default()
    };
    chat.on_turn_event(lca_protocol::TurnEvent::Usage(spend.clone()));
    // Settle the turn while tab 0 is the owner, the way `reap` does.
    let owner = chat.turn_tab.unwrap_or(chat.active_tab);
    chat.with_tab_state(owner, |chat| chat.fold_turn_usage(&spend));
    chat.settle_turn_tab();
    // Tab 0 now shows its own spend.
    assert!(
        (chat.usage.cost - 0.5).abs() < 1e-12,
        "tab 0 owns the spend: {}",
        chat.usage.cost
    );
    // Tab 1's footer is untouched.
    chat.select_tab(1);
    assert_eq!(
        chat.usage.cost, 0.0,
        "a background turn's 0.50 must not appear on tab 1's footer"
    );
    // And tab 0 still has it after the round trip.
    chat.select_tab(0);
    assert!(
        (chat.usage.cost - 0.5).abs() < 1e-12,
        "the spend survived the switch: {}",
        chat.usage.cost
    );
}

// Verifies: gh #462 - a mid-turn tab switch must not move the host's session
// binding under the running turn. The turn worker reads the host's
// "current session" cell on its own thread, lazily; `switch_session` rewrites
// that same cell. So a switch in the window between `begin_turn` and the
// worker's first write would append THIS turn's records to the OTHER session's
// log - silent cross-session corruption, worse than a corrupted display.
//
// The fix is deferral: a switch mid-turn swaps the visible state (each tab
// already carries its own transcript) but does not call the host until the
// turn settles.
#[test]
fn a_mid_turn_switch_does_not_move_the_hosts_session_binding() {
    use std::sync::Mutex;
    let switched: std::sync::Arc<Mutex<Vec<String>>> = Default::default();
    let seen = switched.clone();
    let minted = std::sync::Arc::new(Mutex::new(3u32));
    let next = minted.clone();
    let mut opts = tabbed_options();
    opts.hooks.switch_session = Some(std::sync::Arc::new(move |id: &str| {
        seen.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(id.to_string());
        Some(Vec::new())
    }));
    opts.hooks.new_session = Some(std::sync::Arc::new(move || {
        let mut n = next.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let id = format!("s{}", *n);
        *n += 1;
        Some(id)
    }));
    let mut chat = Chat::new(
        opts,
        Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    );
    // Two tabs while idle (this switch legitimately reaches the host).
    chat.handle_key("\x14");
    chat.select_tab(0);
    // Snapshot once the reader is settled on tab 0 and the turn is starting.
    let before = switched
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .len();
    chat.begin_turn(lca_protocol::steer_queue());
    // Now switch while the turn runs.
    chat.select_tab(1);
    let during = switched
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .len();
    assert_eq!(
        during,
        before,
        "the host's session binding must not move under a running turn: {:?}",
        switched.lock().unwrap_or_else(|p| p.into_inner())
    );
    // But the display did switch.
    assert_eq!(chat.active_tab, 1);
    // And it lands once the turn settles - in `reap`'s order: settle the tab,
    // clear the running flag, then hand the host the deferred switch.
    chat.settle_turn_tab();
    chat.turn_running = false;
    chat.apply_deferred_host_switch();
    let after = switched
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .len();
    assert_eq!(after, before + 1, "the deferred switch lands at settle");
}

// Verifies: gh #462 - opening a tab mid-turn is refused, because minting a
// session necessarily rewrites the host's binding (there is no way to create
// one without making it current). A narrow, honest refusal rather than a
// corrupted log.
#[test]
fn opening_a_tab_mid_turn_is_refused() {
    let mut chat = two_tab_chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.handle_key("\x14");
    assert_eq!(chat.tabs.len(), 1, "no tab opened past the seed");
    assert!(
        chat.last_notice()
            .as_deref()
            .is_some_and(|notice| notice.contains("switch tabs") || notice.contains("opening")),
        "it says why: {:?}",
        chat.last_notice()
    );
}
