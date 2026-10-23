//! Rows for the collapsed compaction card (gh #393).
//!
//! Each row is a promise from the brief's test policy. The "wall is measurably
//! gone" row counts rows rather than characters, so it cannot pass because a
//! summary happened to be short.

use crate::theme::Theme;
use crate::transcript::compaction_card::Card;
use crate::transcript::{Entry, Transcript};

/// A transcript with one replayed compaction carrying a long summary.
fn with_compaction(summary: &str) -> Transcript {
    let mut transcript = Transcript::default();
    transcript.replay_records(
        &[lca_protocol::Record::Compaction {
            v: 1,
            ts: 0,
            id: "c1".to_string(),
            parent: None,
            replaced_from: "u1".to_string(),
            replaced_to: "u2".to_string(),
            first_kept_id: String::new(),
            summary: summary.to_string(),
            strategy: "llm".to_string(),
            usage: None,
            read_files: Vec::new(),
            modified_files: Vec::new(),
            system_prompt: None,
        }],
        None,
    );
    transcript
}

fn frame(transcript: &Transcript) -> String {
    transcript.render(100, &Theme::default()).join("\n")
}

// Verifies: gh #393's acceptance - **a compaction card is collapsed by default
// and names itself.** The reader must be able to tell that their context was
// summarised, by which strategy, and what range - without reading the summary.
#[test]
fn a_compaction_card_renders_collapsed_naming_strategy_and_range() {
    let transcript = with_compaction("THE-WHOLE-SUMMARY");
    let text = frame(&transcript);
    assert!(
        text.contains("[compaction]"),
        "the card names itself:\n{text}"
    );
    assert!(
        text.contains("summarised via llm"),
        "and names the strategy:\n{text}"
    );
    assert!(
        text.contains("u1..u2"),
        "and the record range it replaced:\n{text}"
    );
    assert!(
        !text.contains("THE-WHOLE-SUMMARY"),
        "and does NOT show the summary until asked:\n{text}"
    );
    assert!(
        text.contains("expand"),
        "and offers the key that reveals it:\n{text}"
    );
}

// Verifies: gh #393 design point 4 - **collapsed is not gone.** A reader who
// never expands still learns the counts, so "collapsed" cannot become "silently
// discarded". The range is the fallback when a count is not recorded.
#[test]
fn a_card_with_a_count_names_the_entries_it_replaced() {
    let mut transcript = Transcript::default();
    transcript.push_compaction_card(Card::replay("llm", "S", "a", "b", Some(12)));
    let text = frame(&transcript);
    assert!(text.contains("12 entries replaced"), "{text}");
    // Singular is not "1 entries".
    let mut one = Transcript::default();
    one.push_compaction_card(Card::replay("llm", "S", "a", "b", Some(1)));
    assert!(frame(&one).contains("1 entry replaced"), "{}", frame(&one));
}

// Verifies: gh #393 design point 1 - **expanding shows the full summary, and
// collapsing hides it again.** The toggle is the per-entry override, so both
// directions come from the same state.
#[test]
fn expanding_shows_the_summary_and_collapsing_hides_it_again() {
    let mut transcript = with_compaction("THE-WHOLE-SUMMARY");
    assert!(
        !frame(&transcript).contains("THE-WHOLE-SUMMARY"),
        "starts collapsed"
    );

    assert!(transcript.toggle_entry_compaction(0), "the card toggles");
    assert!(
        frame(&transcript).contains("THE-WHOLE-SUMMARY"),
        "expanding reveals the summary:\n{}",
        frame(&transcript)
    );

    assert!(transcript.toggle_entry_compaction(0), "and toggles back");
    assert!(
        !frame(&transcript).contains("THE-WHOLE-SUMMARY"),
        "collapsing hides it again:\n{}",
        frame(&transcript)
    );
}

// Verifies: gh #393 design point 3 - **an expanded card survives a streaming
// delta.** The reader expanded *this* summary; the assistant writing more text
// must not fold it back up under them. A redraw is not a reset.
#[test]
fn an_expanded_card_stays_expanded_across_a_streaming_delta() {
    let mut transcript = with_compaction("THE-WHOLE-SUMMARY");
    transcript.toggle_entry_compaction(0);
    assert!(frame(&transcript).contains("THE-WHOLE-SUMMARY"));

    // A streaming delta lands after the card, as the next turn's answer would.
    transcript.append_text("more answer text");
    assert!(
        frame(&transcript).contains("THE-WHOLE-SUMMARY"),
        "the delta did not collapse the reader's card:\n{}",
        frame(&transcript)
    );
}

