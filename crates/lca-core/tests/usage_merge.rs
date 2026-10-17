//! Usage accounting across a split provider report (gh #276): a provider
//! may send its counters in more than one stream event, and the two
//! receivers that accumulate them must merge per field rather than
//! overwrite. Split from `loop.rs` / `loop_phases.rs` at the file ceiling
//! (gate 11), which is also where these rows belong: both assert the same
//! contract on the two paths a turn can reach.
//!
//! The defect: Anthropic's SSE lifecycle sends `message_start` with
//! `/input_tokens` and the cache counters (output 0), then
//! `message_delta` with `/output_tokens` (prompt counters absent). A
//! whole-struct assignment left the prompt count at zero, so
//! `maybe_compact` never fired and long sessions overflowed (FR-SESS-4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_protocol::Record;
use lca_session::ViewMode;
use lca_testkit::FakeProvider;
use lca_tools::CompletionBackend as _;
use std::sync::Arc;

// The Anthropic SSE shape as the turn sees it: `message_start` reports
// input/cache with output 0, `message_delta` reports output with the
// prompt counters 0. Reaching the record through the plain streaming
// receiver is what proves the merge, and the prompt count is what
// compaction reads.
// Verifies: gh #276 (a two-event stream keeps both halves; FR-CORE-8)
#[tokio::test]
async fn split_usage_events_merge_into_the_turn_record() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("answer")
                // message_start: the prompt half, output not yet known.
                .usage_event(lca_protocol::Usage {
                    input: 1200,
                    cache_read: 4000,
                    cache_write: 300,
                    cache_write_1h: 64,
                    ..Default::default()
                })
                // message_delta: the output half, prompt counters zeroed.
                .usage(lca_protocol::Usage {
                    output: 87,
                    ..Default::default()
                })
        })
        .build();
    let mut h = harness("split-usage", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };

    let outcome = turn(&mut h, "hi", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        (
            outcome.usage.input,
            outcome.usage.output,
            outcome.usage.cache_read,
            outcome.usage.cache_write,
            outcome.usage.cache_write_1h
        ),
        (1200, 87, 4000, 300, 64),
        "the delta's zeros must not wipe the start's prompt counts"
    );
    assert_eq!(
        outcome.usage.prompt_tokens(),
        1200 + 4000 + 300 + 64,
        "prompt tokens are the MERGED report (gh #276)"
    );

    // The persisted record carries the same merged struct: this is what
    // `maybe_compact` re-reads on the next turn.
    let read = h
        .store
        .read_with(&h.session, ViewMode::Display)
        .expect("records");
    let assistant = read
        .records
        .iter()
        .rev()
        .find_map(|record| match record {
            Record::Assistant { usage, .. } => usage.clone(),
            _ => None,
        })
        .expect("an assistant record");
    assert_eq!(
        (assistant.input, assistant.output, assistant.cache_read),
        (1200, 87, 4000),
        "the session record carries the merged usage, not the last event"
    );
}

// The second receiver: `ProviderBackend` (the `completion` capability
// path) accumulates its own `Usage` while draining the producer's
// channel. A split-usage provider reaches it the same way a split-usage
// provider reaches the turn's own receiver, and a whole-struct
// overwrite here would hand compaction a zero prompt count.
// Verifies: gh #276 (both receivers merge; FR-SESS-5's spend attribution)
#[test]
fn the_completion_backend_merges_split_usage_reports() {
    let fake = Arc::new(
        FakeProvider::builder()
            .turn(|t| {
                t.text("summary")
                    // message_start half.
                    .usage_event(lca_protocol::Usage {
                        input: 1500,
                        cache_read: 6000,
                        ..Default::default()
                    })
                    // message_delta half.
                    .usage(lca_protocol::Usage {
                        output: 40,
                        ..Default::default()
                    })
            })
            .build(),
    );
    let backend = lca_core::ext_provider::ProviderBackend::new(fake, "m", "sess-1");
    let messages = vec![lca_protocol::ChatMessage::text(
        lca_protocol::MessageRole::User,
        "summarize",
    )];
    let (_text, usage) = backend.complete(&messages).expect("the turn answers");
    assert_eq!(
        (usage.input, usage.output, usage.cache_read),
        (1500, 40, 6000),
        "the delta's zeros must not wipe the start's prompt counts"
    );
    let drained = backend.take_usage().expect("spend landed on the record");
    assert_eq!(
        drained.prompt_tokens(),
        1500 + 6000,
        "compaction's own accounting reads the merged report"
    );
}
