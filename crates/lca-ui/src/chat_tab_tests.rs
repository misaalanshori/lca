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
        chat.world.notice.as_deref(),
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
        chat.world.notice.as_deref(),
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

// Verifies: gh #209 - a mid-turn switch, open, or close refuses with
// the tab named (background turns are the deferred follow-up).
#[test]
fn tab_moves_refuse_mid_turn() {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let mut chat = Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()));
    chat.turn_running = true;
    chat.handle_key("\x14");
    assert_eq!(chat.tabs.len(), 1, "no tab opened past the seed");
    assert!(
        chat.world
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("before opening a tab")),
        "it names the turn: {:?}",
        chat.world.notice
    );
    chat.turn_running = false;
    chat.handle_key("\x14"); // tab two first, then the turn starts
    assert_eq!(chat.tabs.len(), 2);
    chat.turn_running = true;
    chat.handle_key("\x1b1");
    assert!(
        chat.world
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("before switching tabs")),
        "it names the turn: {:?}",
        chat.world.notice
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
