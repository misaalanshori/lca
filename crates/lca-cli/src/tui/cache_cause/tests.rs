//! Rows for gh #402's cache-miss attribution.
//!
//! The behaviour under test is the *attribution*, so these build record lists by
//! hand rather than driving a session: the measurement itself is
//! `lca-session`'s and is covered by `docs/testing-plan.md` §9's scenarios. What no
//! row covered until now is "which cause does the notice name".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::{FORMAT_VERSION, Record};

use super::{Cause, attribute, notice_text};

fn miss(record_id: &str, idle_ms: u64, model_changed: bool) -> lca_session::CacheMiss {
    lca_session::CacheMiss {
        record_id: record_id.to_string(),
        missed_tokens: 50_000,
        missed_cost: 0.25,
        idle_ms,
        model_changed,
    }
}

fn user(id: &str, content: &str, ts: u64) -> Record {
    Record::User {
        v: FORMAT_VERSION,
        ts,
        id: id.to_string(),
        parent: None,
        content: content.to_string(),
        attachments: vec![],
        queue: None,
    }
}

fn injected(id: &str, kind: &str, content: &str, ts: u64) -> Record {
    Record::CustomMessage {
        v: FORMAT_VERSION,
        ts,
        id: id.to_string(),
        parent: None,
        custom_type: kind.to_string(),
        content: content.to_string(),
        display: false,
        details: None,
    }
}

fn narrowed(id: &str, ts: u64) -> Record {
    Record::ExtensionEvent {
        v: FORMAT_VERSION,
        ts,
        id: id.to_string(),
        parent: None,
        extension: "host".to_string(),
        event: "cache-boundary-narrowed".to_string(),
        detail: "stable region diverged at message 3 (boundary 5 -> 3)".to_string(),
    }
}

fn assistant(id: &str, ts: u64) -> Record {
    Record::Assistant {
        v: FORMAT_VERSION,
        ts,
        id: id.to_string(),
        parent: None,
        content: vec![],
        reasoning: None,
        reasoning_signature: None,
        provider_thinking_level: None,
        model: Some("m".to_string()),
        provider: Some("p".to_string()),
        usage: None,
    }
}

// Verifies: gh #402 - **a skill match is named as a skill match**, read from the
// `[skill instructions]` marker gh #299 put in the log rather than inferred from
// the token arithmetic.
#[test]
fn a_skill_injection_is_attributed_to_the_skill_match() {
    let records = vec![
        user("u1", "first", 1),
        injected("m1", "skills", "[skill instructions]\ndo the thing\n", 2),
        assistant("a2", 3),
    ];
    assert_eq!(
        attribute(&records, &miss("a2", 0, false), 0),
        Cause::SkillMatched
    );
}

// Verifies: gh #402 - an instruction delta is named as one. gh #217's delta carries
// `[instructions changed]`, and it reaches the prompt by the same route.
#[test]
fn an_instruction_delta_is_attributed_to_the_change() {
    let records = vec![
        user("u1", "first", 1),
        injected(
            "m1",
            "instructions",
            "[instructions changed]\ndiff here\n",
            2,
        ),
        assistant("a2", 3),
    ];
    assert_eq!(
        attribute(&records, &miss("a2", 0, false), 0),
        Cause::InstructionsChanged
    );
}

// Verifies: gh #402 - a `cache-boundary-narrowed` event is attributed to the
// divergence, reading the event the measurement already wrote rather than
// re-deriving that the prefix moved.
#[test]
fn a_boundary_narrowing_is_attributed_to_the_divergence() {
    let records = vec![
        user("u1", "first", 1),
        narrowed("e1", 2),
        assistant("a2", 3),
    ];
    assert_eq!(
        attribute(&records, &miss("a2", 0, false), 0),
        Cause::BoundaryDiverged
    );
    assert!(
        Cause::BoundaryDiverged.is_defect(),
        "a transform eating the cache is worth investigating"
    );
}

// Verifies: gh #402 design point 3 / FR-CACHE-2 - **a model switch is counted, not
// a fault.** The wording row: it must not read as a bug report, because the cost is
// deliberate and crying wolf here would train users past the notice that matters.
#[test]
fn a_model_switch_reads_as_counted_cost_not_a_defect() {
    let records = vec![user("u1", "first", 1), assistant("a2", 3)];
    let cause = attribute(&records, &miss("a2", 0, true), 0);
    assert_eq!(cause, Cause::ModelSwitched);
    assert!(!cause.is_defect(), "a switch is by design (FR-CACHE-2)");
    let text = notice_text(cause, 50_000, 0.25, 1);
    assert!(
        text.contains("counted as real cost"),
        "the wording says counted, not wasted: {text}"
    );
    for forbidden in ["miss:", "bug", "wasted", "leak"] {
        assert!(
            !text.to_lowercase().contains(forbidden),
            "a deliberate switch must not be worded as a defect ({forbidden}): {text}"
        );
    }
}

