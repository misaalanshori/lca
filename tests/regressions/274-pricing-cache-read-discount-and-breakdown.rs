//! Regression rows for gh #274: the curated price table must bill cache
//! reads at their own rate and must populate the per-bucket cost breakdown.
//!
//! Two defects in one path:
//!
//! 1. `resources/pricing.toml` carried only `input` and `output`, so
//!    `table_cost` billed `cache_read` and `cache_write` tokens at 100% of the
//!    input rate. Every real vendor discounts cache *reads* heavily - 50% for
//!    GPT-4o, 90% for the Claude line - so a cache-heavy turn was overpriced
//!    by up to 10x, and the saving a cache actually produced was invisible.
//! 2. `turn/mod.rs` assigned only `response.usage.cost`, leaving
//!    `cost_input`, `cost_cache_read`, and `cost_cache_write` at `0.0`. Those
//!    three fields are exactly what `lca_session::cache` reads to compute
//!    dollar cost per cache miss (ADR-0017), so on any table-priced model the
//!    cache-waste figure was always `$0.00`.
//!
//! Verifies: gh #274 - cache reads are discounted, and the breakdown lands.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_core::pricing::{Price, price_for, table_cost};
use lca_protocol::Usage;

/// Build a usage record with explicit buckets.
fn usage(input: u64, cache_read: u64, cache_write: u64, output: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        ..Default::default()
    }
}

/// The published rates are in the table. A missing rate would silently fall
/// back to the input rate and reproduce the defect.
#[test]
fn the_curated_table_carries_cache_rates() {
    // gpt-4o: input 2.50, cache read 1.25 (half), output 10.00.
    let price = price_for("gpt-4o").expect("gpt-4o is listed");
    assert_eq!(price.input_per_mtok, 2.50);
    assert_eq!(price.output_per_mtok, 10.00);
    assert_eq!(
        price.cache_read_per_mtok, 1.25,
        "the published cache-read rate, not the input rate"
    );
    assert_eq!(
        price.cache_write_per_mtok, 2.50,
        "gpt-4o cache writes bill at the input rate"
    );
}

/// Claude's cache read is a tenth of input and its write carries a 25%
/// premium - the shape that makes the defect most visible.
#[test]
fn a_premium_cache_write_rate_is_carried() {
    let price = price_for("claude-sonnet-4-5").expect("listed");
    assert_eq!(price.input_per_mtok, 3.00);
    assert_eq!(price.cache_read_per_mtok, 0.30);
    assert_eq!(
        price.cache_write_per_mtok, 3.75,
        "a cache write costs 1.25x input on this line"
    );
}

/// One million tokens in each bucket of gpt-4o:
/// `2.50 + 1.25 + 2.50 + 10.00 = 16.25`.
///
/// Before the fix, `cache_read` rode the input rate, so the total was
/// `2.50 + 2.50 + 2.50 + 10.00 = 17.50`.
#[test]
fn cache_reads_are_billed_at_the_discounted_rate() {
    let cost =
        table_cost("gpt-4o", &usage(1_000_000, 1_000_000, 1_000_000, 1_000_000)).expect("listed");
    assert!(
        (cost - 16.25).abs() < 1e-9,
        "2.50 input + 1.25 cache_read + 2.50 cache_write + 10.00 output = 16.25, got {cost}"
    );
}

/// A cache-heavy turn on Claude: mostly cache reads, so the total must be far
/// below what the un-discounted math would give.
#[test]
fn a_cache_heavy_turn_is_cheaper_than_the_input_rate() {
    // 1M cache reads, no other input, 1M output.
    let cost = table_cost("claude-sonnet-4-5", &usage(0, 1_000_000, 0, 1_000_000)).expect("listed");
    // 0.30 + 15.00 = 15.30, versus 3.00 + 15.00 = 18.00 at the input rate.
    assert!((cost - 15.30).abs() < 1e-9, "got {cost}");
}

