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

// Verifies: gh #317 - `ScratchDir::new` must give every call its own
// directory. Before the fix the path was `<tmp>/lca-test-<name>-<pid>`, so
// two tests in one binary that happened to share a name computed the same
// path: the second call's `remove_dir_all` deleted the first one's live
// directory out from under it, and whichever test lost the race failed for
// a reason that had nothing to do with what it was testing.
#[test]
fn consecutive_scratch_dirs_with_one_name_are_distinct() {
    let first = lca_testkit::ScratchDir::new("gh317-same-name");
    let second = lca_testkit::ScratchDir::new("gh317-same-name");
    assert_ne!(
        first.path(),
        second.path(),
        "two calls with the same name must not share a path"
    );
    // Both must exist at the same time: the second call must not have
    // cleared the first.
    assert!(first.path().is_dir(), "the first directory still exists");
    assert!(second.path().is_dir(), "the second directory exists");
}

// Verifies: gh #317 - and the multi-threaded case the issue describes:
// threads in one test binary racing on the same name each get a live,
// distinct directory. This is the shape that actually broke under
// `cargo test`.
#[test]
fn parallel_threads_never_share_a_scratch_path() {
    let paths: std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let dirs: std::sync::Arc<std::sync::Mutex<Vec<lca_testkit::ScratchDir>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let paths = paths.clone();
            let dirs = dirs.clone();
            scope.spawn(move || {
                let dir = lca_testkit::ScratchDir::new("gh317-race");
                assert!(dir.path().is_dir(), "each thread's own directory exists");
                paths.lock().expect("paths").push(dir.path().to_path_buf());
                // Held alive until the scope ends, so a clobbering
                // implementation is caught rather than tidied away.
                dirs.lock().expect("dirs").push(dir);
            });
        }
    });
    let paths = paths.lock().expect("paths");
    let mut unique = paths.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        paths.len(),
        unique.len(),
        "every thread got its own directory: {paths:?}"
    );
}
