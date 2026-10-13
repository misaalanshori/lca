//! Queue delivery modes (gh #76): follow-up flush granularity.
//! Uses the `chat_tests` fixtures like the other queue journeys.

use super::chat;

// Verifies: gh #76 - `one-at-a-time` follow-ups pop one per turn end
// (today's behavior, pinned); `all` flushes every pending follow-up
// as one joined submission, in order.
#[test]
fn follow_up_modes_flush_one_or_all() {
    let mut one = chat();
    one.follow_up_mode = lca_protocol::QueueMode::OneAtATime;
    one.queue_submit("first".into(), lca_protocol::SubmitMode::FollowUp);
    one.queue_submit("second".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(one.take_next_pending().as_deref(), Some("first"));
    assert_eq!(one.take_next_pending().as_deref(), Some("second"));

    let mut all = chat();
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
    let mut all = chat();
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
