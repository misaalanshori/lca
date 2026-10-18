//! Session usage accounting (gh #327): the footer/`/session`/`/usage`
//! totals must count each turn exactly once.
//!
//! Split from `chat_tests.rs` for the workspace's 1,200-line ceiling (gate
//! 11). Two accumulation points used to exist for the same money — the
//! stream's `TurnEvent::Usage` handler grew `chat.usage.*` (including
//! `cost`) and `TurnState::reap` added `outcome.usage.cost` again — so
//! every turn landed twice. The split now: stream events drive *display*
//! only (footer prompt size, tok/s meter, a turn-local view), and the
//! session totals fold core's authoritative per-turn `TurnOutcome::usage`
//! once through `Chat::fold_turn_usage`, which the record-replay path
//! shares.
//!
//! The rows live here rather than in `tests/regressions/` because
//! `UiOptions` has no `Default` and `Chat::new` takes it — rebuilding that
//! forty-field struct in another crate to test five numbers would be worse
//! than keeping the guard beside the code it pins.

use super::tests::chat;
use super::*;

//
// Two accumulation points existed for the same money: the stream's
// `TurnEvent::Usage` handler grew `chat.usage.*` (including `cost`), and
// `TurnState::reap` added `outcome.usage.cost` again at completion. Every
// turn therefore landed twice in the footer, `/session` and `/usage`.
//
// The split: stream events drive ONLY live display (the footer's prompt
// size and the tok/s meter) plus a turn-local view; the session total is
// folded exactly once, from core's authoritative per-turn `TurnOutcome`.
// `Chat::fold_turn_usage` is that one implementation, used by `reap` and
// by the record-replay path.

fn sample(cost: f64, input: u64, output: u64) -> lca_protocol::Usage {
    lca_protocol::Usage {
        input,
        output,
        cost,
        ..Default::default()
    }
}

// Verifies: gh #327 acceptance 1+3 — one turn's 0.05 lands as 0.05.
#[test]
fn a_single_turn_folds_its_cost_exactly_once() {
    let mut chat = chat();
    chat.fold_turn_usage(&sample(0.05, 100, 20));
    assert!(
        (chat.usage.cost - 0.05).abs() < 1e-12,
        "one turn of 0.05 must total 0.05, not 0.10: {}",
        chat.usage.cost
    );
    assert_eq!((chat.usage.input, chat.usage.output), (100, 20));
}

// Verifies: gh #327 acceptance 1 — two turns sum, still once each.
#[test]
fn two_turns_sum_without_doubling() {
    let mut chat = chat();
    chat.fold_turn_usage(&sample(0.05, 10, 2));
    chat.fold_turn_usage(&sample(0.07, 20, 3));
    assert!(
        (chat.usage.cost - 0.12).abs() < 1e-12,
        "0.05 + 0.07 = 0.12: {}",
        chat.usage.cost
    );
    assert_eq!((chat.usage.input, chat.usage.output), (30, 5));
}

// Verifies: gh #327 — THE LIVE-STREAM ROW. A stream event must NOT grow the
// session total: only the display state (footer prompt size, meter) moves.
// This is the row that pins the fix's split; on the old handler it moved
// `usage.cost` and doubled against `reap`.
#[test]
fn a_stream_usage_event_updates_display_but_not_the_session_total() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::Usage(sample(0.05, 700, 40)));
    assert_eq!(
        chat.footer.context_used, 700,
        "the live prompt size still updates"
    );
    assert_eq!(
        chat.usage.cost, 0.0,
        "the session total is reap's job, not the stream's"
    );
    assert_eq!(
        (chat.usage.input, chat.usage.output),
        (0, 0),
        "no session counter moves during the stream"
    );
    // The turn-local view carries what the stream saw, for mid-turn display.
    assert!((chat.turn_usage.cost - 0.05).abs() < 1e-12);
    assert_eq!(chat.turn_usage.input, 700);
}

// Verifies: gh #327 + gh #276 — REFINEMENT. A provider reporting in two
// events (0.03 then 0.05 authoritative) must leave ONE turn's 0.05, not
// 0.08. The per-event `+=` on the old handler double-counted refinements.
#[test]
fn a_refined_usage_report_inside_one_turn_does_not_double_count() {
    let mut chat = chat();
    chat.begin_turn_usage();
    chat.on_turn_event(TurnEvent::Usage(sample(0.03, 600, 10)));
    chat.on_turn_event(TurnEvent::Usage(sample(0.05, 600, 25)));
    assert!(
        (chat.turn_usage.cost - 0.05).abs() < 1e-12,
        "a refinement replaces, never adds: {}",
        chat.turn_usage.cost
    );
    chat.fold_turn_usage(&chat.turn_usage.clone());
    assert!(
        (chat.usage.cost - 0.05).abs() < 1e-12,
        "the session total is the turn's 0.05: {}",
        chat.usage.cost
    );
}

