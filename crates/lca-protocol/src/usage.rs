//! Token usage and cost accounting for one provider response.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Token counts and cost for one completion call.
///
/// Cache fields mirror ADR-0017: `cache_write_1h` mirrors Anthropic's
/// extended one-hour cache tier. Fields a provider does not report stay at
/// zero; `extras` carries non-structural additions without an ABI break
/// (`docs/abi-versioning.md`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Billed input tokens not served from cache.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Input tokens served from the prompt cache.
    pub cache_read: u64,
    /// Input tokens written to the prompt cache this call.
    pub cache_write: u64,
    /// Input tokens written to the extended (one-hour) cache tier.
    pub cache_write_1h: u64,
    /// Total cost in USD, as reported or computed by the provider extension.
    pub cost: f64,
    /// Reserved map for non-structural extensions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
}

impl Usage {
    /// Prompt tokens for this call: everything the provider counted as input,
    /// including tokens read from or written to cache.
    ///
    /// This is the number cache-waste measurement compares turn over turn
    /// (ADR-0017, `cache-stats.ts`'s `promptTokens`).
    pub fn prompt_tokens(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.cache_write_1h
    }

    /// Whether this usage record shows any cache activity at all.
    pub fn reported_cache(&self) -> bool {
        self.cache_read + self.cache_write + self.cache_write_1h > 0
    }
}
