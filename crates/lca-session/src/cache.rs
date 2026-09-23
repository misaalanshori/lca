//! Cache-waste measurement: pi's `cache-stats.ts` adopted directly per
//! ADR-0017, scanning session records for what the prompt cache should
//! have served but didn't.
//!
//! The pass compares each assistant turn's prompt token count against the
//! previous turn's and subtracts what was actually read from cache; the
//! baseline resets on a `compaction` record (the prompt legitimately
//! changed) but is never reset by a model switch (that cost is real and
//! worth surfacing). A provider that never reports cache activity has
//! nothing to measure, and misses under the noise floor are
//! breakpoint-granularity noise rather than regressions.

use lca_protocol::{Record, Usage};

/// A counted cache miss on one assistant turn (ADR-0017).
#[derive(Debug, Clone, PartialEq)]
pub struct CacheMiss {
    /// The record that paid for it.
    pub record_id: String,
    /// Prompt tokens that were in the previous turn's prompt but not read
    /// from cache (FR-CACHE-1).
    pub missed_tokens: u64,
    /// Extra dollars paid versus a full cache hit, from this turn's own
    /// bucket rates;0 when the provider reports no cost breakdown.
    pub missed_cost: f64,
    /// Milliseconds since the previous request (idle gaps explain TTL
    /// expiry).
    pub idle_ms: u64,
    /// Whether the model changed relative to the previous request; a
    /// switch is counted, never exempt (FR-CACHE-2).
    pub model_changed: bool,
}

/// Cumulative cache waste across a scan (FR-CORE-8's status-line sibling).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheWasteTotals {
    /// Counted missed prompt tokens.
    pub missed_tokens: u64,
    /// Counted missed dollars.
    pub missed_cost: f64,
    /// Number of turns whose miss cleared the noise floor (FR-CACHE-3).
    pub miss_count: u64,
}

/// The last request seen by a scan; everything in its prompt should be
/// cacheable next time.
#[derive(Debug, Clone)]
struct PreviousRequest {
    prompt_tokens: u64,
    model_key: String,
    timestamp: u64,
    /// Sticky across the segment: some earlier request reported cache
    /// activity, distinguishing a total miss on a cache-read-only provider
    /// from a provider that never reports caching at all (FR-CACHE-4).
    reported_cache: bool,
}

fn usage_prompt_tokens(usage: &Usage) -> u64 {
    usage.input + usage.cache_read + usage.cache_write + usage.cache_write_1h
}

/// Compute this turn's miss, or `None` when nothing counts.
fn detect_miss(
    previous: &PreviousRequest,
    usage: &Usage,
    model_key: &str,
    timestamp: u64,
    noise_floor: u64,
    record_id: &str,
) -> Option<CacheMiss> {
    let prompt_tokens = usage_prompt_tokens(usage);
    if prompt_tokens == 0 {
        return None;
    }
    // A zero-cache turn only counts when cache activity was reported
    // before: on cache-read-only providers that is a total miss; on
    // providers that never report caching it means nothing (FR-CACHE-4).
    if !usage.reported_cache() && !previous.reported_cache {
        return None;
    }
    let missed = previous
        .prompt_tokens
        .min(prompt_tokens)
        .saturating_sub(usage.cache_read);
    if missed <= noise_floor {
        // FR-CACHE-3: below the noise floor, this is breakpoint noise.
        return None;
    }
    // Missed tokens can only land in the paid or cache-write buckets, so
    // the paid rate comes straight from this turn's own breakdown; the
    // cache-read rate likewise. A provider reporting only totals yields
    // zero dollars but still counts the tokens (pi's shape).
    let paid_tokens = usage.input + usage.cache_write + usage.cache_write_1h;
    let paid_per_token = if paid_tokens > 0 {
        (usage.cost_input + usage.cost_cache_write) / paid_tokens as f64
    } else {
        0.0
    };
    let read_per_token = if usage.cache_read > 0 {
        usage.cost_cache_read / usage.cache_read as f64
    } else {
        0.0
    };
    Some(CacheMiss {
        record_id: record_id.to_string(),
        missed_tokens: missed,
        missed_cost: missed as f64 * (paid_per_token - read_per_token).max(0.0),
        idle_ms: timestamp.saturating_sub(previous.timestamp),
        model_changed: model_key != previous.model_key,
    })
}

fn scan(records: &[Record], noise_floor: u64) -> (CacheWasteTotals, Vec<CacheMiss>) {
    let mut previous: Option<PreviousRequest> = None;
    let mut totals = CacheWasteTotals::default();
    let mut misses = Vec::new();
    for record in records {
        match record {
            // FR-CACHE-2: the prompt legitimately changed; the next turn
            // is new content, not re-billed content. Model switches are
            // deliberately absent from this arm.
            Record::Compaction { .. } => {
                previous = None;
            }
            Record::Assistant {
                id,
                usage: Some(usage),
                model,
                provider,
                ts,
                ..
            } => {
                let Some(previous_request) = previous.as_ref() else {
                    previous = Some(PreviousRequest {
                        prompt_tokens: usage_prompt_tokens(usage),
                        model_key: format!(
                            "{}/{}",
                            provider.as_deref().unwrap_or(""),
                            model.as_deref().unwrap_or("")
                        ),
                        timestamp: *ts,
                        reported_cache: usage.reported_cache(),
                    });
                    continue;
                };
                let model_key = format!(
                    "{}/{}",
                    provider.as_deref().unwrap_or(""),
                    model.as_deref().unwrap_or("")
                );
                if let Some(miss) =
                    detect_miss(previous_request, usage, &model_key, *ts, noise_floor, id)
                {
                    totals.missed_tokens += miss.missed_tokens;
                    totals.missed_cost += miss.missed_cost;
                    totals.miss_count += 1;
                    misses.push(miss);
                }
                previous = Some(PreviousRequest {
                    prompt_tokens: usage_prompt_tokens(usage),
                    model_key,
                    timestamp: *ts,
                    reported_cache: previous_request.reported_cache || usage.reported_cache(),
                });
            }
            _ => {}
        }
    }
    (totals, misses)
}

/// Cumulative cache waste across a session's records (FR-CACHE-1).
pub fn compute_cache_waste(records: &[Record], noise_floor: u64) -> CacheWasteTotals {
    scan(records, noise_floor).0
}

/// Every counted miss in order, for per-turn notices (ADR-0017: surface,
/// don't block).
pub fn collect_cache_misses(records: &[Record], noise_floor: u64) -> Vec<CacheMiss> {
    scan(records, noise_floor).1
}