// Verifies: gh #393 design point 3, the other half - **a NEW compaction starts
// collapsed** even while an older one is expanded. Expansion belongs to the card
// the reader opened; a later summary is a different thing they have not read.
#[test]
fn a_new_compaction_arrives_collapsed_even_if_an_older_one_is_expanded() {
    let mut transcript = with_compaction("FIRST-SUMMARY");
    transcript.toggle_entry_compaction(0);
    assert!(frame(&transcript).contains("FIRST-SUMMARY"));

    transcript.push_compaction_card(Card::replay("llm", "SECOND-SUMMARY", "u3", "u4", None));
    let text = frame(&transcript);
    assert!(
        text.contains("FIRST-SUMMARY"),
        "the older card is still open"
    );
    assert!(
        !text.contains("SECOND-SUMMARY"),
        "the new card starts collapsed:\n{text}"
    );
}

// Verifies: gh #393 design point 5 - **replay and live agree on shape.** #496
// split them on *content* (the live event carries no summary); this change makes
// them agree on *form*: one band card, one label, one headline, collapsed.
//
// The one remaining difference is deliberate and asserted rather than glossed:
// a live card has no summary, so it offers **no expand key**. Advertising a key
// that reveals nothing would be the trap the failed-card row also guards.
#[test]
fn the_live_card_and_the_replayed_card_share_their_collapsed_shape() {
    let replay_frame = frame(&with_compaction("S"));
    let mut live = Transcript::default();
    live.push_compaction_card(Card::live("threshold", true));
    let live_frame = frame(&live);

    for (name, text) in [("replay", &replay_frame), ("live", &live_frame)] {
        assert!(
            text.contains("[compaction]"),
            "{name} names itself:\n{text}"
        );
        // The shape: the card is one band, and it is short.
        let rows = text.lines().count();
        assert!(
            rows <= 6,
            "{name} is a collapsed card, got {rows} rows:\n{text}"
        );
    }
    // Same label, same headline verb ("summarised"), so a reader recognises the
    // same kind of row in both views.
    assert!(replay_frame.contains("summarised"), "{replay_frame}");
    assert!(live_frame.contains("summarised"), "{live_frame}");

    // The replayed card has a body, so it offers the key and starts collapsed.
    let replay = with_compaction("S");
    assert_eq!(replay.compaction_card_expanded(0), Some(false));
    assert!(replay_frame.contains("expand"), "{replay_frame}");
    // The live card has none, so it offers nothing it cannot honour.
    assert_eq!(live.compaction_card_expanded(0), None);
    assert!(!live_frame.contains("expand"), "{live_frame}");
}

// Verifies: gh #393 + #496 - **a failed compaction is not rendered as a success,
// and has nothing to expand.** `success: false` means no record was written and
// the context is exactly as it was, so the card says so and offers no key it
// cannot honour.
#[test]
fn a_failed_live_compaction_says_the_context_is_unchanged_and_offers_no_key() {
    let mut transcript = Transcript::default();
    transcript.push_compaction_card(Card::live("manual", false));
    let text = frame(&transcript);
    assert!(text.contains("compaction failed"), "{text}");
    assert!(
        text.contains("the context is unchanged"),
        "the failure states the consequence:\n{text}"
    );
    assert!(
        !text.contains("expand"),
        "there is no summary to reveal, so no key is advertised:\n{text}"
    );
    assert_eq!(
        transcript.compaction_card_expanded(0),
        None,
        "and the card reports no expandable state"
    );
    assert!(
        !transcript.toggle_entry_compaction(0),
        "so a click on it does nothing"
    );
}

