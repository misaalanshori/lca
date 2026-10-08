//! Token-derived cost (gh #125): the turn fills a zero reported cost
//! from the curated table for listed models; unlisted models keep
//! tokens-only (zero), and a reported cost is never overwritten.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_testkit::{FakeProvider, fake_usage};

fn priced_config() -> lca_core::AgentConfig {
    lca_core::AgentConfig {
        model: "gpt-4o".to_string(),
        ..default_config()
    }
}

// Verifies: gh #125 - a listed model with no reported cost records the
// table math (1M in @2.50 + 1M out @10.00).
#[tokio::test]
async fn a_listed_model_fills_zero_reported_cost_from_the_table() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("hi").usage(fake_usage(1_000_000, 1_000_000, 0, 0)))
        .build();
    let mut h = harness("priced-fill", provider, priced_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let log = h.store.read(&h.session).expect("read");
    let cost = log
        .records
        .iter()
        .filter_map(|r| match r {
            lca_protocol::Record::Assistant { usage, .. } => usage.as_ref().map(|u| u.cost),
            _ => None,
        })
        .sum::<f64>();
    assert!(
        (cost - 12.50).abs() < 1e-9,
        "table math lands on the record: {cost}"
    );
}

// Verifies: gh #125 - an unlisted model keeps tokens-only: no cost is
// invented, and a provider-reported cost is never overwritten.
#[tokio::test]
async fn unlisted_models_keep_tokens_only_and_reported_cost_wins() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("hi").usage(fake_usage(1_000_000, 1_000_000, 0, 0)))
        .build();
    let mut h = harness("unpriced", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let log = h.store.read(&h.session).expect("read");
    let cost = log
        .records
        .iter()
        .filter_map(|r| match r {
            lca_protocol::Record::Assistant { usage, .. } => usage.as_ref().map(|u| u.cost),
            _ => None,
        })
        .sum::<f64>();
    assert_eq!(cost, 0.0, "faux-1 is unlisted: tokens-only");
}
