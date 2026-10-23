//! The live compaction marker (gh #496): a compaction that happens while the
//! user is in the session leaves a row in the transcript.
//!
//! The defect was that `apply_turn_event` mapped
//! `CompactionStarted | CompactionEnded` to `{}` - explicitly a no-op, on the
//! reasoning that they are "reconstruction events for headless/rpc; rendered
//! elsewhere". But the live path *is* where the user is when a threshold or
//! overflow compaction fires: the transcript they are reading has just been
//! summarized underneath them, and nothing said so. They learned it on the next
//! `/resume`, if ever.
//!
//! # Why a bare marker (option A) and not the other two
//!
//! The issue listed three options and the brief picked A. The rejections are
//! worth recording here so a future reader does not re-litigate them:
//!
//! - **Threading the store into `Chat`** would let the live render print the
//!   summary and match replay byte for byte. But `lca-ui` deliberately has no
//!   `lca-session` dependency, so that seam would be bought to fetch text the
//!   user sees anyway on replay.
//! - **Rendering on `CompactionStarted`** would put a transient "compacting…"
//!   row in the transcript, duplicating the `Compacting` state the separator
//!   already shows (#388 gap 2). Two live indicators of one fact is the
//!   duplication this repo keeps consolidating away.
//!
//! So the user learns *that* it happened, in chronological position, with the
//! **reason** - the one piece of information the event carries and the
//! transcript can add without the record.

use super::*;

/// The marker the transcript uses, in one place so the rows and the renderer
/// cannot drift.
const MARKER: &str = "[compaction]";

// Verifies: gh #496's acceptance - **a live compaction shows something in the
// transcript at the moment it happens**. This is the row the issue describes as
// missing: before the fix the transcript was byte-identical with and without a
// compaction, which is exactly what a user saw.
#[test]
fn a_live_compaction_leaves_a_marker_in_the_transcript() {
    let mut chat = chat();
    chat.event(lca_protocol::TurnEvent::TextDelta("before".to_string()));
    let before = chat.rendered_frame();

    chat.event(lca_protocol::TurnEvent::CompactionStarted {
        reason: "threshold".to_string(),
    });
    chat.event(lca_protocol::TurnEvent::CompactionEnded {
        reason: "threshold".to_string(),
        success: true,
    });

    let after = chat.rendered_frame();
    assert_ne!(
        before, after,
        "the live transcript changed when the compaction finished; before gh #496 \
         it rendered nothing at all"
    );
    assert!(
        after.contains(MARKER),
        "and the change is a named marker:\n{after}"
    );
}

// Verifies: gh #496 design point 1 - **the reason is shown**. `threshold` (the
// context crossed a limit the user did not set) and `overflow` (a provider
// reported one, and the range was cut to recover) mean very different things to
// someone deciding whether to trust the summary; a bare marker would collapse
// them into one.
#[test]
fn the_marker_names_why_the_compaction_fired() {
    for reason in ["threshold", "overflow", "manual"] {
        let mut chat = chat();
        chat.event(lca_protocol::TurnEvent::CompactionEnded {
            reason: reason.to_string(),
            success: true,
        });
        let frame = chat.rendered_frame();
        assert!(
            frame.contains(reason),
            "the marker names `{reason}`:\n{frame}"
        );
    }
}

// Verifies: gh #496 design point 1's second half - **a failed compaction must
// not look like a successful one**. `success: false` means no summary record was
// written: the context is unchanged, and a user who sees a neutral marker has
// been told the opposite of the truth about the state of their session.
#[test]
fn a_failed_compaction_renders_distinctly_from_a_successful_one() {
    let mut ok = chat();
    ok.event(lca_protocol::TurnEvent::CompactionEnded {
        reason: "threshold".to_string(),
        success: true,
    });

    let mut failed = chat();
    failed.event(lca_protocol::TurnEvent::CompactionEnded {
        reason: "threshold".to_string(),
        success: false,
    });

    assert_ne!(
        ok.rendered_frame(),
        failed.rendered_frame(),
        "a failure and a success do not render identically:\nok:\n{}\nfailed:\n{}",
        ok.rendered_frame(),
        failed.rendered_frame()
    );
    let frame = failed.rendered_frame();
    assert!(
        frame.contains("failed"),
        "and the failure says so in words:\n{frame}"
    );
}

