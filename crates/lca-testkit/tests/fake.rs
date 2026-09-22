//! Fake provider tests: the scripted stand-in every later phase depends on.

use lca_protocol::{StreamEvent, Usage};
use lca_testkit::{FakeProvider, Provider, fake_usage};

// The testing plan makes per-turn usage records mandatory in the builder: a
// test that scripts a realistic conversation without cache numbers scripts
// something that could not happen against a real caching provider.
#[test]
#[should_panic(expected = "missing a mandatory usage record")]
fn every_scripted_turn_carries_usage() {
    FakeProvider::builder()
        .turn(|t| t.text("no usage here"))
        .build();
}

#[test]
fn streams_scripted_turns_in_order() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("hello ")
                .text("world")
                .usage(fake_usage(100, 20, 0, 100))
        })
        .turn(|t| {
            t.tool_call("read", r#"{"path":"a.txt"}"#)
                .usage(fake_usage(150, 30, 100, 0))
        })
        .build();

    let events = provider.run_next_turn_blocking();
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "hello world");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::Usage { .. }))
    );

    let events = provider.run_next_turn_blocking();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolCallStart { .. }))
    );
}

#[test]
fn reports_usage_with_cache_fields() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.text("x").usage(Usage {
                input: 40,
                output: 12,
                cache_read: 1200,
                cache_write: 0,
                ..Usage::default()
            })
        })
        .build();
    let events = provider.run_next_turn_blocking();
    let usage = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Usage { usage } => Some(usage.clone()),
            _ => None,
        })
        .expect("usage event");
    assert_eq!(usage.cache_read, 1200);
    assert_eq!(usage.prompt_tokens(), 40 + 1200);
}

// The failure paths FR-PROV-8 and the accumulator tests need; a real
// provider will not reliably reproduce them on demand (testing plan s.3).
#[test]
fn scripts_failure_paths() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.error("transport reset", true)
                .usage(fake_usage(10, 0, 0, 0))
        })
        .turn(|t| {
            t.open_tool_call("shell", r#"{"command":"ec"#)
                .usage(fake_usage(10, 5, 0, 0))
        })
        .turn(|t| {
            t.capability_denied("net", "api.example.com")
                .usage(fake_usage(10, 5, 0, 0))
        })
        .build();

    let events = provider.run_next_turn_blocking();
    assert!(
        events.iter().any(|e| matches!(
            e,
            StreamEvent::Error {
                retryable: true,
                ..
            }
        )),
        "retryable transport error"
    );

    let events = provider.run_next_turn_blocking();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolCallEnd { .. })),
        "call stays open"
    );

    let events = provider.run_next_turn_blocking();
    assert!(
        events.iter().any(|e| matches!(
            e,
            StreamEvent::Error {
                retryable: false,
                ..
            }
        )) || events
            .iter()
            .any(|e| matches!(e, StreamEvent::VendorEvent { .. })),
        "capability denial surfaces as an error or vendor event"
    );
}

#[test]
fn exhaustion_is_an_error_not_a_panic() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("one").usage(fake_usage(1, 1, 0, 0)))
        .build();
    let _ = provider.run_next_turn_blocking();
    let events = provider.run_next_turn_blocking();
    assert!(
        events.iter().any(|e| matches!(
            e,
            StreamEvent::Error {
                retryable: false,
                ..
            }
        )),
        "no more scripted responses queued"
    );
    assert_eq!(provider.call_count(), 2);
}

#[test]
fn exposes_models_like_a_provider_should() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text("x").usage(fake_usage(1, 1, 0, 0)))
        .build();
    let models = Provider::list_models(&provider);
    assert!(!models.is_empty());
    assert_eq!(provider.name(), "fake");
}
