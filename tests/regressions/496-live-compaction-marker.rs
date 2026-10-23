//! NFR-24 guard for gh #496: a compaction that happens **while the user is in
//! the session** leaves a mark in the transcript, and the replay path is not
//! traded away to get it.
//!
//! The defect: `apply_turn_event` mapped `CompactionStarted | CompactionEnded` to
//! `{}` - explicitly a no-op, on the reasoning that they are "reconstruction
//! events for headless/rpc; rendered elsewhere". But the live path *is* where the
//! user is when a threshold or overflow compaction fires: the transcript they are
//! reading has just been summarized underneath them, and nothing said so. They
//! learned it on the next `/resume`, if ever.
//!
//! # Split of coverage, on purpose
//!
//! The behaviour rows (marker appears, reason named, failure distinct,
//! chronological position, one marker per compaction) live in
//! `lca-ui`'s `chat_compaction_tests.rs`, beside the `Chat` fixture they need -
//! `UiOptions` is a forty-field struct with no `Default`, so a second copy in
//! another crate would drift. This file guards what is observable from outside
//! that crate:
//!
//! 1. **The replay path still works, and is pinned** - the issue says it is
//!    already correct, and a fix for the live half must not cost it. It had no
//!    test of its own; the issue's "verified with a 4-record fixture" was a manual
//!    check.
//! 2. **The wiring is the one that ships** - the event arm routes to the marker
//!    rather than being a no-op again.
//!
//! Verifies: gh #496, FR-UI-20's live/replay consistency, and NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::Record;

/// A `Compaction` record with everything a replay needs.
fn compaction_record(id: &str, summary: &str, strategy: &str) -> Record {
    Record::Compaction {
        v: 1,
        ts: 0,
        id: id.to_string(),
        parent: None,
        replaced_from: "u1".to_string(),
        replaced_to: "u2".to_string(),
        first_kept_id: String::new(),
        summary: summary.to_string(),
        strategy: strategy.to_string(),
        usage: None,
        read_files: Vec::new(),
        modified_files: Vec::new(),
        system_prompt: None,
    }
}

/// A user-message record, so the fixture has entries on both sides of the cut.
fn user_record(id: &str, text: &str) -> Record {
    Record::User {
        v: 1,
        ts: 0,
        id: id.to_string(),
        parent: None,
        content: text.to_string(),
        attachments: Vec::new(),
        queue: None,
    }
}

// Verifies: gh #496 - **the replay path is untouched**. `replay_records` places a
// summary at the compaction record's own position, naming the strategy. This is
// the behaviour the issue says already works, and the reason option A was enough:
// the user who leaves and comes back gets the *content*, so the live marker only
// has to say the *fact*.
//
// The 4-record fixture is the one the issue cites (`u1, u2, Compaction{u1..u2},
// u3`) - it existed only as a manual check until this row.
#[test]
fn replaying_a_session_still_puts_the_summary_in_record_order() {
    let records = vec![
        user_record("u1", "U:first"),
        user_record("u2", "U:second"),
        compaction_record("c1", "SUMMARY", "llm"),
        user_record("u3", "U:third"),
    ];
    let mut transcript = lca_ui::Transcript::default();
    transcript.replay_records(&records, None);
    let text = transcript.render(100, &lca_ui::Theme::default()).join("\n");

    let first = text.find("U:first").expect("the first user record");
    let summary = text.find("[compaction]").expect("the compaction record");
    let third = text.find("U:third").expect("the record after the cut");
    assert!(
        first < summary && summary < third,
        "the summary sits in record order:\n{text}"
    );
    assert!(
        text.contains("SUMMARY"),
        "and carries the summary text, unlike the live marker which has none:\n{text}"
    );
    assert!(
        text.contains("(via llm)"),
        "and names the strategy that produced it:\n{text}"
    );
}

// Verifies: gh #496 - **the live marker names the event, the replay names the
// content, and they are allowed to differ.** This is the acceptance the issue
// asks for: "the two views are on the record as different-but-consistent". A
// regression that made the live path render the summary too would need the store
// seam option B was rejected for, so this row also documents the boundary.
#[test]
fn the_live_marker_and_the_replayed_summary_are_different_on_purpose() {
    let mut transcript = lca_ui::Transcript::default();
    transcript.replay_records(&[compaction_record("c1", "SUMMARY", "llm")], None);
    let replayed = transcript.render(100, &lca_ui::Theme::default()).join("\n");
    assert!(
        replayed.contains("(via llm)"),
        "the replay knows the strategy:\n{replayed}"
    );

    // The live marker has no strategy and no summary - it is built from the
    // `TurnEvent`, which carries neither. Asserted structurally, because the
    // live path needs the `Chat` fixture that lives inside `lca-ui`.
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/chat_turn_events.rs"
    ))
    .expect("the event applier is readable");
    let marker = source
        .find("fn note_live_compaction")
        .expect("the live marker exists");
    let body = &source[marker..];
    let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];
    assert!(
        body.contains("[compaction]") && body.contains("summarised"),
        "the live marker names the event:\n{body}"
    );
    assert!(
        !body.contains("summary"),
        "and does not pretend to carry the summary - it cannot, and option B's \
         store seam was rejected:\n{body}"
    );
}

// Verifies: gh #496's actual defect - `CompactionEnded` is **not** a no-op again.
// The issue was one line: both compaction events mapped to `{}`. A future
// refactor that restores that arm passes every behaviour row in `lca-ui` only if
// it also deletes them, but it could just as easily route to a method that
// silently returns; this asserts the arm exists and reaches the marker.
#[test]
fn the_compaction_end_event_routes_to_the_marker() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/chat_turn_events.rs"
    ))
    .expect("the event applier is readable");
    let arm = source
        .find("TurnEvent::CompactionEnded { reason, success }")
        .expect("`CompactionEnded` has its own arm, not the no-op group");
    let tail = &source[arm..];
    let end = tail.find("TurnEvent::TextDelta").unwrap_or(tail.len());
    assert!(
        tail[..end].contains("note_live_compaction"),
        "the end event calls the marker: {}",
        &tail[..end.min(400)]
    );
    // The no-op group it used to share must no longer name either compaction
    // event, or the fix would be shadowed by an earlier matching arm.
    for noop in source.lines().filter(|l| l.contains("=> {}")) {
        assert!(
            !noop.contains("Compaction"),
            "a compaction event is still folded into a no-op arm: {noop}"
        );
    }
}

// Verifies: gh #496 design point 2 - **the transcript gets no transient state**.
// `CompactionStarted` must add nothing: the separator already shows `Compacting`
// (#388 gap 2), and a "compacting…" row in the scrollback duplicates it. Pinned
// on the arm as well as in the behaviour row, because the behaviour row can only
// see a frame, not the intent.
#[test]
fn the_compaction_start_event_adds_no_transcript_row() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/chat_turn_events.rs"
    ))
    .expect("the event applier is readable");
    let arm = source
        .find("TurnEvent::CompactionStarted")
        .expect("`CompactionStarted` has its own arm");
    let tail = &source[arm..];
    let end = tail
        .find("TurnEvent::CompactionEnded")
        .expect("the next arm follows");
    let body = &tail[..end];
    assert!(
        !body.contains("push_raw") && !body.contains("note_live_compaction"),
        "starting a compaction writes nothing to the transcript:\n{body}"
    );
}
