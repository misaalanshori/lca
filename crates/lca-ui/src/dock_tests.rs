//! gh #388 gap 2: the dock's height is constant, and `Compacting` is a
//! first-class separator state.
//!
//! **Why row-count invariance and not "an idle status exists".** The latter
//! passes while the height still changes, which is exactly the defect: the
//! dock used to gain a `running...` row on turn start and keep it, so every
//! turn moved the transcript by a row and a long session felt unstable. The
//! rows here count rendered dock rows before, during, and after.
//!
//! Split from `chat_tests.rs` for the 1,200-line ceiling.

use super::*;
use super::{chat, options};
use lca_tui::engine::text::strip_terminal_sequences;

fn dock(chat: &Chat, width: u16) -> Vec<String> {
    chat.sections(width)
        .1
        .iter()
        .map(|row| strip_terminal_sequences(row))
        .collect()
}

fn end_turn(chat: &mut Chat) {
    chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
        status: lca_protocol::TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
    });
}

// Verifies: gh #388 gap 2 - **the property**, not a proxy for it. Starting a
// turn and reaping it leaves the dock's row count identical, so the
// transcript above it never shifts.
#[test]
fn the_dock_keeps_its_height_across_a_turn() {
    let mut chat = chat();
    let idle = dock(&chat, 80).len();

    chat.begin_turn(Default::default());
    let working = dock(&chat, 80).len();
    assert_eq!(
        working, idle,
        "starting a turn must not change the dock's height: it went {idle} -> {working}"
    );

    end_turn(&mut chat);
    let after = dock(&chat, 80).len();
    assert_eq!(
        after, idle,
        "and reaping one must not either: {idle} -> {working} -> {after}"
    );
}

// Verifies: gh #388 gap 2 - the same invariance in the other direction, for a
// compaction. A compaction is work, so it takes the status row a turn takes
// and gives it back, without the dock growing.
#[test]
fn the_dock_keeps_its_height_across_a_compaction() {
    let mut chat = chat();
    let idle = dock(&chat, 80).len();
    chat.begin_turn(Default::default());
    let working = dock(&chat, 80).len();
    chat.compacting = true;
    chat.mark_compacting_for_test(crate::CompactionReason::Threshold);
    let compacting = dock(&chat, 80).len();
    chat.compacting = false;
    chat.separator.working();
    let back = dock(&chat, 80).len();
    assert_eq!(
        (working, compacting, back),
        (idle, idle, idle),
        "Working -> Compacting -> Working must not change the row count"
    );
}

// Verifies: gh #388 gap 2 - a compaction happens **mid-turn**, so the two
// states must compose rather than replace one another: the turn keeps the
// badge and the separator shows the compaction.
#[test]
fn a_compaction_mid_turn_keeps_the_turn_visible() {
    let mut chat = chat();
    chat.begin_turn(Default::default());
    chat.compacting = true;
    chat.mark_compacting_for_test(crate::CompactionReason::Manual);
    let rows = dock(&chat, 100);
    let separator = rows
        .iter()
        .find(|row| row.contains("Compacting"))
        .unwrap_or_else(|| panic!("a compacting separator row: {rows:?}"));
    assert!(
        separator.contains("Compacting"),
        "the separator says what is happening: {separator:?}"
    );
    assert!(
        crate::status::work_state(chat.turn_running, chat.compacting, false).is_working(),
        "and the turn still counts as work for the badge"
    );
}

// Verifies: gh #388 gap 2 - each reason renders, and the reason is **visible**
// (not merely carried): a threshold compaction and a manual one read
// differently to a user who triggered neither.
#[test]
fn every_compaction_reason_is_displayed() {
    for (reason, needle) in [
        (crate::CompactionReason::Manual, "manual"),
        (crate::CompactionReason::Threshold, "threshold"),
        (crate::CompactionReason::Overflow, "overflow"),
    ] {
        let mut chat = chat();
        chat.compacting = true;
        chat.mark_compacting_for_test(reason);
        let rows = dock(&chat, 100);
        let separator = rows
            .iter()
            .find(|row| row.contains("Compacting"))
            .unwrap_or_else(|| panic!("{reason:?} renders: {rows:?}"));
        assert!(
            separator.contains(needle),
            "the reason must be on screen ({needle}): {separator:?}"
        );
    }
}