// Verifies: gh #402 - **an idle expiry is named as an idle expiry**, from
// `CacheMiss::idle_ms` (the measurement's own field, not a second opinion).
#[test]
fn an_idle_gap_past_the_ttl_is_attributed_to_expiry() {
    let records = vec![user("u1", "first", 1), assistant("a2", 3)];
    let cause = attribute(&records, &miss("a2", 3_600_000, false), 300_000);
    assert_eq!(cause, Cause::IdleExpired);
    assert!(!cause.is_defect(), "a TTL expiry is how caching works");
}

// Verifies: gh #402 design point 5 - **an unattributable miss says so.** No
// preceding event, no model switch, no idle gap: guessing would be undebuggable,
// so the notice names the gap instead.
#[test]
fn an_unattributable_miss_is_named_unknown_not_guessed() {
    let records = vec![user("u1", "first", 1), assistant("a2", 3)];
    let cause = attribute(&records, &miss("a2", 0, false), 300_000);
    assert_eq!(cause, Cause::Unknown);
    let text = notice_text(cause, 50_000, 0.0, 1);
    assert!(
        text.contains("cause unknown"),
        "the notice admits the gap rather than inventing a cause: {text}"
    );
    assert!(
        text.contains("cost unknown"),
        "and does not invent a number either: {text}"
    );
}

// Verifies: gh #402 - **the window is the miss's own record.** An event recorded
// *after* the miss cannot have caused it; attribution must not reach forward.
#[test]
fn an_event_after_the_miss_does_not_explain_it() {
    let records = vec![
        user("u1", "first", 1),
        assistant("a2", 3),
        injected("m1", "skills", "[skill instructions]\nlater\n", 4),
    ];
    assert_eq!(
        attribute(&records, &miss("a2", 0, false), 0),
        Cause::Unknown,
        "the skill matched after this miss, so it did not cause it"
    );
}

// Verifies: gh #402 - a prompt change outranks a coincident model switch, because
// it is the more actionable fact and the notice is once-per-cause (so a switch
// masked here would never be reported at all).
#[test]
fn a_prompt_change_outranks_a_coincident_model_switch() {
    let records = vec![
        user("u1", "first", 1),
        narrowed("e1", 2),
        assistant("a2", 3),
    ];
    assert_eq!(
        attribute(&records, &miss("a2", 0, true), 0),
        Cause::BoundaryDiverged,
        "the boundary move is the specific fact; the switch is not lost because the \
         next unattributed switch still reports"
    );
}

// Verifies: gh #402 - **the marker is read where gh #299 put it.** A `User` record
// carrying the wrapper (older sessions) attributes the same way as a
// `CustomMessage`, so the notice does not depend on which record kind a version
// happened to write.
#[test]
fn a_plain_user_record_carrying_the_marker_also_attributes() {
    let records = vec![
        user("u1", "first", 1),
        user("u2", "[skill instructions]\nold shape\n", 2),
        assistant("a3", 3),
    ];
    assert_eq!(
        attribute(&records, &miss("a3", 0, false), 0),
        Cause::SkillMatched
    );
}

// Verifies: gh #402 - the notice carries the numbers the footer shows. Both read
// one `collect_cache_misses`; this pins that the text renders the miss's own
// fields rather than a re-computation.
#[test]
fn the_notice_renders_the_miss_own_numbers() {
    let text = notice_text(Cause::BoundaryDiverged, 12_345, 0.0625, 1);
    assert!(text.contains("12,345") || text.contains("12345"), "{text}");
    assert!(text.contains("$0.0625"), "{text}");
    let no_cost = notice_text(Cause::BoundaryDiverged, 12_345, 0.0, 1);
    assert!(no_cost.contains("cost unknown"), "{no_cost}");
}

// Verifies: gh #402 design point 2 - a repeated cause adds up rather than
// repeating, so the count is what tells a user it keeps happening.
#[test]
fn a_repeated_cause_reports_how_often() {
    let once = notice_text(Cause::BoundaryDiverged, 1_000, 0.01, 1);
    assert!(!once.contains("times"), "{once}");
    let thrice = notice_text(Cause::BoundaryDiverged, 1_000, 0.01, 3);
    assert!(thrice.contains("3 times this session"), "{thrice}");
}

