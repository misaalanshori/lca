//! Steering and follow-up queue rows (FR-CORE-11/12, ADR-0038), split
//! from `chat_tests.rs` under the workspace's 1,200-line ceiling (gate
//! 11). Shares that module's fixtures.

use super::chat;
use super::strip;
use crate::state::Action;

// Verifies: FR-CORE-11 - a message submitted while a turn runs queues
// (steer) instead of starting a second turn, and shows in the band.
#[test]
fn steering_queues_while_a_turn_runs() {
    let mut chat = chat();
    let steer = lca_protocol::steer_queue();
    chat.begin_turn(steer.clone());
    for c in "mid-turn note".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert_eq!(chat.pending.len(), 1);
    assert_eq!(chat.pending[0].mode, lca_protocol::SubmitMode::Steer);
    assert_eq!(
        steer.lock().unwrap().len(),
        1,
        "the steer reached the running turn's boundary queue"
    );
    let text = strip(&chat.render(80)).join("\n");
    assert!(text.contains("mid-turn note"), "the pending band shows it");
}

// Verifies: FR-CORE-12 - an aborted turn returns its queue to the editor.
#[test]
fn abort_restores_pending_to_the_editor() {
    let mut chat = chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("one".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_submit("two".into(), lca_protocol::SubmitMode::FollowUp);
    chat.restore_pending();
    assert!(chat.pending.is_empty());
    // SANCTIONED CHANGE (gh #401): the restore now separates messages the
    // way pi's `restoreQueuedMessagesToEditor` does, `\n\n`, so two queued
    // prompts read as two messages rather than one two-line message. The
    // row's intent (both come back, in order) is unchanged.
    assert_eq!(chat.editor.text(), "one\n\ntwo");
}

// Verifies: FR-CORE-11 - follow-ups auto-run in order at turn end.
#[test]
fn follow_ups_run_in_order() {
    let mut chat = chat();
    chat.queue_submit("first".into(), lca_protocol::SubmitMode::FollowUp);
    chat.queue_submit("second".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(chat.take_next_pending().as_deref(), Some("first"));
    assert_eq!(chat.take_next_pending().as_deref(), Some("second"));
    assert!(chat.take_next_pending().is_none());
}
