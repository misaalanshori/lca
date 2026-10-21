//! `HeadlessSink`'s own test rows: the JSONL shape per event kind, the
//! plain-text path, and the drift guard that reads `docs/headless.md`'s event
//! table and fails when a documented kind has no producer.
//!
//! Split from `headless.rs` for the workspace's 1,200-line ceiling (gate 11)
//! when gh #264 added the `extension-event` rows; behaviour unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use crate::headless::*;
use lca_protocol::TurnEvent;

/// A sink over a buffer, so an assertion can read the emitted JSONL.
///
/// Reuses the buffer writer `sink_tests` already carries rather than
/// duplicating it.
fn buffered(json: bool) -> (HeadlessSink, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
    let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink =
        HeadlessSink::with_writer(json, false, Box::new(sink_tests::VecWriter(buffer.clone())));
    (sink, buffer)
}

/// The lines a sink emitted, parsed.
fn lines(buffer: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> Vec<serde_json::Value> {
    let bytes = buffer
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    String::from_utf8(bytes)
        .expect("utf-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect()
}

/// An extension lifecycle event, the shape `lca-core` and `lca-cli` emit.
fn extension_event() -> TurnEvent {
    TurnEvent::ExtensionEvent {
        extension: "conformance".to_string(),
        event: "disabled".to_string(),
        detail: "tapd at call 3".to_string(),
    }
}

// A tool-using turn emits one assistant message before the tool and one
// after; plain headless glued them into one run-on line.
#[test]
fn plain_headless_separates_multiple_assistant_messages() {
    let mut sink = HeadlessSink::new(false, false);
    sink.on_event(TurnEvent::AssistantText("I'll read the file.".into()));
    sink.on_event(TurnEvent::AssistantText("The answer is 42.".into()));
    assert_eq!(sink.plain, "I'll read the file.\n\nThe answer is 42.");
}

// Verifies: gh #264 acceptance 1/2 - `--mode json` emits the documented
// `extension-event` row, with exactly the fields `docs/headless.md`'s
// table names (`extension`, `event`, `detail`).
#[test]
fn json_mode_emits_a_documented_extension_event() {
    let (mut sink, buffer) = buffered(true);
    sink.on_event(extension_event());
    let rows = lines(&buffer);
    assert_eq!(rows.len(), 1, "one event, one row: {rows:?}");
    assert_eq!(
        rows[0],
        serde_json::json!({
            "type": "extension-event",
            "extension": "conformance",
            "event": "disabled",
            "detail": "tapd at call 3",
        }),
        "the documented shape, field for field"
    );
}

// Verifies: gh #264 - the text path stays silent. An extension event is a
// JSON-mode record; printing it would put host diagnostics in the middle
// of the answer a script reads from stdout.
#[test]
fn plain_mode_emits_no_extension_event() {
    let (mut sink, buffer) = buffered(false);
    sink.on_event(extension_event());
    assert!(
        buffer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_empty(),
        "text mode writes the event nowhere"
    );
}

// Verifies: gh #264 - arrival order is preserved and nothing coalesces:
// two events are two rows, in the order the engine emitted them.
#[test]
fn extension_events_keep_arrival_order_and_do_not_coalesce() {
    let (mut sink, buffer) = buffered(true);
    for (extension, event) in [("a", "load"), ("b", "disabled")] {
        sink.on_event(TurnEvent::ExtensionEvent {
            extension: extension.to_string(),
            event: event.to_string(),
            detail: String::new(),
        });
    }
    let rows = lines(&buffer);
    let named: Vec<(&str, &str)> = rows
        .iter()
        .map(|row| {
            (
                row["extension"].as_str().expect("extension"),
                row["event"].as_str().expect("event"),
            )
        })
        .collect();
    assert_eq!(named, vec![("a", "load"), ("b", "disabled")]);
}

// Verifies: gh #264 - the vocabulary-drift guard. Every `type` value
// `docs/headless.md`'s event table documents has a producer in this file,
// so a future refactor of `on_event` cannot silently drop a documented
// kind the way #264's `_ => {}` dropped `extension-event`.
//
// The table is read from the doc itself rather than restated here: a row
// added to the docs and forgotten in the sink fails this test, which is
// the drift the guard exists for.
#[test]
fn every_documented_event_type_has_a_producer() {
    // Kinds documented but deliberately produced outside `on_event`:
    // `session-start` opens the stream before any turn. `queue-queued` used
    // to be listed here as a known gap (gh #264 recorded it, gh #426 fixed
    // it): the accept is now emitted by `HeadlessSink::emit_queue_queued`,
    // the same sink that emits the `queue-flushed` drain. An entry in this
    // list is a decision, an omission is a bug - so the list is now the one
    // kind that genuinely has no turn event behind it.
    const PRODUCED_ELSEWHERE: &[&str] = &["session-start"];

    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/headless.md"),
    )
    .expect("docs/headless.md is readable");
    let source = include_str!("headless.rs");

    // The event table is one section: `## The event stream` up to the
    // `### Pi-shape mapping` sub-table, whose left column is pi's names.
    // Scoping to it is what keeps this row about `type` values.
    let section = doc
        .split("## The event stream")
        .nth(1)
        .expect("the event-stream section exists")
        .split("### Pi-shape mapping")
        .next()
        .expect("the section ends at the mapping table");

    let mut missing = Vec::new();
    let mut documented = 0;
    for line in section.lines() {
        let Some(rest) = line.strip_prefix("| `") else {
            continue;
        };
        let Some((kind, _)) = rest.split_once('`') else {
            continue;
        };
        // The header row is `| \`type\` | Fields | Meaning |`.
        if kind == "type" {
            continue;
        }
        documented += 1;
        if PRODUCED_ELSEWHERE.contains(&kind) {
            continue;
        }
        if !source.contains(&format!("\"type\": \"{kind}\"")) {
            missing.push(kind.to_string());
        }
    }
    assert!(
        documented >= 18,
        "the doc table was parsed ({documented} rows); a parse break must not silently pass"
    );
    assert!(
        missing.is_empty(),
        "documented event types with no producer in `on_event`: {missing:?}"
    );
}
