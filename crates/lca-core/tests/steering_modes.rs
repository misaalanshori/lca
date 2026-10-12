//! Queue delivery modes (gh #76, pi's `steeringMode` /
//! `followUpMode`): steering drains all-at-once or one per boundary,
//! follow-ups flush one per turn end or all joined.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_protocol::{QueueMode, QueuedMessage, SubmitMode};
use lca_provider::CompletionRequest;
use lca_testkit::{FakeProvider, fake_usage};

fn steer_queue_with(texts: &[&str]) -> lca_protocol::SteerQueue {
    let queue = lca_protocol::steer_queue();
    queue
        .lock()
        .expect("lock")
        .extend(texts.iter().map(|text| QueuedMessage {
            text: text.to_string(),
            mode: SubmitMode::Steer,
        }));
    queue
}

fn request_texts(request: &CompletionRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// Verifies: gh #76 - `one-at-a-time` steering delivers the first
// queued steer at the first boundary and holds the second for the
// next one, in order.
#[tokio::test]
async fn one_at_a_time_steering_drains_one_per_boundary() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("first").usage(fake_usage(10, 5, 0, 0)))
        .turn(|t| t.text("second").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut config = default_config();
    config.steering_mode = QueueMode::OneAtATime;
    config.steer = steer_queue_with(&["steer-one", "steer-two"]);
    let mut h = harness("steer-once", provider, config);
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    for input in ["go", "again"] {
        let outcome = turn(&mut h, input, &mut sink, &mut prompt).await;
        assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    }
    let requests = h.provider.requests();
    assert_eq!(requests.len(), 2);
    let first = request_texts(&requests[0]).join("\n");
    let second = request_texts(&requests[1]).join("\n");
    assert!(first.contains("steer-one"), "first boundary drains one");
    assert!(!first.contains("steer-two"), "the second waits");
    assert!(second.contains("steer-two"), "next boundary drains it");
}

// Verifies: gh #76 - `all` steering (the default) flushes every
// queued steer at the first boundary.
#[tokio::test]
async fn all_steering_flushes_everything_at_once() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("done").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut config = default_config();
    config.steering_mode = QueueMode::All;
    config.steer = steer_queue_with(&["steer-one", "steer-two"]);
    let mut h = harness("steer-all", provider, config);
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    let outcome = turn(&mut h, "go", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let requests = h.provider.requests();
    assert_eq!(requests.len(), 1);
    let texts = request_texts(&requests[0]).join("\n");
    assert!(texts.contains("steer-one") && texts.contains("steer-two"));
    let first = texts.find("steer-one").unwrap();
    let second = texts.find("steer-two").unwrap();
    assert!(first < second, "order preserved");
}
