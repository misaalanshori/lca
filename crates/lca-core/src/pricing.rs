//! Curated model pricing (gh #125): per-1M-token USD rates from
//! `resources/pricing.toml`, driving token-derived cost where the
//! provider reports none. Only verified entries exist; an unlisted
//! model prices to `None` (tokens-only downstream), never to a guess.

use std::collections::HashMap;
use std::sync::OnceLock;

/// USD per 1M tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    /// Billed input tokens not served from cache.
    pub input_per_mtok: f64,
    /// Input tokens served from the prompt cache. Vendors discount these
    /// heavily (gh #274); an entry that omits the rate falls back to
    /// `input_per_mtok`.
    pub cache_read_per_mtok: f64,
    /// Input tokens written to the prompt cache. Some vendors charge a
    /// premium over fresh input (the Claude line bills 1.25x).
    pub cache_write_per_mtok: f64,
    /// Output side.
    pub output_per_mtok: f64,
}

fn table() -> &'static HashMap<String, Price> {
    static TABLE: OnceLock<HashMap<String, Price>> = OnceLock::new();
    TABLE.get_or_init(|| {
        // A file we ship: a typo here must not panic a running agent.
        // Malformed content prices to an empty table (tokens-only
        // everywhere), and the non-empty test below fails the build.
        let parsed: toml::Table =
            toml::from_str(include_str!("../resources/pricing.toml")).unwrap_or_default();
        let mut map = HashMap::new();
        for (model, rates) in &parsed {
            let input = rates.get("input").and_then(|v| v.as_float());
            let output = rates.get("output").and_then(|v| v.as_float());
            if let (Some(input), Some(output)) = (input, output) {
                // An entry that predates the cache columns keeps its old
                // flat behaviour rather than pricing cache at zero.
                let cache_read = rates
                    .get("cache_read")
                    .and_then(|v| v.as_float())
                    .unwrap_or(input);
                let cache_write = rates
                    .get("cache_write")
                    .and_then(|v| v.as_float())
                    .unwrap_or(input);
                map.insert(
                    model.to_ascii_lowercase(),
                    Price {
                        input_per_mtok: input,
                        cache_read_per_mtok: cache_read,
                        cache_write_per_mtok: cache_write,
                        output_per_mtok: output,
                    },
                );
            }
        }
        map
    })
}

/// The curated price for a configured model name, if listed. A
/// `profile/` prefix is stripped first (`antigravity/gpt-4o` prices as
/// `gpt-4o`); matching lowercases. Unlisted names price to `None`.
pub fn price_for(model: &str) -> Option<Price> {
    let bare = model.rsplit('/').next().unwrap_or(model);
    table().get(&bare.to_ascii_lowercase()).copied()
}

/// The per-bucket dollar cost of one usage record under a price.
///
/// The four buckets sum to `total`. The turn loop copies three of them onto
/// the usage record it persists, because `lca_session::cache` attributes a
/// cache miss to a dollar figure per bucket (ADR-0017) - before gh #274 those
/// fields stayed `0.0` on every table-priced turn, so a cache saving measured
/// as exactly `$0.00`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricedUsage {
    /// Billed input tokens at the input rate.
    pub cost_input: f64,
    /// Cache-read tokens at the cache-read rate.
    pub cost_cache_read: f64,
    /// Cache-write tokens (both tiers) at the cache-write rate.
    pub cost_cache_write: f64,
    /// Output tokens at the output rate.
    pub cost_output: f64,
    /// The four buckets summed.
    pub total: f64,
}

impl PricedUsage {
    /// Price `usage` under `price`.
    #[must_use]
    pub fn new(price: Price, usage: &lca_protocol::Usage) -> PricedUsage {
        let per_mtok = |tokens: u64, rate: f64| tokens as f64 / 1_000_000.0 * rate;
        let cost_input = per_mtok(usage.input, price.input_per_mtok);
        let cost_cache_read = per_mtok(usage.cache_read, price.cache_read_per_mtok);
        // The one-hour tier is still a cache write; it bills at the
        // cache-write rate rather than at a rate of its own.
        let cost_cache_write = per_mtok(
            usage.cache_write + usage.cache_write_1h,
            price.cache_write_per_mtok,
        );
        let cost_output = per_mtok(usage.output, price.output_per_mtok);
        PricedUsage {
            cost_input,
            cost_cache_read,
            cost_cache_write,
            cost_output,
            total: cost_input + cost_cache_read + cost_cache_write + cost_output,
        }
    }
}