// Verifies: gh #393 gap 2 - **a branch summary gets the same treatment.**
//
// Before this change `Record::BranchSummary` rendered *nowhere*: it fell into
// `replay_records`' `_ => {}` arm, so a resumed fork silently lost the text that
// explained the branch. Collapsing it is strictly better than dropping it.
#[test]
fn a_branch_summary_renders_as_a_collapsed_card() {
    let mut transcript = Transcript::default();
    transcript.replay_records(
        &[lca_protocol::Record::BranchSummary {
            v: 1,
            ts: 0,
            id: "b1".to_string(),
            parent: None,
            from_id: None,
            summary: "WHAT-THE-BRANCH-LEARNED".to_string(),
        }],
        None,
    );
    let text = frame(&transcript);
    assert!(
        text.contains("branch summary"),
        "the card says what it is:\n{text}"
    );
    assert!(
        !text.contains("WHAT-THE-BRANCH-LEARNED"),
        "and starts collapsed:\n{text}"
    );
    assert!(
        transcript.toggle_entry_compaction(0),
        "an expanded branch summary is reachable"
    );
    assert!(
        frame(&transcript).contains("WHAT-THE-BRANCH-LEARNED"),
        "and shows the text once opened:\n{}",
        frame(&transcript)
    );
}

// Verifies: gh #393 - **the wall is measurably gone.** One compaction adds a
// fixed number of rows however long its summary is, so ten compactions cost ten
// cards rather than ten walls.
//
// Measured against a pre-fix baseline computed in the row itself: the baseline is
// what the old `Entry::Raw` shape would have rendered for the same summary, so the
// comparison cannot drift from the fix.
#[test]
fn several_compactions_stay_readable_because_the_summary_no_longer_expands() {
    let summary: String = (1..=40)
        .map(|n| format!("line {n} of the summary\n"))
        .collect();
    let mut transcript = Transcript::default();
    for _ in 0..10 {
        transcript.push_compaction_card(Card::replay("llm", &summary, "a", "b", None));
    }
    let after = transcript.render(100, &Theme::default()).len();

    // The pre-fix shape: ten Raw entries carrying the whole summary each.
    let mut baseline = Transcript::default();
    for _ in 0..10 {
        baseline.push_raw(format!("[compaction] {summary} (via llm)"));
    }
    let before = baseline.render(100, &Theme::default()).len();

    assert!(
        after * 4 < before,
        "ten collapsed cards ({after} rows) are far fewer than ten expanded \
         summaries ({before} rows)"
    );
    // And the per-card cost is bounded, not merely smaller: a card is a handful of
    // rows regardless of the summary.
    let one = {
        let mut t = Transcript::default();
        t.push_compaction_card(Card::replay("llm", &summary, "a", "b", None));
        t.render(100, &Theme::default()).len()
    };
    assert!(one <= 8, "a collapsed card is a handful of rows, got {one}");
}

// Verifies: the entry is a typed card, not a pre-rendered string (gh #393's
// shape rule, and the reason the reader can expand it at all).
#[test]
fn a_compaction_is_a_typed_card_not_a_raw_string() {
    let transcript = with_compaction("S");
    let entries = transcript.entries();
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, Entry::Compaction { .. })),
        "the record becomes a typed entry"
    );
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, Entry::Raw(text) if text.contains("[compaction]"))),
        "not a pre-rendered raw line"
    );
}

// Verifies: a live-found defect - **the range must distinguish the records it
// names.** Record ids are `<13-digit ms>-<counter>-<entropy>`, so every id in a
// session shares its first thirteen characters. Truncating from the head printed
// `17927858..17927858` for a range spanning four records: the card looked like it
// reported a range and reported nothing.
#[test]
fn the_record_range_distinguishes_ids_that_share_a_timestamp_prefix() {
    let from = "1792785803400-00000002-430c";
    let to = "1792785803400-00000009-430c";
    let mut transcript = Transcript::default();
    transcript.push_compaction_card(Card::replay("llm", "S", from, to, None));
    let text = frame(&transcript);
    let range = text
        .split(" · ")
        .find(|part| part.contains(".."))
        .unwrap_or_else(|| panic!("the card reports a range:\n{text}"));
    let (left, right) = range.trim().split_once("..").expect("a range");
    assert_ne!(
        left, right,
        "the two ends must differ, or the range says nothing:\n{text}"
    );
    assert!(
        text.contains("00000002") && text.contains("00000009"),
        "the distinguishing counter is what survives:\n{text}"
    );
    // And an id with no groups at all is returned whole rather than panicking.
    assert!(
        Card::replay("llm", "S", "plain", "plain", None)
            .headline()
            .contains("plain"),
        "a single-group id survives whole"
    );
}