// Verifies: gh #327 acceptance 2 — a multi-step turn's total is core's to
// compute. Core SUMS the per-call usage records of one turn
// (`accumulate_usage`), so `TurnOutcome::usage` already carries 1+2+3 = 6
// cents and the fold adds it once. The stream may also see three events;
// its turn-local view reaches the same number by summing, which is why
// `note_turn_step` exists beside `absorb` (a step is a new call's complete
// report; an `absorb` is a refinement within one call).
#[test]
fn a_multi_step_turn_sums_each_steps_report_once() {
    let mut chat = chat();
    chat.begin_turn_usage();
    // Three steps, three complete reports (distinct calls, not refinements).
    for (cost, input, output) in [(0.01, 100, 5), (0.02, 200, 7), (0.03, 300, 9)] {
        let step = sample(cost, input, output);
        chat.on_turn_event(TurnEvent::Usage(step.clone()));
        chat.note_turn_step(&step);
    }
    assert!(
        (chat.turn_usage.cost - 0.06).abs() < 1e-12,
        "1+2+3 cents: {}",
        chat.turn_usage.cost
    );
    assert_eq!(chat.turn_usage.input, 600);
    // Core hands the turn total to `reap`; the fold adds it once.
    chat.fold_turn_usage(&chat.turn_usage.clone());
    assert!((chat.usage.cost - 0.06).abs() < 1e-12);
}

// Verifies: gh #327 — the REPLAY path keeps counting (resume-shaped): a
// resumed session folds each historical record's usage once, and a live
// turn afterwards adds on top rather than replacing.
#[test]
fn replaying_history_still_accumulates() {
    let mut chat = chat();
    let records = vec![
        lca_protocol::Record::Assistant {
            v: lca_protocol::FORMAT_VERSION,
            ts: 1,
            id: "a1".to_string(),
            parent: None,
            content: vec![],
            reasoning: None,
            reasoning_signature: None,
            provider_thinking_level: None,
            usage: Some(sample(0.02, 50, 5)),
            model: None,
            provider: None,
        },
        lca_protocol::Record::Assistant {
            v: lca_protocol::FORMAT_VERSION,
            ts: 2,
            id: "a2".to_string(),
            parent: None,
            content: vec![],
            reasoning: None,
            reasoning_signature: None,
            provider_thinking_level: None,
            usage: Some(sample(0.03, 60, 6)),
            model: None,
            provider: None,
        },
    ];
    chat.load_records(&records, None);
    assert!(
        (chat.usage.cost - 0.05).abs() < 1e-12,
        "resumed history reports its own spend once: {}",
        chat.usage.cost
    );
    assert_eq!(chat.usage.input, 110);
    // A live turn now adds on top.
    chat.fold_turn_usage(&sample(0.04, 70, 7));
    assert!(
        (chat.usage.cost - 0.09).abs() < 1e-12,
        "0.05 history + 0.04 live = 0.09: {}",
        chat.usage.cost
    );
}

// Verifies: gh #327 acceptance 1 — the token counters are single-counted
// alongside cost, in both the cache buckets and the input/output pair.
#[test]
fn token_counters_single_count_alongside_cost() {
    let mut chat = chat();
    chat.fold_turn_usage(&lca_protocol::Usage {
        input: 100,
        output: 20,
        cache_read: 900,
        cache_write: 40,
        cache_write_1h: 7,
        cost: 0.05,
        ..Default::default()
    });
    chat.fold_turn_usage(&lca_protocol::Usage {
        input: 100,
        output: 20,
        cache_read: 900,
        cache_write: 40,
        cache_write_1h: 7,
        cost: 0.05,
        ..Default::default()
    });
    assert_eq!(chat.usage.input, 200);
    assert_eq!(chat.usage.output, 40);
    assert_eq!(chat.usage.cache_read, 1800);
    assert_eq!(chat.usage.cache_write, 80);
    assert_eq!(chat.usage.cache_write_1h, 14);
    assert!((chat.usage.cost - 0.10).abs() < 1e-12);
}

//
// Every turn's cost used to land in the session totals twice: once from the
// stream's `TurnEvent::Usage` handler (which grew `chat.usage.*` including
// `cost`) and again from `TurnState::reap`. Plus, under gh #276's partial
// usage model, the stream-side `+=` double-counted a provider refining its
// totals across two events.
//
// The split: stream events drive display only (footer prompt size, tok/s
// meter, a turn-local view); the session totals fold core's authoritative
// per-turn `TurnOutcome::usage` ONCE through `fold_turn_usage`, which the
// record-replay path shares.
//
// The rows live here rather than in `tests/regressions/` because `UiOptions`
// has no `Default` and `Chat::new` takes the options struct — building it
// again in another crate would duplicate forty fields to test five numbers.

fn usage_(cost: f64, input: u64, output: u64) -> Usage {
    Usage {
        input,
        output,
        cost,
        ..Default::default()
    }
}

// Verifies: gh #327 acceptance 1+3 — a turn reporting 0.05 totals 0.05.
#[test]
fn one_turn_costs_its_own_five_cents() {
    let mut chat = chat();
    chat.fold_turn_usage(&usage_(0.05, 100, 20));
    assert!(
        (chat.usage.cost - 0.05).abs() < 1e-12,
        "$0.05 in, $0.05 out — not $0.10: {}",
        chat.usage.cost
    );
    assert_eq!((chat.usage.input, chat.usage.output), (100, 20));
}

// Verifies: gh #327 acceptance 1 — the stream never moves the session
// totals; only display state. On the old handler `usage.cost` grew here
// and `reap` doubled it.
#[test]
fn the_stream_moves_display_only() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::Usage(usage_(0.05, 700, 40)));
    assert_eq!(chat.footer.context_used, 700, "live prompt size updates");
    assert_eq!(chat.usage.cost, 0.0, "the session total is reap's job");
    assert_eq!((chat.usage.input, chat.usage.output), (0, 0));
    assert!(
        (chat.turn_usage.cost - 0.05).abs() < 1e-12,
        "the turn-local view carries it instead"
    );
}