/// The one-hour cache tier has its own rate: Anthropic bills it at 2x input,
/// against 1.25x for the five-minute tier. Folding the two together (the
/// first draft of gh #274 did) under-bills a long-lived cache badly.
#[test]
fn the_one_hour_cache_tier_bills_at_twice_input() {
    let usage = Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: 1_000_000,
        ..Default::default()
    };
    let cost = table_cost("claude-sonnet-4-5", &usage).expect("listed");
    assert!(
        (cost - 6.00).abs() < 1e-9,
        "1M one-hour writes on claude-sonnet-4-5 bill at 2x input (6.00),          not the 3.75 five-minute rate: {cost}"
    );
    // And the five-minute tier is unchanged. Built fresh: `..usage` would
    // carry the one-hour count along and this assertion would silently be
    // measuring both tiers (which is how the first draft of this row failed).
    let short = Usage {
        cache_write: 1_000_000,
        cache_write_1h: 0,
        ..Default::default()
    };
    let short_cost = table_cost("claude-sonnet-4-5", &short).expect("listed");
    assert!((short_cost - 3.75).abs() < 1e-9, "5m tier: {short_cost}");
}

/// The two write tiers are distinct on the same record.
#[test]
fn the_two_write_tiers_are_priced_separately() {
    let both = Usage {
        cache_write: 1_000_000,
        cache_write_1h: 1_000_000,
        ..Default::default()
    };
    let cost = table_cost("claude-sonnet-4-5", &both).expect("listed");
    assert!(
        (cost - (3.75 + 6.00)).abs() < 1e-9,
        "1M five-minute (3.75) + 1M one-hour (6.00) = 9.75: {cost}"
    );
}

/// An unlisted model still prices to `None` - the fix must not invent a
/// number for a model nobody verified.
#[test]
fn an_unlisted_model_still_prices_to_none() {
    assert_eq!(
        table_cost("unknown-model", &usage(1_000_000, 0, 0, 0)),
        None
    );
    assert_eq!(price_for("faux-1"), None);
}

/// Every curated entry carries positive cache rates: a typo'd or missing rate
/// in the TOML would fall back to the input rate and quietly resurrect the
/// defect, so the table is checked as a whole.
#[test]
fn every_entry_carries_positive_cache_rates() {
    // The curated ids, spelled out rather than discovered, so a dropped entry
    // fails here instead of silently shrinking the coverage.
    for model in [
        "gpt-4o",
        "gpt-4o-mini",
        "gpt-4.1",
        "claude-sonnet-4-5",
        "claude-opus-4-1",
        "claude-haiku-4-5",
        "gemini-2.5-pro",
        "gemini-2.5-flash",
    ] {
        let price = price_for(model).unwrap_or_else(|| panic!("{model} is listed"));
        assert!(
            price.cache_read_per_mtok > 0.0,
            "{model} carries a positive cache-read rate"
        );
        assert!(
            price.cache_write_per_mtok > 0.0,
            "{model} carries a positive cache-write rate"
        );
        assert!(
            price.cache_read_per_mtok <= price.input_per_mtok,
            "{model}: a cache read is never dearer than fresh input"
        );
        assert!(
            price.cache_write_1h_per_mtok > 0.0,
            "{model} carries a positive one-hour cache-write rate"
        );
        assert!(
            price.cache_write_1h_per_mtok >= price.cache_write_per_mtok,
            "{model}: the one-hour tier is never cheaper than the five-minute one"
        );
    }
}

/// A zero-token usage costs exactly nothing, not epsilon.
#[test]
fn zero_tokens_cost_zero() {
    assert_eq!(table_cost("gpt-4o", &Usage::default()), Some(0.0));
}

/// The breakdown a table-priced turn must populate. This row states the
/// contract the turn loop relies on: the four buckets sum to the total, so
/// `lca_session::cache` can attribute a miss to a dollar figure.
#[test]
fn the_breakdown_sums_to_the_total() {
    let usage = usage(1_000_000, 500_000, 250_000, 2_000_000);
    let price = price_for("claude-opus-4-1").expect("listed");
    let buckets = lca_core::pricing::PricedUsage::new(price, &usage);
    assert!(
        (buckets.cost_input - 15.00).abs() < 1e-9,
        "{}",
        buckets.cost_input
    );
    assert!(
        (buckets.cost_cache_read - 0.75).abs() < 1e-9,
        "0.5M at 1.50: {}",
        buckets.cost_cache_read
    );
    assert!(
        (buckets.cost_cache_write - 4.6875).abs() < 1e-9,
        "0.25M at 18.75: {}",
        buckets.cost_cache_write
    );
    // 2M output at 75.00 dominates: 15 + 0.75 + 4.6875 + 150 = 170.4375.
    assert!(
        (buckets.cost_output - 150.0).abs() < 1e-9,
        "2M at 75.00: {}",
        buckets.cost_output
    );
    assert!((buckets.total - 170.4375).abs() < 1e-9, "{}", buckets.total);
    let summed = buckets.cost_input
        + buckets.cost_cache_read
        + buckets.cost_cache_write
        + buckets.cost_output;
    assert!(
        (summed - buckets.total).abs() < 1e-9,
        "the buckets sum to the total"
    );
}

