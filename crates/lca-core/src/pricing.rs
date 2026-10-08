//! Curated model pricing (gh #125): per-1M-token USD rates from
//! `resources/pricing.toml`, driving token-derived cost where the
//! provider reports none. Only verified entries exist; an unlisted
//! model prices to `None` (tokens-only downstream), never to a guess.

use std::collections::HashMap;
use std::sync::OnceLock;

/// USD per 1M tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    /// Billed input side, cache buckets included (see the table header).
    pub input_per_mtok: f64,
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
                map.insert(
                    model.to_ascii_lowercase(),
                    Price {
                        input_per_mtok: input,
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

/// The token-derived cost of one usage record under a model name:
/// input-side buckets (input, cache reads and writes) at the input
/// rate, output at the output rate. `None` when the model is unlisted.
pub fn table_cost(model: &str, usage: &lca_protocol::Usage) -> Option<f64> {
    let price = price_for(model)?;
    let input_side = usage.input + usage.cache_read + usage.cache_write + usage.cache_write_1h;
    Some(
        input_side as f64 / 1_000_000.0 * price.input_per_mtok
            + usage.output as f64 / 1_000_000.0 * price.output_per_mtok,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #125 - a listed model prices by the book; an
    // unlisted one prices to `None` (never a made-up number).
    #[test]
    fn known_models_price_and_unknown_models_do_not() {
        let price = price_for("gpt-4o").expect("listed");
        assert_eq!(
            price,
            Price {
                input_per_mtok: 2.50,
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

    // Verifies: gh #125 - the cost math over every bucket; cache
    // rides the input rate per the table header.
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
        let cost = table_cost("gpt-4o", &usage).expect("listed");
        assert!(
            (cost - (3.0 * 2.50 + 10.00)).abs() < 1e-9,
            "3M input-side @2.50 + 1M output @10.00: {cost}"
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
                price.input_per_mtok > 0.0 && price.output_per_mtok > 0.0,
                "{model} carries positive rates"
            );
        }
    }
}
