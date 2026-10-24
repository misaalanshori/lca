//! NFR-24 guard for gh #462: a tab switch must not cancel the turn, and a
//! background turn's stream must not corrupt the visible tab.
//!
//! The defect: #209 specified that switching tabs while an agent generates in
//! Tab 1 does not cancel Tab 1's turn - it keeps streaming in the background,
//! its tab item pulses, and an unread badge appears when it settles. The
//! shipped implementation (`222a8312`) refused each of `open_tab`,
//! `select_tab`, and `close_tab` mid-turn instead, and
//! `chat_tab_tests::tab_moves_refuse_mid_turn` pinned that refusal as expected
//! behaviour. The refusal defeats the primary reason tabs exist: a 30-120 s
//! `cargo test` blocked the whole interface.
//!
//! Three properties make the safe slice work, and this file guards the two a
//! single crate cannot see from the code alone:
//!
//! 1. **The turn belongs to the tab**, not the interface. `Chat::begin_turn`
//!    pins `turn_tab`, and every later event is routed by it.
//! 2. **A background turn's state is that tab's**, so its deltas land in its
//!    own transcript and its spend lands on its own session total. This is the
//!    corruption row: leaking either into the visible tab is exactly the
//!    "corrupted transcript a week later" the brief warns about.
//! 3. **Cancellation is scoped to what is visible.** `Ctrl+C` in a tab that
//!    owns no turn must not kill an invisible one.
//!
//! The behaviour rows live beside the code (`chat::tests::tab_tests`), because
//! `UiOptions` has no `Default` - rebuilding that forty-field fixture here
//! would be a second fixture that drifts. What this file adds is the
//! *wiring*: that the three facts above are actually consulted at the seams a
//! unit row in one module can miss (the key handler, the tab bar, and the
//! per-session accounting).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::{CommandEffect, QueueMode, TurnEvent, TurnStatus, Usage};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_ui::state::UiOptions;
use lca_ui::{Action, Chat};

/// Two sessions: `switch` replays per-session records, `new_session` mints
/// fresh ids, and the session list names both - the minimum a tab needs.
fn tabbed_options() -> UiOptions {
    let minted = Arc::new(Mutex::new(3u32));
    let next = minted.clone();
    let mut opts = UiOptions {
        prompt_slot: Default::default(),
        dialog_slot: Default::default(),
        pending_models: None,
        model_label: Arc::new(Mutex::new("openai-compatible/m".to_string())),
        context_window: Arc::new(Mutex::new(128_000)),
        thinking: Arc::new(Mutex::new(None)),
        theme: "auto".to_string(),
        theme_extra_dirs: Vec::new(),
        themes: lca_ui::theme::THEMES
            .iter()
            .map(|s| s.to_string())
            .collect(),
        initial_lines: Vec::new(),
        initial_header: (Vec::new(), Vec::new()),
        initial_records: Vec::new(),
        initial_tail_lines: Vec::new(),
        initial_messages: Vec::new(),
        open_resume_picker: false,
        yolo: false,
        early_input: None,
        thinking_visibility: Default::default(),
        codeblock_border: Default::default(),
        plain: true,
        invoke_command: Arc::new(|_, _| CommandEffect::None),
        slash_commands: Vec::new(),
        models: Vec::new(),
        workspace: PathBuf::from("."),
        keybinding_overrides: Default::default(),
        keybinding_error: None,
        render_regions: None,
        ui_events: None,
        update_notice: None,
        login: None,
        complete_login: None,
        pick_login: None,
        confirm_login_grant: None,
        confirm_switch: None,
        hooks: lca_ui::UiHooks::default(),
        fullscreen: false,
        follow_up_mode: QueueMode::OneAtATime,
    };
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
            .map(|id| lca_ui::resume::SessionEntry {
                id: id.to_string(),
                title: format!("title {id}"),
                messages: 1,
                age: "now".to_string(),
                labels: Vec::new(),
                modified_ms: 0,
                parent: None,
            })
            .collect()
    }));
    opts.hooks.current_session_id = Some(Arc::new(|| "s1".to_string()));
    opts
}

fn chat() -> Chat {
    Chat::new(tabbed_options(), Arc::new(KeybindingsManager::new()))
}

/// Two tabs, sitting on tab 1 (the second), with tab 0 reachable by `Alt+1`.
fn two_tabs() -> Chat {
    let mut chat = chat();
    chat.handle_key("\x14"); // Ctrl+T
    assert_eq!(chat.tabs_len(), 2);
    assert_eq!(chat.active_tab_index(), 1);
    chat
}

