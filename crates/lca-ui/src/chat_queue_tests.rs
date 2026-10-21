//! Queue delivery modes (gh #76): follow-up flush granularity, plus the
//! gh #401 queue-UX gaps (dock labels and the dequeue hint, draft-merge
//! on restore, and message queueing through a compaction).
//! Uses the `chat_tests` fixtures like the other queue journeys.

use super::chat as new_chat;
use super::chat_with_commands;
use super::strip;
use crate::chat::Chat;
use crate::state::Action;
use std::sync::Arc;

// Verifies: gh #76 - `one-at-a-time` follow-ups pop one per turn end
// (today's behavior, pinned); `all` flushes every pending follow-up
// as one joined submission, in order.
#[test]
fn follow_up_modes_flush_one_or_all() {
    let mut one = new_chat();
    one.follow_up_mode = lca_protocol::QueueMode::OneAtATime;
    one.queue_submit("first".into(), lca_protocol::SubmitMode::FollowUp);
    one.queue_submit("second".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(one.take_next_pending().as_deref(), Some("first"));
    assert_eq!(one.take_next_pending().as_deref(), Some("second"));

    let mut all = new_chat();
    all.follow_up_mode = lca_protocol::QueueMode::All;
    all.queue_submit("first".into(), lca_protocol::SubmitMode::FollowUp);
    all.queue_submit("second".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(all.take_next_pending().as_deref(), Some("first\n\nsecond"));
    assert!(all.take_next_pending().is_none());
}

// Verifies: gh #254 - the transcript's user order is the fork's index
// space (one entry per turn record), so an `all`-mode flush pushes
// ONE joined user entry, not one per drained message. Two entries
// for one record forked index 1 into "no user message at index".
#[test]
fn all_mode_flush_pushes_one_user_entry() {
    let mut all = new_chat();
    all.follow_up_mode = lca_protocol::QueueMode::All;
    all.queue_submit("first".into(), lca_protocol::SubmitMode::FollowUp);
    all.queue_submit("second".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(all.take_next_pending().as_deref(), Some("first\n\nsecond"));
    let users: Vec<_> = all
        .transcript
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            crate::transcript::Entry::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(users, vec!["first\n\nsecond".to_string()]);
}

// ---- gh #401: the queue's three UX gaps -------------------------------

/// The pending dock's rendered frame, footer included.
fn frame(chat: &Chat) -> String {
    strip(&chat.render(100)).join("\n")
}

// Verifies: gh #401 (pi's `updatePendingMessagesDisplay`) - a queued
// steer reads `Steering:` and a queued follow-up reads `Follow-up:`,
// pi's own labels, and the dequeue hint sits under them naming the LIVE
// binding for `app.message.dequeue` rather than a literal.
#[test]
fn the_pending_dock_labels_modes_and_hints_the_dequeue_key() {
    let mut chat = new_chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("check text.rs".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_submit("then nextest".into(), lca_protocol::SubmitMode::FollowUp);
    let text = frame(&chat);
    assert!(
        text.contains("Steering: check text.rs"),
        "a steer reads as pi's `Steering:`:\n{text}"
    );
    assert!(
        text.contains("Follow-up: then nextest"),
        "a follow-up reads as pi's `Follow-up:`:\n{text}"
    );
    let key = chat.keybindings.primary_key("app.message.dequeue");
    assert!(!key.is_empty(), "the dequeue binding resolves");
    assert!(
        text.contains(&format!("↳ {key} to edit all queued messages")),
        "the hint names the live key `{key}`:\n{text}"
    );
}

// Verifies: gh #401 - the hint is a queue affordance, so an empty queue
// paints none of it (an idle frame stays quiet).
#[test]
fn an_empty_queue_paints_no_dequeue_hint() {
    let chat = new_chat();
    let text = frame(&chat);
    assert!(
        !text.contains("to edit all queued messages"),
        "no hint without a queue:\n{text}"
    );
}

// Verifies: gh #401 item 1 (pi's `restoreQueuedMessagesToEditor`) - the
// restore joins the queue ahead of the live draft, `\n\n`-separated, so
// aborting a runaway turn keeps both and nothing is auto-sent. The
// queue is older than the draft, which is the merge rule.
#[test]
fn a_restore_merges_the_queue_ahead_of_the_live_draft() {
    let mut chat = new_chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("queued one".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_submit("queued two".into(), lca_protocol::SubmitMode::FollowUp);
    // The user kept typing while the turn ran.
    chat.editor.set_text("half-typed draft");
    let restored = chat.restore_pending();
    assert_eq!(restored, 2, "both queued messages came back");
    assert_eq!(
        chat.editor.text(),
        "queued one\n\nqueued two\n\nhalf-typed draft",
        "queue first, draft last"
    );
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("Restored 2 queued messages to editor")
    );
}

// Verifies: gh #401 item 1 - a lone queued message uses pi's singular
// copy, and an empty restore says nothing (a no-op never announces).
#[test]
fn the_restore_notice_counts_and_never_speaks_on_an_empty_queue() {
    let mut chat = new_chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("only one".into(), lca_protocol::SubmitMode::Steer);
    assert_eq!(chat.restore_pending(), 1);
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("Restored 1 queued message to editor")
    );

    let mut empty = new_chat();
    assert_eq!(empty.restore_pending(), 0);
    assert_eq!(empty.last_notice(), None, "nothing was restored to say");
}

// Verifies: gh #401 item 1 - a draft that is only whitespace is not
// text: the merge keeps the queue alone rather than leaving a blank
// paragraph between them.
#[test]
fn a_blank_draft_leaves_the_restored_queue_alone() {
    let mut chat = new_chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("queued".into(), lca_protocol::SubmitMode::Steer);
    chat.editor.set_text("   \n\t ");
    chat.restore_pending();
    assert_eq!(chat.editor.text(), "queued");
}

// Verifies: gh #401 item 1 - the restored text lands with the caret at
// the end (pi parks it there), so the next keystroke refines the last
// message instead of editing the middle of the queue.
#[test]
fn a_restore_parks_the_caret_at_the_end() {
    let mut chat = new_chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("first".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_submit("second".into(), lca_protocol::SubmitMode::FollowUp);
    chat.restore_pending();
    chat.editor.insert_str("-tail");
    assert_eq!(chat.editor.text(), "first\n\nsecond-tail");
}

// Verifies: gh #401 item 1 (pi's `clearQueue`) - the restore also empties
// the running turn's boundary queue, or a restored message would ALSO be
// injected into the turn it was just pulled out of.
#[test]
fn a_restore_empties_the_running_turns_boundary_queue() {
    let mut chat = new_chat();
    let steer = lca_protocol::steer_queue();
    chat.begin_turn(steer.clone());
    chat.queue_submit("steer me".into(), lca_protocol::SubmitMode::Steer);
    assert_eq!(steer.lock().unwrap().len(), 1);
    chat.restore_pending();
    assert_eq!(
        steer.lock().unwrap().len(),
        0,
        "the boundary queue is empty after the restore"
    );
    assert_eq!(chat.editor.text(), "steer me");
}

// Verifies: gh #401 item 2 (pi's `queueCompactionMessage`) - a prompt
// typed while `/compact` runs queues instead of being refused, the dock
// shows it, and the notice says when it will run. The editor stays live
// for the whole compaction.
#[test]
fn typing_during_compaction_queues_for_after_it() {
    let mut chat = new_chat();
    chat.compacting = true;
    for c in "ask after".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert_eq!(
        chat.compaction_queued.len(),
        1,
        "the message parked instead of being refused"
    );
    assert_eq!(chat.compaction_queued[0].text, "ask after");
    assert_eq!(
        chat.compaction_queued[0].mode,
        lca_protocol::SubmitMode::Steer,
        "Enter during compaction parks a steer"
    );
    assert!(chat.pending.is_empty(), "not in the turn-end queue");
    assert_eq!(
        chat.take_submitted(),
        None,
        "no turn starts under compaction"
    );
    let text = frame(&chat);
    assert!(
        text.contains("Steering: ask after"),
        "the dock lists it:\n{text}"
    );
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("Queued message for after compaction")
    );
}

// Verifies: gh #401 item 2 - `Alt+Enter` during compaction parks a
// follow-up, so the two speeds survive the compaction case too.
#[test]
fn a_follow_up_during_compaction_keeps_its_mode() {
    let mut chat = new_chat();
    chat.compacting = true;
    chat.editor.set_text("later please");
    assert_eq!(chat.handle_key("\x1b\r"), Action::Continue); // Alt+Enter
    assert_eq!(chat.compaction_queued.len(), 1);
    assert_eq!(
        chat.compaction_queued[0].mode,
        lca_protocol::SubmitMode::FollowUp
    );
    let text = frame(&chat);
    assert!(text.contains("Follow-up: later please"), "{text}");
}

// Verifies: gh #401 item 2 (pi's `flushCompactionQueue`) - when
// compaction ends the queue flushes in order: the first message starts
// the turn, the rest wait behind it, and every mode is preserved.
#[test]
fn the_compaction_queue_flushes_in_order_when_compaction_ends() {
    let mut chat = new_chat();
    chat.compacting = true;
    chat.queue_for_compaction("first".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_for_compaction("second".into(), lca_protocol::SubmitMode::FollowUp);
    chat.queue_for_compaction("third".into(), lca_protocol::SubmitMode::Steer);
    assert!(chat.flush_compaction_queue(), "the flush moved the queue");
    assert!(chat.compaction_queued.is_empty());
    assert_eq!(
        chat.take_submitted().as_deref(),
        Some("first"),
        "the first message starts the turn"
    );
    assert_eq!(
        chat.take_submitted_queue(),
        Some(lca_protocol::SubmitMode::Steer)
    );
    assert_eq!(
        chat.take_next_pending().as_deref(),
        Some("second"),
        "the rest wait behind it, in order"
    );
    assert_eq!(chat.take_next_pending().as_deref(), Some("third"));
}

// Verifies: gh #401 item 2 (pi's `flushCompactionQueue`) - a queued
// extension command runs FIRST, so a turn started by the first prompt
// cannot swallow it.
#[test]
fn a_queued_command_in_the_compaction_queue_runs_before_the_turn() {
    let called = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut chat = chat_with_commands(called.clone());
    chat.compacting = true;
    chat.queue_command_for_compaction("/inspect".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_for_compaction("then work".into(), lca_protocol::SubmitMode::Steer);
    assert!(chat.flush_compaction_queue());
    let seen = called.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "the command dispatched: {seen:?}");
    assert_eq!(seen[0].0, "inspect");
    assert_eq!(chat.take_submitted().as_deref(), Some("then work"));
}

// Verifies: gh #401 item 2 (pi's `restoreQueue`) - a flush that cannot
// start a turn (no model) puts the whole queue back, so a compaction
// ending during a broken login never loses what the user typed.
#[test]
fn a_flush_without_a_model_rolls_the_queue_back() {
    let mut chat = new_chat();
    chat.world.options.model_label = Arc::new(std::sync::Mutex::new(String::new()));
    chat.compacting = true;
    chat.queue_for_compaction("keep me".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_for_compaction("and me".into(), lca_protocol::SubmitMode::FollowUp);
    assert!(chat.flush_compaction_queue(), "the flush ran and failed");
    assert_eq!(
        chat.compaction_queued.len(),
        2,
        "both messages are back in the queue"
    );
    assert_eq!(chat.compaction_queued[0].text, "keep me");
    assert_eq!(chat.compaction_queued[1].text, "and me");
    assert_eq!(chat.take_submitted(), None, "no turn started");
    assert!(
        chat.pending.is_empty(),
        "nothing leaked to the turn-end queue"
    );
}

// Verifies: gh #401 item 2 - a flush while a turn still runs routes into
// that turn (pi's `willRetry` branch): a steer joins the boundary, a
// follow-up waits, and neither starts a second turn.
#[test]
fn a_flush_mid_turn_joins_the_running_turn() {
    let mut chat = new_chat();
    let steer = lca_protocol::steer_queue();
    chat.begin_turn(steer.clone());
    chat.compacting = true;
    chat.queue_for_compaction("steer in".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_for_compaction("follow in".into(), lca_protocol::SubmitMode::FollowUp);
    assert!(chat.flush_compaction_queue());
    assert_eq!(
        steer.lock().unwrap().len(),
        1,
        "the steer reached the boundary queue"
    );
    assert_eq!(chat.take_submitted(), None, "no second turn");
    assert_eq!(chat.pending.len(), 2, "both are visible in the dock");
}

// Verifies: gh #401 (pi's `clearAllQueues`) - the dequeue/abort rescue
// takes the compaction queue too, so one key returns everything.
#[test]
fn a_restore_takes_the_compaction_queue_as_well() {
    let mut chat = new_chat();
    chat.compacting = true;
    chat.queue_for_compaction("parked".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_submit("queued".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(chat.restore_pending(), 2);
    assert!(chat.compaction_queued.is_empty());
    assert!(chat.pending.is_empty());
    assert_eq!(chat.editor.text(), "queued\n\nparked");
}

// Verifies: gh #401 item 3 - `Alt+Up` dequeues while a queue exists and
// still jumps between prompts when it does not (the binding is shared,
// so the empty case keeps its old meaning).
#[test]
fn alt_up_dequeues_when_a_queue_exists_and_jumps_when_it_does_not() {
    let mut chat = new_chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("take me back".into(), lca_protocol::SubmitMode::Steer);
    assert_eq!(chat.handle_key("\x1b[1;3A"), Action::Continue); // Alt+Up
    assert!(chat.pending.is_empty(), "the queue came back to the editor");
    assert_eq!(chat.editor.text(), "take me back");

    // With no queue, Alt+Up is not a dequeue; the jump keys (Shift/Ctrl+Up)
    // still hop, which is the pair the row above pins.
    let mut empty = new_chat();
    empty.world.resize(80, 24);
    empty.transcript.push_user("first question");
    empty.transcript.append_text("an answer");
    empty.transcript.finish_assistant();
    empty.transcript.push_user("second question");
    assert_eq!(empty.handle_key("\x1b[1;3A"), Action::Continue); // Alt+Up
    assert!(
        empty.jump_target.is_none(),
        "with no queue Alt+Up dequeues nothing and jumps nothing"
    );
    assert_eq!(empty.handle_key("\x1b[1;2A"), Action::Continue); // Shift+Up
    assert!(empty.jump_target.is_some(), "the jump key still hops");
}
