//! gh #464: the footer's `tok/s` reading, at the interface seam.
//!
//! Split out of `chat_tests.rs` for the workspace's 1,200-line ceiling (gate
//! 11). These rows drive `Chat::on_turn_event` with usage records that carry a
//! generation span - the shape `lca-core` now stamps where the stream is
//! consumed - and assert what the footer cell does with it. The pure arithmetic
//! lives next to the code in `footer::meter`'s own rows; what is pinned here is
//! the wiring: that the interface reads the stamp at all, that it never
//! substitutes its own clock, and that an impossible reading reaches the cell as
//! nothing rather than as a number.
//!
//! Verifies: FR-UI-20's `N tok/s` annotation (gh #464, gh #327).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::tests::chat;
use super::tests::strip;
use super::*;
use lca_protocol::{StopReason, ToolCall, ToolResult, TurnStatus};

// Verifies: the owner's "can we add tok/s to the footer?" ask, under gh
// #464's measurement rule - a finished turn reports output tokens over the
// generation span `lca-core` stamped where the stream was consumed.
//
// SANCTIONED CHANGE (gh #464, named in this cycle's brief): this row used to
// drive the old interface-side clock (an `Instant` window opened on the first
// delta and closed at a tool call) and assert `rate > 400` after a 150 ms
// sleep, because a UI-thread sleep was, on the old code, indistinguishable
// from generation time. The interface no longer owns a clock at all, so the
// span arrives on the usage record and the assertion is the arithmetic the
// footer performs: 40 tokens over 250 ms is 160 tok/s.
#[test]
fn a_finished_turn_reports_tokens_per_second() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta("chunk".into()));
    chat.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "c1".into(),
        name: "shell".into(),
        arguments: "{}".into(),
        parent_call_id: None,
    }));
    chat.on_turn_event(TurnEvent::ToolFinished(ToolResult::ok("c1", "out")));
    chat.on_turn_event(TurnEvent::Usage(stamped_usage(40, 250_000)));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    assert_eq!(
        chat.footer.tok_s,
        Some(160),
        "40 tokens over 250 ms of stamped generation time"
    );
    let text = strip(&chat.render(120)).join("\n");
    assert!(text.contains("160 tok/s"), "{text}");
}

// Verifies: gh #464 - the interface's own wakeups cannot shape the reading.
// On the old code this row was a 150 ms tool-run sleep between two deltas; the
// sleep no longer exists because it measured the interface thread, not the
// model. What is pinned instead is the property that made the sleep
// unnecessary: the reading is a pure function of the stamped span, so a
// stamp's value alone decides it.
#[test]
fn the_interface_clock_cannot_shape_the_reading() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta("chunk".into()));
    // A deliberately silly interval on this thread: 100 ms of pure sleep
    // between the delta and the usage record. It must not move the rate.
    std::thread::sleep(std::time::Duration::from_millis(100));
    chat.on_turn_event(TurnEvent::Usage(stamped_usage(40, 500_000)));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    assert_eq!(
        chat.footer.tok_s,
        Some(80),
        "40 tokens over the stamped 500 ms; the 100 ms of interface sleep is not in it"
    );
}

// Verifies: gh #464 and FR-UI-20's "no reading invented before the first
// measurable turn" - a turn that measured nothing keeps the previous reading
// rather than inventing one or clearing a good number.
//
// SANCTIONED CHANGE (gh #464): the first turn now carries a stamped span
// instead of relying on the interface clock, and the tools-only turn's usage
// carries none, which is exactly the "unmeasured" condition the rule is about.
#[test]
fn a_turn_without_streaming_keeps_the_previous_rate() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta("chunk".into()));
    chat.on_turn_event(TurnEvent::Usage(stamped_usage(40, 250_000)));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    let rate = chat.footer.tok_s.expect("the first turn reports a rate");
    assert_eq!(rate, 160);

    chat.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "c2".into(),
        name: "shell".into(),
        arguments: "{}".into(),
        parent_call_id: None,
    }));
    chat.on_turn_event(TurnEvent::ToolFinished(ToolResult::ok("c2", "out")));
    chat.on_turn_event(TurnEvent::Usage(Usage {
        output: 5,
        ..Default::default()
    }));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    assert_eq!(chat.footer.tok_s, Some(rate), "no stream, no new reading");
}

// Verifies: gh #464 - the receipt's arithmetic cannot reach the footer. 30
// tokens over a burst's 6 microseconds is millions; the cell shows nothing,
// because an impossible reading is a broken measurement, not data. This is the
// one row that would have caught the shipped defect at the interface.
#[test]
fn an_impossible_reading_shows_no_cell_at_all() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta(
        "Hello! How can I help you today?".into(),
    ));
    chat.on_turn_event(TurnEvent::Usage(stamped_usage(30, 6)));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    assert_eq!(chat.footer.tok_s, None, "millions is not a measurement");
    let text = strip(&chat.render(120)).join("\n");
    assert!(!text.contains("tok/s"), "no cell, not a wrong one: {text}");
}

/// A usage record with a generation span, as `lca-core` stamps it (gh #464).
fn stamped_usage(output: u64, micros: u64) -> Usage {
    let mut usage = Usage {
        output,
        ..Default::default()
    };
    usage.extras.insert(
        lca_protocol::GENERATION_US_EXTRA.to_string(),
        micros.to_string(),
    );
    usage
}