// ---------------------------------------------------------------------------
// 1. The turn belongs to the tab.
// ---------------------------------------------------------------------------

// Verifies: gh #462 - the owning tab is recorded at turn start and is what the
// interface consults. This is the single fact the whole slice rests on: with
// `turn_tab` unset (the pre-fix state) there is no way to route an event, and
// the only safe answer is the refusal that shipped.
#[test]
fn beginning_a_turn_pins_the_tab_that_owns_it() {
    let mut chat = two_tabs();
    chat.handle_key("\x1b1"); // tab 0
    assert!(!chat.turn_is_owned_by_visible_tab_hint());
    chat.begin_turn_pub(lca_protocol::steer_queue());
    assert_eq!(chat.turn_tab_index(), Some(0), "tab 0 owns the turn");
    assert!(chat.turn_is_owned_by_visible_tab_hint());
    // Switching away leaves the ownership alone.
    chat.handle_key("\x1b2");
    assert_eq!(chat.turn_tab_index(), Some(0), "still tab 0's turn");
    assert!(
        !chat.turn_is_owned_by_visible_tab_hint(),
        "and the visible tab no longer owns it"
    );
}

// ---------------------------------------------------------------------------
// 2. The corruption row: a background turn's state is that tab's.
// ---------------------------------------------------------------------------

// Verifies: gh #462 - a background turn's deltas land in its own transcript and
// never in the visible tab's. Switch away, stream, switch back: the content is
// complete on return and absent from the tab that was showing.
#[test]
fn a_background_stream_never_reaches_the_visible_transcript() {
    let mut chat = two_tabs();
    chat.handle_key("\x1b1"); // tab 0 owns the turn
    chat.begin_turn_pub(lca_protocol::steer_queue());
    chat.event(TurnEvent::TextDelta("ANSWER-ON-TAB-0".into()));
    chat.handle_key("\x1b2"); // tab 0 is backgrounded
    chat.event(TurnEvent::TextDelta(" MORE-ON-TAB-0".into()));
    // The visible tab (1) replays only its own records.
    let visible = chat.rendered_frame();
    assert!(
        !visible.contains("ANSWER-ON-TAB-0") && !visible.contains("MORE-ON-TAB-0"),
        "a background stream must not leak into the visible tab:\n{visible}"
    );
    assert!(
        visible.contains("hello from s3"),
        "the visible tab shows its own session:\n{visible}"
    );
    // Back on tab 0 the whole answer is there, in order.
    chat.handle_key("\x1b1");
    let back = chat.rendered_frame();
    assert!(
        back.contains("ANSWER-ON-TAB-0 MORE-ON-TAB-0"),
        "the background turn's content is complete on return:\n{back}"
    );
}

// Verifies: gh #462 - a background turn's cost lands on its own session total,
// not on the visible tab's footer. The session total is per session; folding
// tab 0's spend into tab 1's totals would report one conversation's money under
// another's name, which is a quieter corruption than a mixed transcript.
#[test]
fn a_background_turns_spend_lands_on_its_own_session() {
    let mut chat = two_tabs();
    chat.handle_key("\x1b1");
    chat.begin_turn_pub(lca_protocol::steer_queue());
    let spend = Usage {
        output: 40,
        cost: 0.5,
        ..Default::default()
    };
    chat.event(TurnEvent::Usage(spend.clone()));
    // Settle it the way `TurnState::reap` does, while tab 1 is visible.
    chat.handle_key("\x1b2");
    chat.fold_owned_turn_for_test(&spend);
    chat.settle_owned_turn_for_test();
    assert!(
        (chat.session_cost_for_visible_tab() - 0.0).abs() < 1e-12,
        "tab 1's footer must not carry tab 0's spend: {}",
        chat.session_cost_for_visible_tab()
    );
    chat.handle_key("\x1b1");
    assert!(
        (chat.session_cost_for_visible_tab() - 0.5).abs() < 1e-12,
        "tab 0's own footer carries it: {}",
        chat.session_cost_for_visible_tab()
    );
}

// ---------------------------------------------------------------------------
// 3. Cancellation is scoped to what the reader can see.
// ---------------------------------------------------------------------------