// Verifies: gh #496 design point 2 - **no transient row in the transcript**. The
// separator's `Compacting` is the live state and the transcript row is the record
// of the event; starting must add nothing, because a "compacting…" line in the
// scrollback is a spinner in the wrong place and duplicates the separator.
#[test]
fn starting_a_compaction_adds_no_transcript_row() {
    let mut chat = chat();
    chat.event(lca_protocol::TurnEvent::TextDelta("before".to_string()));
    let before = chat.rendered_frame();
    chat.event(lca_protocol::TurnEvent::CompactionStarted {
        reason: "threshold".to_string(),
    });
    assert_eq!(
        before,
        chat.rendered_frame(),
        "`CompactionStarted` alone changes nothing in the transcript; the \
         separator owns the live state"
    );
}

// Verifies: gh #496 design point 1 - **chronological position**. The marker sits
// where the compaction happened among the turn's entries, not appended at the top
// or hoisted to the end. A user reading back needs to know which messages the
// summary covered.
#[test]
fn the_marker_lands_in_chronological_position() {
    let mut chat = chat();
    chat.event(lca_protocol::TurnEvent::TextDelta("first".to_string()));
    chat.event(lca_protocol::TurnEvent::CompactionEnded {
        reason: "threshold".to_string(),
        success: true,
    });
    chat.event(lca_protocol::TurnEvent::TextDelta("second".to_string()));

    let text = chat.rendered_frame();
    let first = text.find("first").expect("the first delta is rendered");
    let marker = text.find(MARKER).expect("the marker is rendered");
    let second = text.find("second").expect("the second delta is rendered");
    assert!(
        first < marker && marker < second,
        "the marker sits between the entries it separates:\n{text}"
    );
}

// Verifies: gh #496 design point 5 - **idempotence**. `compact_candidate` emits
// `CompactionStarted`/`CompactionEnded` once per attempt, and a turn that
// overflows can attempt a recovery; a retry must not collect two markers for one
// compaction, because the transcript is the record and a duplicate reads as two
// summaries.
#[test]
fn a_repeated_end_event_does_not_twice_mark_one_compaction() {
    let mut chat = chat();
    chat.event(lca_protocol::TurnEvent::CompactionStarted {
        reason: "threshold".to_string(),
    });
    chat.event(lca_protocol::TurnEvent::CompactionEnded {
        reason: "threshold".to_string(),
        success: true,
    });
    chat.event(lca_protocol::TurnEvent::CompactionEnded {
        reason: "threshold".to_string(),
        success: true,
    });
    let frame = chat.rendered_frame();
    assert_eq!(
        frame.matches(MARKER).count(),
        1,
        "one compaction is one marker, however many end events arrive:\n{frame}"
    );
}

// Verifies: gh #496 - **a second compaction is a second marker**. The idempotence
// rule above must key on the compaction, not on "a marker already exists": a
// session that compacts twice has two events and needs two rows, or the second
// summary has no chronological slot.
#[test]
fn a_second_compaction_gets_its_own_marker() {
    let mut chat = chat();
    chat.event(lca_protocol::TurnEvent::CompactionStarted {
        reason: "threshold".to_string(),
    });
    chat.event(lca_protocol::TurnEvent::CompactionEnded {
        reason: "threshold".to_string(),
        success: true,
    });
    chat.event(lca_protocol::TurnEvent::TextDelta("between".to_string()));
    // A second compaction, shaped the way `compact_candidate` really emits one:
    // Started always precedes its End, which is also what makes "one marker per
    // Started/Ended cycle" the honest dedupe rule.
    chat.event(lca_protocol::TurnEvent::CompactionStarted {
        reason: "overflow".to_string(),
    });
    chat.event(lca_protocol::TurnEvent::CompactionEnded {
        reason: "overflow".to_string(),
        success: true,
    });
    let frame = chat.rendered_frame();
    assert_eq!(
        frame.matches(MARKER).count(),
        2,
        "two compactions, two markers:\n{frame}"
    );
    assert!(
        frame.contains("threshold") && frame.contains("overflow"),
        "each names its own reason:\n{frame}"
    );
}