// Verifies: gh #402 - fault-shaped and counted-by-design causes read differently.
// The distinction is the whole point of `is_defect`, so it is asserted as a
// property over every cause rather than one example.
#[test]
fn only_a_divergence_or_an_unknown_reads_as_a_defect() {
    for cause in [
        Cause::SkillMatched,
        Cause::InstructionsChanged,
        Cause::BoundaryDiverged,
        Cause::IdleExpired,
        Cause::ModelSwitched,
        Cause::Unknown,
    ] {
        let text = notice_text(cause, 1_000, 0.01, 1);
        assert!(
            !text.is_empty() && !cause.phrase().is_empty(),
            "{cause:?} has wording"
        );
        if cause.is_defect() {
            assert!(text.contains("prompt cache miss:"), "{cause:?}: {text}");
        } else {
            assert!(
                !text.contains("prompt cache miss:"),
                "{cause:?} must not be worded as a fault: {text}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Once-per-cause and footer agreement, driven through the real `miss_notice_for`
// seam rather than the pure `attribute`, so the *set* behaviour is pinned where a
// caller reaches it.
//
// These live here rather than in `display.rs` because that file sits near the
// 1,200-line ceiling (gate 11); the rows are unchanged either way.
// ---------------------------------------------------------------------------

use super::super::miss_notice_for;
use lca_session::{SessionStore, ViewMode};

fn session_with(tag: &str) -> (std::path::PathBuf, SessionStore, lca_session::Session) {
    let root = lca_testkit::scratch_path(tag);
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    (root, store, session)
}

fn usage_record(id: &str, ts: u64, input: u64, cache_write: u64) -> Record {
    Record::Assistant {
        v: FORMAT_VERSION,
        ts,
        id: id.to_string(),
        parent: None,
        content: vec![],
        reasoning: None,
        reasoning_signature: None,
        provider_thinking_level: None,
        model: Some("m".to_string()),
        provider: Some("test".to_string()),
        usage: Some(lca_protocol::Usage {
            input,
            cache_write,
            ..Default::default()
        }),
    }
}

// Verifies: gh #402 design point 2 - **once per cause.** A second miss from the
// same cause is silent; a miss from a *different* cause re-arms. This is the row
// that distinguishes the cause set from the old `last: usize` counter, which could
// only say "more misses happened".
#[test]
fn a_repeated_cause_is_silent_and_a_new_cause_rearms() {
    let (root, store, session) = session_with("lca-402-once");
    // Turn 1 reports cache activity so later misses are measurable (FR-CACHE-4).
    store
        .append(&session, usage_record("a1", 1, 1_000, 49_000))
        .expect("append");
    let mut seen = std::collections::BTreeSet::new();
    assert!(
        miss_notice_for(&store, &session, 1024, &mut seen, true).is_none(),
        "one turn is no miss"
    );

    // Turn 2 re-bills with no explanatory event: `Unknown`.
    store
        .append(&session, usage_record("a2", 2, 50_000, 0))
        .expect("append");
    let first = miss_notice_for(&store, &session, 1024, &mut seen, true).expect("a notice");
    assert!(first.contains("cause unknown"), "{first}");

    // Turn 3: same shape, same cause -> silent.
    store
        .append(&session, usage_record("a3", 3, 50_000, 0))
        .expect("append");
    assert!(
        miss_notice_for(&store, &session, 1024, &mut seen, true).is_none(),
        "the same cause never repeats: {first}"
    );

    // Turn 4 introduces a different cause: a boundary narrowing.
    store.append(&session, narrowed("e1", 4)).expect("append");
    store
        .append(&session, usage_record("a4", 5, 50_000, 0))
        .expect("append");
    let second = miss_notice_for(&store, &session, 1024, &mut seen, true).expect("re-armed");
    assert!(
        second.contains("rewrote context"),
        "a new cause re-arms the notice: {second}"
    );
    assert_ne!(first, second, "different causes, different notices");
    let _ = std::fs::remove_dir_all(root);
}

// Verifies: gh #402 design point 4 - **the notice and `/session` agree.** Both are
// claims about one number, so neither may hold its own copy: the notice takes
// `collect_cache_misses`'s newest miss and `/session` takes `compute_cache_waste`
// over the same records. This asserts they are the same measurement.
#[test]
fn the_notice_and_the_session_totals_share_one_measurement() {
    let (root, store, session) = session_with("lca-402-agree");
    store
        .append(&session, usage_record("a1", 1, 1_000, 49_000))
        .expect("append");
    store
        .append(&session, usage_record("a2", 2, 50_000, 0))
        .expect("append");
    let read = store.resolved(&session, ViewMode::Display).expect("read");
    let misses = lca_session::collect_cache_misses(&read.records, 1024);
    let totals = lca_session::compute_cache_waste(&read.records, 1024);
    assert_eq!(
        misses.len() as u64,
        totals.miss_count,
        "the notice list and the totals are the same scan"
    );
    assert_eq!(
        misses.iter().map(|m| m.missed_tokens).sum::<u64>(),
        totals.missed_tokens,
        "the per-miss tokens sum to the number /session reports"
    );
    let notice = miss_notice_for(
        &store,
        &session,
        1024,
        &mut std::collections::BTreeSet::new(),
        true,
    )
    .expect("a notice");
    let newest = misses.last().expect("a miss");
    assert!(
        notice.contains(&newest.missed_tokens.to_string()),
        "the notice quotes the same miss /session counts: {notice}"
    );
    let _ = std::fs::remove_dir_all(root);
}