// Verifies: gh #462 - `Ctrl+C` on a tab that owns no turn does not cancel the
// invisible one. This is the safety question the shipped refusal was
// protecting, and getting it wrong loses work the user cannot see.
#[test]
fn ctrl_c_on_a_foreign_tab_does_not_cancel_a_background_turn() {
    let mut chat = two_tabs();
    chat.handle_key("\x1b1");
    chat.begin_turn_pub(lca_protocol::steer_queue());
    chat.handle_key("\x1b2"); // the turn is now invisible
    let action = chat.press("\x03");
    assert_ne!(
        action,
        Action::CancelTurn,
        "a tab with no turn of its own must not cancel an invisible one"
    );
    assert!(chat.turn_is_running_hint(), "the background turn survives");
    // On the owning tab it still cancels.
    chat.handle_key("\x1b1");
    assert_eq!(
        chat.press("\x03"),
        Action::CancelTurn,
        "the owning tab cancels its own turn"
    );
}

// ---------------------------------------------------------------------------
// 4. The tab bar reports the state #209 promised.
// ---------------------------------------------------------------------------

// Verifies: gh #462 - the tab item pulses `◌` while its turn runs and shows an
// unread badge `•` once it settles unfocused, then clears on focus. Both
// markers are text, so they survive a colorless terminal (NFR-28).
#[test]
fn the_tab_bar_marks_a_background_turn_then_its_unread_settle() {
    let mut chat = two_tabs();
    chat.handle_key("\x1b1");
    chat.begin_turn_pub(lca_protocol::steer_queue());
    chat.handle_key("\x1b2"); // tab 0 runs in the background
    let bar = chat.tab_bar_text();
    assert!(
        bar.contains('◌'),
        "the background tab paints the generating marker:\n{bar}"
    );
    chat.event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
    });
    let bar = chat.tab_bar_text();
    assert!(
        !bar.contains('◌'),
        "settled: the generating marker is gone:\n{bar}"
    );
    assert!(
        bar.contains('•'),
        "and an unread marker appears until focused:\n{bar}"
    );
    chat.handle_key("\x1b1"); // focus it
    let bar = chat.tab_bar_text();
    assert!(!bar.contains('•'), "focus clears the unread marker:\n{bar}");
}

// ---------------------------------------------------------------------------
// 5. The tab moves themselves, pinned.
// ---------------------------------------------------------------------------

// Verifies: gh #462 - the three tab moves during a turn: **select** is allowed
// (its host effect is deferred to settle), while **open** and **close** refuse
// for the tab that owns the running turn. Select is the user-visible win the
// brief asked for; open refuses because minting a session rewrites the host's
// current-session cell, which the running worker reads; close refuses because
// the stream would land in a removed row. This is the contract
// `tab_moves_refuse_mid_turn` used to pin the opposite of, and it is named as
// a sanctioned change in this cycle's report.
#[test]
fn the_three_tab_moves_during_a_turn() {
    let mut chat = chat(); // one tab
    // A second tab first, so the switch half has somewhere to go.
    chat.handle_key("\x14");
    assert_eq!(chat.tabs_len(), 2);
    chat.handle_key("\x1b1");
    chat.begin_turn_pub(lca_protocol::steer_queue()); // tab 0 owns it
    // Select: allowed, and cancels nothing.
    chat.handle_key("\x1b2");
    assert_eq!(chat.active_tab_index(), 1, "switched mid-turn");
    assert!(chat.turn_is_running_hint(), "and still running");
    chat.handle_key("\x1b1");
    assert_eq!(chat.active_tab_index(), 0, "switched back mid-turn");
    assert!(chat.turn_is_running_hint(), "still running");
    // Open: REFUSED. Minting a session makes it the host's current one, and
    // that cell is what the running worker reads; a switch can be deferred to
    // settle, a creation cannot.
    chat.handle_key("\x14");
    assert_eq!(chat.tabs_len(), 2, "no tab opened mid-turn");
    assert!(
        chat.notice_log()
            .iter()
            .any(|line| line.contains("switch tabs")),
        "the refusal names the alternative: {:?}",
        chat.notice_log()
    );
    // Close the owning tab (0, where `begin_turn` ran): refused, with a reason.
    chat.handle_key("\x17");
    assert_eq!(chat.tabs_len(), 2, "the owning tab was not closed");
    assert!(
        chat.notice_log()
            .iter()
            .any(|line| line.contains("still running")),
        "the refusal names the reason: {:?}",
        chat.notice_log()
    );
}