/// The `Price` literal a caller constructs still has to state the cache rates,
/// so a new entry cannot be added with only input and output.
#[test]
fn a_price_literal_names_both_cache_rates() {
    let price = Price {
        input_per_mtok: 1.0,
        cache_read_per_mtok: 0.5,
        cache_write_per_mtok: 1.25,
        cache_write_1h_per_mtok: 2.0,
        output_per_mtok: 2.0,
    };
    assert_eq!(price.cache_read_per_mtok, 0.5);
}

/// The wiring half: a table-priced turn must persist the breakdown on the
/// assistant record, not just the total. This is the row that would have
/// caught the defect at the turn level - `lca_session::cache` reads these
/// three fields, and with them at `0.0` the cache-waste figure was always
/// `$0.00` no matter how much was actually wasted.
#[test]
fn a_table_priced_turn_persists_the_per_bucket_breakdown() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use lca_core::{Agent, AgentConfig};
    use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff};
    use lca_session::{SessionStore, ViewMode};
    use lca_testkit::{FakeProvider, fake_usage};
    use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

    struct Sink;
    impl lca_core::TurnSink for Sink {
        fn on_event(&mut self, _event: lca_core::TurnEvent) {}
    }
    struct Prompt;
    impl PermissionPrompt for Prompt {
        fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
            Decision::Denied
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let root = lca_testkit::scratch_path("regression-274-breakdown");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = SessionStore::new(root.join("data"));
        let session = store.create_session(&project, "test").expect("session");
        let grants = Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        ));
        let mut tools = ToolExecutor::new(
            Arc::new(NativeOps::default()),
            project.clone(),
            project.clone(),
            65536,
            Some(Duration::from_secs(30)),
        );
        // 1M input, 1M cache_read, 1M cache_write, 1M output on gpt-4o, with
        // the provider reporting no dollar cost so the table must price it.
        let provider = FakeProvider::builder()
            .turn(|t| {
                t.text("ok")
                    .usage(fake_usage(1_000_000, 1_000_000, 1_000_000, 1_000_000))
            })
            .build();
        let config = AgentConfig {
            provider: "fake".to_string(),
            model: "gpt-4o".to_string(),
            retry_limit: 1,
            retry_base_delay: Duration::ZERO,
            max_iterations: 10,
            ..AgentConfig::default()
        };
        let mut sink = Sink;
        let mut prompt = Prompt;
        let mut agent = Agent::new(
            &store,
            &session,
            &provider,
            &mut tools,
            grants.clone(),
            &mut prompt,
            None,
            config,
        );
        let outcome = agent.run_turn("hello", &mut sink, &CancelFlag::new()).await;
        assert_eq!(outcome.status, lca_core::TurnStatus::Ok);

        let usage = store
            .read_with(&session, ViewMode::Display)
            .expect("records")
            .records
            .iter()
            .find_map(|record| match record {
                lca_protocol::Record::Assistant { usage, .. } => usage.clone(),
                _ => None,
            })
            .expect("an assistant record with usage");
        assert!(
            (usage.cost - 16.25).abs() < 1e-9,
            "the total is the discounted sum: {}",
            usage.cost
        );
        assert!(
            (usage.cost_input - 2.50).abs() < 1e-9,
            "cost_input persisted: {}",
            usage.cost_input
        );
        assert!(
            (usage.cost_cache_read - 1.25).abs() < 1e-9,
            "cost_cache_read persisted (was 0.0 before gh #274): {}",
            usage.cost_cache_read
        );
        assert!(
            (usage.cost_cache_write - 2.50).abs() < 1e-9,
            "cost_cache_write persisted (was 0.0 before gh #274): {}",
            usage.cost_cache_write
        );
    });
}
