//! Token-derived pricing (gh #125, gh #260): when a provider reports no cost,
//! the turn prices its usage from the embedded model catalog.
//!
//! The catalog is `lca_registry::models` - a build-time stripped `models.dev`
//! document plus the user's drop-in at `~/.lca/catalogs/models.json`. This
//! module holds the arithmetic only: it names no vendor and no model, so
//! `lca-core` keeps zero vendor knowledge (gh #260's whole point).
//!
//! ## The buckets
//!
//! Four rates, billed separately because vendors price them separately:
//!
//! * fresh input, at the input rate;
//! * cache reads, heavily discounted by every vendor;
//! * cache writes, which can carry a premium (the Claude line bills 1.25x
//!   input) - split further into the five-minute tier and the one-hour tier,
//!   which Anthropic bills at 2x input;
//! * output.
//!
//! The breakdown is persisted, not just summed, because `lca_session::cache`
//! attributes a cache miss to a dollar figure per bucket (ADR-0017). Before
//! gh #274 those fields stayed `0.0` on every catalog-priced turn, so a cache
//! saving measured as exactly `$0.00`.

use lca_protocol::Usage;
use lca_registry::models::{self, ModelRates};

/// Resolve the rates for `provider`/`model`, or `None` when the catalog has
/// none.
///
/// `None` is the honest answer and the caller's cue to record tokens only: a
/// model nobody verified never gets an invented price (gh #125's rule, kept
/// by gh #260).
#[must_use]
pub fn rates_for(provider: &str, model: &str) -> Option<ModelRates> {
    models::lookup(provider, model)?.rates()
}

/// The per-bucket dollar cost of one usage record under a set of rates.
///
/// The four buckets sum to `total`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricedUsage {
    /// Billed input tokens at the input rate.
    pub cost_input: f64,
    /// Cache-read tokens at the cache-read rate.
    pub cost_cache_read: f64,
    /// Cache-write tokens at the cache-write rate, plus the one-hour tier at
    /// its own (higher) rate.
    pub cost_cache_write: f64,
    /// Output tokens at the output rate.
    pub cost_output: f64,
    /// The four buckets summed.
    pub total: f64,
}

impl PricedUsage {
    /// Price `usage` under `rates`.
    #[must_use]
    pub fn new(rates: ModelRates, usage: &Usage) -> PricedUsage {
        let per_mtok = |tokens: u64, rate: f64| tokens as f64 / 1_000_000.0 * rate;
        let cost_input = per_mtok(usage.input, rates.input_per_mtok);
        let cost_cache_read = per_mtok(usage.cache_read, rates.cache_read_per_mtok);
        // The one-hour tier is a cache write for accounting purposes but not
        // for billing: Anthropic charges 2x input for it against 1.25x for
        // the five-minute tier, so folding them together under-bills a
        // long-lived cache by a wide margin. `cost_cache_write` carries both,
        // since the persisted record has one cache-write cost field.
        let cost_cache_write = per_mtok(usage.cache_write, rates.cache_write_per_mtok)
            + per_mtok(usage.cache_write_1h, rates.cache_write_1h_per_mtok);
        let cost_output = per_mtok(usage.output, rates.output_per_mtok);
        PricedUsage {
            cost_input,
            cost_cache_read,
            cost_cache_write,
            cost_output,
            total: cost_input + cost_cache_read + cost_cache_write + cost_output,
        }
    }
}

/// The token-derived cost of one usage record, or `None` when the catalog has
/// no rates for the model.
#[must_use]
pub fn table_cost(provider: &str, model: &str, usage: &Usage) -> Option<f64> {
    Some(PricedUsage::new(rates_for(provider, model)?, usage).total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One million tokens in every bucket, so each rate reads as a literal.
    fn full_usage() -> Usage {
        Usage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
            cache_write_1h: 0,
            ..Default::default()
        }
    }

    // Verifies: gh #125 - a listed model prices by the catalog; an unlisted
    // one prices to `None` (never a made-up number).
    #[test]
    fn known_models_price_and_unknown_models_do_not() {
        let rates = rates_for("openai", "gpt-4o").expect("the catalog lists gpt-4o");
        assert_eq!(rates.input_per_mtok, 2.50);
        assert_eq!(rates.cache_read_per_mtok, 1.25);
        assert_eq!(rates.cache_write_per_mtok, 2.50, "falls back to input");
        assert_eq!(rates.cache_write_1h_per_mtok, 5.00, "2x input");

        // Profiles qualify the name; the bare id still matches.
        assert_eq!(rates_for("openai", "antigravity/gpt-4o"), Some(rates));
        assert_eq!(rates_for("openai", "GPT-4O"), None, "matching is exact");
        assert!(rates_for("openai", "not-a-real-model").is_none());
        assert!(
            rates_for("fake", "faux-1").is_none(),
            "test models: no entry"
        );
        assert!(rates_for("openai", "").is_none(), "empty: no entry");
    }

    // Verifies: gh #125, gh #274 - the cost math over every bucket, each at
    // its own rate, and the breakdown summing to the total.
    #[test]
    fn cost_math_covers_every_bucket() {
        let rates = rates_for("openai", "gpt-4o").expect("listed");
        let cost = table_cost("openai", "gpt-4o", &full_usage()).expect("listed");
        // 2.50 input + 1.25 cache_read + 2.50 cache_write + 10.00 output.
        assert!(
            (cost - 16.25).abs() < 1e-9,
            "1M per bucket on gpt-4o is 16.25: {cost}"
        );
        let breakdown = PricedUsage::new(rates, &full_usage());
        assert!((breakdown.cost_input - 2.50).abs() < 1e-9);
        assert!((breakdown.cost_cache_read - 1.25).abs() < 1e-9);
        assert!((breakdown.cost_cache_write - 2.50).abs() < 1e-9);
        assert!((breakdown.cost_output - 10.00).abs() < 1e-9);
        assert!(
            (breakdown.total - cost).abs() < 1e-9,
            "the breakdown sums to the total"
        );
        assert!(table_cost("openai", "not-a-real-model", &full_usage()).is_none());
    }

    // Verifies: gh #125's zero-hallucination row - a zero-token usage costs
    // exactly nothing (not "unknown", not epsilon).
    #[test]
    fn zero_tokens_cost_zero() {
        assert_eq!(table_cost("openai", "gpt-4o", &Usage::default()), Some(0.0));
    }

    // Verifies: gh #260 - the same model on two providers is two prices, and
    // the provider argument is what picks between them.
    #[test]
    fn the_provider_argument_selects_the_price() {
        let groq = rates_for("groq", "llama-3.3-70b-versatile").expect("groq lists it");
        assert_eq!(groq.input_per_mtok, 0.59);
        // A provider that does not list it falls back to a canonical row,
        // which is a price from somewhere rather than a guess.
        assert!(rates_for("some-reseller", "llama-3.3-70b-versatile").is_some());
    }

    // Verifies: gh #260 - an unlisted provider still prices a model another
    // provider lists (the issue's canonical fallback).
    #[test]
    fn an_unlisted_provider_uses_the_canonical_fallback() {
        let rates = rates_for("some-reseller", "gpt-4o").expect("the fallback finds it");
        assert_eq!(rates.input_per_mtok, 2.50);
    }
}
