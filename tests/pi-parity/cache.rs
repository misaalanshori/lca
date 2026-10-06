//! Cache-waste parity: pi's `cache-stats.ts` method, replayed through LCA's
//! measurement. Token math matches pi exactly; dollar rates come from the
//! turn's own cost buckets (see `docs/pi-parity.md`).

use lca_protocol::{Record, Usage};
use lca_session::compute_cache_waste;

const NOISE_FLOOR: u64 = 1024;

fn usage(input: u64, cache_read: u64, cache_write: u64, cost_input: f64, cost_write: f64) -> Usage {
    Usage {
        input,
        output: 10,
        cache_read,
        cache_write,
        cache_write_1h: 0,
        cost: 0.0,
        cost_input,
        cost_cache_read: 0.0,
        cost_cache_write: cost_write,
        extras: Default::default(),
    }
}

fn assistant(id: &str, ts: u64, model: &str, usage: Usage) -> Record {
    Record::Assistant {
        v: 1,
        ts,
        id: id.to_string(),
        content: vec![],
        reasoning: None,
        model: Some(model.to_string()),
        provider: Some("p".to_string()),
        usage: Some(usage),
    }
}

fn compaction(id: &str) -> Record {
    Record::Compaction {
        v: 1,
        ts: 3,
        id: id.to_string(),
        replaced_from: "u0".to_string(),
        replaced_to: "a0".to_string(),
        first_kept_id: String::new(),
        summary: "summary".to_string(),
        strategy: "parity-harness".to_string(),
        usage: None,
    }
}

// Verifies: pi:packages/coding-agent/test/cache-stats.test.ts#counts-nothing-for-healthy-sessions
#[test]
fn pi_parity_cache_healthy_turns_have_no_waste() {
    let records = vec![
        assistant("a1", 0, "m", usage(0, 0, 100_000, 0.0, 0.375)),
        assistant("a2", 60_000, "m", usage(0, 100_000, 5_000, 0.0, 0.019)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 0,
        "a full cache read bills nothing twice"
    );
    assert_eq!(totals.missed_cost, 0.0);
}

// Verifies: pi:packages/coding-agent/test/cache-stats.test.ts#accumulates-missed-tokens-and-cost-across-turns
#[test]
fn pi_parity_cache_full_miss_bills_the_previous_prompt() {
    let records = vec![
        assistant("a1", 0, "m", usage(0, 0, 100_000, 0.0, 0.375)),
        assistant("a2", 60_000, "m", usage(0, 100_000, 5_000, 0.0, 0.019)),
        assistant("a3", 120_000, "m", usage(0, 0, 110_000, 0.4125, 0.0)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 105_000,
        "the previous 105k prompt, rebilled"
    );
    assert!(
        totals.missed_cost > 0.0,
        "a real miss has a real dollar cost"
    );
}

// Verifies: pi:packages/coding-agent/test/cache-stats.test.ts#skips-the-turn-after-a-compaction-reset
#[test]
fn pi_parity_cache_baseline_resets_on_compaction() {
    let records = vec![
        assistant("a1", 0, "m", usage(0, 0, 100_000, 0.0, 0.375)),
        compaction("c1"),
        assistant("a2", 60_000, "m", usage(0, 0, 20_000, 0.075, 0.0)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 0,
        "a changed prompt is new content, not waste"
    );
}

// Verifies: pi:packages/coding-agent/docs/settings.md (the 1024-token
// noise floor LCA documents beside `cache.noise_floor_tokens`).
#[test]
fn pi_parity_cache_noise_floor_swallows_small_misses() {
    let records = vec![
        assistant("a1", 0, "m", usage(0, 0, 100_000, 0.0, 0.375)),
        assistant("a2", 60_000, "m", usage(0, 99_500, 500, 0.0, 0.0)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 0,
        "a 500-token miss is breakpoint noise"
    );
    assert_eq!(totals.miss_count, 0);
}

// Verifies: pi:packages/coding-agent/src/core/cache-stats.ts (a model
// switch re-bills the full prompt; that cost is real and worth surfacing,
// so no exemption — LCA's `docs/inspiration.md` adopts this explicitly).
#[test]
fn pi_parity_cache_model_change_does_not_reset() {
    let records = vec![
        assistant("a1", 0, "m1", usage(0, 0, 100_000, 0.0, 0.375)),
        assistant("a2", 60_000, "m2", usage(0, 0, 100_000, 0.375, 0.0)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 100_000,
        "the switch is counted, never exempt"
    );
}

// Verifies: pi:packages/coding-agent/src/core/cache-stats.ts (a provider
// that never reported cache activity reads as no measurable waste, not a
// permanent total miss).
#[test]
fn pi_parity_cache_silent_provider_has_no_measurable_waste() {
    let records = vec![
        assistant("a1", 0, "m", usage(50_000, 0, 0, 0.0, 0.0)),
        assistant("a2", 60_000, "m", usage(52_000, 0, 0, 0.0, 0.0)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 0,
        "no cache signal means no measurable waste"
    );
    assert_eq!(totals.miss_count, 0);
}
