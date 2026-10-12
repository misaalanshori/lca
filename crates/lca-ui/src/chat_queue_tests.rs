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