/// The token-derived cost of one usage record under a model name.
///
/// Each bucket bills at its own rate: `cache_read` is discounted, and
/// `cache_write` can carry a premium. `None` when the model is unlisted.
#[must_use]
pub fn table_cost(model: &str, usage: &lca_protocol::Usage) -> Option<f64> {
    Some(PricedUsage::new(price_for(model)?, usage).total)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #125 - a listed model prices by the book; an
    // unlisted one prices to `None` (never a made-up number).
    #[test]
    fn known_models_price_and_unknown_models_do_not() {
        let price = price_for("gpt-4o").expect("listed");
        // SANCTIONED CHANGE (gh #274): the literal now names the cache
        // rates, because `Price` carries them. gpt-4o bills cache reads at
        // half of input and cache writes at the input rate.
        assert_eq!(
            price,
            Price {
                input_per_mtok: 2.50,
                cache_read_per_mtok: 1.25,
                cache_write_per_mtok: 2.50,
                output_per_mtok: 10.00,
            }
        );
        // Profiles qualify the name; the bare id still matches.
        assert_eq!(price_for("antigravity/gpt-4o"), Some(price));
        assert_eq!(price_for("GPT-4O"), Some(price));
        assert_eq!(price_for("gpt-5"), None, "unverified: tokens-only");
        assert_eq!(price_for("faux-1"), None, "test models: tokens-only");
        assert_eq!(price_for(""), None, "empty: tokens-only");
    }

    // Verifies: gh #125, gh #274 - the cost math over every bucket, each at
    // its own rate, and the breakdown summing to the total.
    #[test]
    fn cost_math_covers_every_bucket() {
        let usage = lca_protocol::Usage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
            cache_write_1h: 0,
            ..Default::default()
        };
        // SANCTIONED CHANGE (gh #274): cache reads bill at 1.25, not at the
        // 2.50 input rate, so the total is 16.25 rather than 17.50.
        let cost = table_cost("gpt-4o", &usage).expect("listed");
        assert!(
            (cost - (2.50 + 1.25 + 2.50 + 10.00)).abs() < 1e-9,
            "1M input @2.50 + 1M cache_read @1.25 + 1M cache_write @2.50 + 1M output @10.00 = 16.25: {cost}"
        );
        let breakdown =
            super::PricedUsage::new(super::price_for("gpt-4o").expect("listed"), &usage);
        assert!((breakdown.cost_input - 2.50).abs() < 1e-9);
        assert!((breakdown.cost_cache_read - 1.25).abs() < 1e-9);
        assert!((breakdown.cost_cache_write - 2.50).abs() < 1e-9);
        assert!((breakdown.cost_output - 10.00).abs() < 1e-9);
        assert!(
            (breakdown.total - cost).abs() < 1e-9,
            "the breakdown sums to the total"
        );
        assert_eq!(table_cost("unknown-model", &usage), None);
    }

    // Verifies: gh #125, the zero-hallucination row - the table is
    // non-empty (a typo'd file fails here, not in production), every
    // entry carries positive rates, and a zero-token usage costs
    // exactly nothing (not "unknown", not epsilon).
    #[test]
    fn zero_tokens_cost_zero_and_every_entry_is_positive() {
        assert!(!table().is_empty(), "the curated table parsed");
        let empty = lca_protocol::Usage::default();
        assert_eq!(table_cost("gpt-4o", &empty), Some(0.0));
        for (model, price) in table().iter() {
            assert!(
                price.input_per_mtok > 0.0
                    && price.output_per_mtok > 0.0
                    && price.cache_read_per_mtok > 0.0
                    && price.cache_write_per_mtok > 0.0,
                "{model} carries positive rates for every bucket"
            );
        }
    }
}