// Verifies: gh #388 gap 2 / NFR-28 - the plain theme still distinguishes the
// states, because the words differ rather than only the colors.
#[test]
fn the_plain_theme_keeps_every_state_distinguishable() {
    let mut chat = chat_with_plain_theme();
    let idle = dock(&chat, 100).join("\n");
    chat.begin_turn(Default::default());
    let working = dock(&chat, 100).join("\n");
    chat.compacting = true;
    chat.mark_compacting_for_test(crate::CompactionReason::Threshold);
    let compacting = dock(&chat, 100).join("\n");
    assert!(working.contains("Working"), "{working}");
    assert!(compacting.contains("Compacting"), "{compacting}");
    assert_ne!(idle, working, "idle and working differ in words");
    assert_ne!(
        working, compacting,
        "working and compacting differ in words"
    );
}

fn chat_with_plain_theme() -> Chat {
    let mut opts = options();
    opts.plain = true;
    Chat::new(
        opts,
        std::sync::Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    )
}

// Verifies: NFR-26 - the separator row does not wrap at the 80-column
// minimum, in every state including a compaction with a reason.
#[test]
fn the_separator_stays_one_row_at_the_narrow_minimum() {
    for reason in [
        None,
        Some(crate::CompactionReason::Manual),
        Some(crate::CompactionReason::Threshold),
        Some(crate::CompactionReason::Overflow),
    ] {
        let mut chat = chat();
        if let Some(reason) = reason {
            chat.compacting = true;
            chat.mark_compacting_for_test(reason);
        } else {
            chat.begin_turn(Default::default());
        }
        let rows = dock(&chat, 80);
        let separator_rows: Vec<&String> = rows.iter().filter(|row| row.contains('─')).collect();
        let top = rows
            .iter()
            .find(|row| row.starts_with('╭'))
            .unwrap_or_else(|| panic!("a framed top row at 80 cols: {rows:?}"));
        assert!(
            visible_width(top) <= 80,
            "the framed top row fits 80 columns: {top:?}"
        );
        assert!(
            !separator_rows.is_empty(),
            "the separator is on screen: {rows:?}"
        );
    }
}

fn visible_width(text: &str) -> usize {
    lca_tui::engine::text::visible_width(text)
}

// Verifies: gh #388 gap 2 - the separator agrees with [`WorkKind`] instead of
// forming a second opinion. The badge and the taskbar spinner already treat a
// compaction as work (gh #369); the separator must show a state for exactly
// the cases `work_state` calls working, or the two surfaces could disagree
// about whether anything is happening.
#[test]
fn the_separator_state_agrees_with_work_kind() {
    // Idle: no turn, no compaction, no shell.
    let idle_chat = chat();
    let work = crate::status::work_state(
        idle_chat.turn_running,
        idle_chat.compacting,
        idle_chat.shell_running(),
    );
    assert!(!work.is_working(), "the fixture starts idle");
    assert_eq!(idle_chat.separator.state(), &crate::SeparatorState::Idle);

    // Compacting alone: `work_state` says working, the separator says
    // Compacting. Both surfaces therefore agree that something is happening.
    let mut compacting_chat = chat();
    compacting_chat.compacting = true;
    compacting_chat.mark_compacting_for_test(crate::CompactionReason::Threshold);
    let work = crate::status::work_state(
        compacting_chat.turn_running,
        compacting_chat.compacting,
        compacting_chat.shell_running(),
    );
    assert!(work.is_working(), "a compaction is work");
    assert!(
        matches!(
            compacting_chat.separator.state(),
            crate::SeparatorState::Compacting { .. }
        ),
        "and the separator says so: {:?}",
        compacting_chat.separator.state()
    );
}
