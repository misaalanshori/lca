//! Cache-waste scenarios from docs/testing-plan.md section9, scripted
//! straight into record sequences: the scan must reproduce pi's
//! measurement (ADR-0017) turn for turn.

use lca_protocol::{ContentBlock, FORMAT_VERSION, Record, Usage};
use lca_session::{CacheMiss, compute_cache_waste};

const NOISE_FLOOR: u64 = 1024;

fn assistant(id: &str, ts: u64, usage: Usage) -> Record {
    Record::Assistant {
        v: FORMAT_VERSION,
        ts,
        id: id.to_string(),
        content: vec![ContentBlock::Text { text: "ok".into() }],
        reasoning: None,
        model: Some("model-a".into()),
        provider: Some("fake".into()),
        usage: Some(usage),
    }
}

/// A clean turn: `prev_prompt` tokens come from the cache, the rest are
/// billed input, and the prompt total is exactly `prompt`
/// (input + cache_read + cache_write, the accounting ADR-0017 measures).
fn clean_turn(id: &str, ts: u64, prompt: u64, prev_prompt: u64) -> Record {
    assistant(
        id,
        ts,
        Usage {
            input: prompt - prev_prompt,
            output: 20,
            cache_read: prev_prompt,
            cache_write: 0,
            ..Usage::default()
        },
    )
}

// Verifies: FR-CACHE-1 (waste compares each turn's prompt count against
// the previous turn's, minus what was actually read from cache).
#[test]
fn clean_conversation_reports_zero_waste_from_turn_two_on() {
    let records = vec![
        clean_turn("t1", 1_000, 1_000, 0),
        clean_turn("t2", 2_000, 2_000, 1_000),
        clean_turn("t3", 3_000, 3_000, 2_000),
        clean_turn("t4", 4_000, 4_000, 3_000),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(totals.miss_count, 0, "{totals:?}");
    assert_eq!(totals.missed_tokens, 0);
}

// A real regression: the prompt grew but nothing was read from cache, so
// the whole prior prompt was re-billed.
#[test]
fn a_full_cache_miss_is_counted_with_its_tokens() {
    let records = vec![
        clean_turn("t1", 1_000, 5_000, 0),
        // Second turn: prompt6000, cache_read0: the whole previous prompt
        // is re-billed. The new content lands in the cache-write bucket,
        // which is exactly where the paid rate comes from.
        assistant(
            "t2",
            2_000,
            Usage {
                input: 0,
                output: 20,
                cache_read: 0,
                cache_write: 6_000,
                cost: 0.054,
                cost_cache_write: 0.054,
                ..Usage::default()
            },
        ),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(totals.miss_count, 1);
    assert_eq!(
        totals.missed_tokens, 5_000,
        "min(prev5000, now6000) - read0"
    );
    assert!(
        totals.missed_cost > 0.0,
        "cost from the turn's own rates: {}",
        totals.missed_cost
    );
    let misses = lca_session::collect_cache_misses(&records, NOISE_FLOOR);
    assert_eq!(misses.len(), 1);
    assert!(matches!(
        &misses[0],
        CacheMiss {
            model_changed: false,
            ..
        }
    ));
}

// Verifies: FR-CACHE-2 (a compaction record resets the baseline; the turn
// after it reports no waste), and model switches are NOT exempt.
#[test]
fn compaction_resets_the_baseline_but_model_switches_do_not() {
    let records = vec![
        clean_turn("t1", 1_000, 4_000, 0),
        // Compaction: the next prompt legitimately changes shape.
        Record::Compaction {
            v: FORMAT_VERSION,
            ts: 1_500,
            id: "c1".into(),
            replaced_from: "t1".into(),
            replaced_to: "t1".into(),
            summary: "summarized".into(),
            strategy: "compaction-default".into(),
            usage: None,
        },
        // Post-compaction prompt is small and fully cache-written: no miss
        // may be attributed to the reset.
        clean_turn("t2", 2_000, 3_000, 0),
        // Model switch: same shape, but the baseline from t2 exists, so a
        // switch turn re-bills the full prompt and IS counted.
        Record::Assistant {
            v: FORMAT_VERSION,
            ts: 3_000,
            id: "t3".into(),
            content: vec![ContentBlock::Text { text: "ok".into() }],
            reasoning: None,
            model: Some("model-b".into()),
            provider: Some("fake".into()),
            usage: Some(Usage {
                input: 900,
                output: 20,
                cache_read: 0,
                cache_write: 900,
                cost: 0.01,
                cost_input: 0.009,
                ..Usage::default()
            }),
        },
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.miss_count, 1,
        "only the model-switch turn counts, baseline reset landed on the compaction: {totals:?}"
    );
    let misses = collect(&records);
    assert!(
        misses.iter().any(|m| m.model_changed),
        "the switch is flagged, not exempt"
    );
}

// Verifies: FR-CACHE-3 (a miss below the1024-token noise floor is not
// counted).
#[test]
fn misses_below_the_noise_floor_stay_uncounted() {
    let records = vec![
        clean_turn("t1", 1_000, 3_000, 0),
        // Prompt3500 with a partial read: missed = min(3000,3500) -2400 =600.
        assistant(
            "t2",
            2_000,
            Usage {
                input: 1_100,
                output: 20,
                cache_read: 2_400,
                cache_write: 1_100,
                ..Usage::default()
            },
        ),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.miss_count, 0,
        "600 tokens is breakpoint noise: {totals:?}"
    );
    assert_eq!(totals.missed_tokens, 0);
}

// Verifies: FR-CACHE-4 (a provider that never reported cache activity has
// nothing to measure, rather than a permanent100% miss rate).
#[test]
fn a_provider_that_never_reports_cache_reports_no_waste() {
    let records = vec![clean_turn("t1", 1_000, 5_000, 0)];
    // Every turn writes cache fields as zero: never any activity.
    let records: Vec<Record> = records
        .into_iter()
        .chain([
            zero_cache_turn("t2", 2_000, 6_000),
            zero_cache_turn("t3", 3_000, 7_000),
        ])
        .collect();
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.miss_count, 0,
        "no cache activity is not a miss: {totals:?}"
    );
    assert_eq!(totals.missed_tokens, 0);
}

fn zero_cache_turn(id: &str, ts: u64, prompt: u64) -> Record {
    Record::Assistant {
        v: FORMAT_VERSION,
        ts,
        id: id.to_string(),
        content: vec![ContentBlock::Text { text: "ok".into() }],
        reasoning: None,
        model: Some("model-a".into()),
        provider: Some("fake".into()),
        usage: Some(Usage {
            input: prompt,
            output: 20,
            cache_read: 0,
            cache_write: 0,
            ..Usage::default()
        }),
    }
}

fn collect(records: &[Record]) -> Vec<CacheMiss> {
    lca_session::collect_cache_misses(records, NOISE_FLOOR)
}
