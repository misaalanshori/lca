//! Gh #390 gap 1: a truncated turn says so, once, and says what to do.
//!
//! Split from `chat_tests.rs` (gate 11's 1,200-line ceiling): one subject, one file.

use super::tests::{chat, strip};
use lca_protocol::{StopReason, ToolCall, TurnEvent, TurnStatus};
// ===========================================================================
// Gh #390 gap 1: a truncated turn says so, once, and says what to do.
// ===========================================================================

/// The banner's own words, from the module that owns them.
fn truncation_banner() -> &'static str {
    crate::chat_turn_events::TRUNCATED_NOTICE
}

/// Rendered rows joined with their whitespace collapsed, so a wrapped notice still
/// matches as one string. The banner is ~150 columns of prose in a 60-column pane,
/// so it is *always* wrapped - the first version of these rows compared against the
/// raw rows and failed on a correct render.
fn flat(rows: &[String]) -> String {
    rows.iter()
        .map(|row| strip(std::slice::from_ref(row)).join(" "))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The banner, whitespace-collapsed the same way, for comparison against [`flat`].
fn truncation_banner_flat() -> String {
    truncation_banner()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// Verifies: gh #390 gap 1 (**the row this issue is about**) - **a `length` stop
// with a tool call in flight shows the banner.** The shape this replaces made the
// banner's presence depend on the *absence* of tool calls, so the case where a call
// was cut mid-stream - the one where the previewed work may never have run - was the
// one that said nothing at all.
#[test]
fn a_truncation_with_a_tool_call_in_flight_shows_the_banner() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta("Let me write that.".into()));
    chat.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "call-1".into(),
        name: "write".into(),
        arguments: "{\"path\": \"a.txt\"".into(),
        parent_call_id: None,
    }));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Length,
    });
    let text = flat(&chat.render(60));
    assert!(
        text.contains(&truncation_banner_flat()),
        "the banner is shown with a tool call present:\n{text}"
    );
    // ...and the tool card is still there: the banner reports, it does not hide.
    assert!(text.contains("write"), "the call's card survives:\n{text}");
}

// Verifies: gh #390 gap 1 - **the control**: a truncation without tool calls shows
// it too. The two rows together are what make the banner unconditional; either
// alone could be satisfied by a conditional that happened to be true once.
#[test]
fn a_truncation_without_tool_calls_shows_the_banner() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta("half an ans".into()));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Length,
    });
    let text = flat(&chat.render(60));
    assert!(text.contains(&truncation_banner_flat()), "{text}");
}

// Verifies: gh #390 gap 1 - **exactly once.** The banner is appended in the one
// place a turn ends (`TurnEnded`); a second site - the error envelope, the status
// line, a per-delta check - would double it, and a duplicated notice is the
// double-report this issue warns about.
#[test]
fn the_truncation_banner_appears_exactly_once() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta("partial".into()));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Length,
    });
    let text = flat(&chat.render(60));
    assert_eq!(
        text.matches(&truncation_banner_flat()).count(),
        1,
        "one banner, not a repeat per render or per event:\n{text}"
    );
    // A second render must not append another: the renderer is pure.
    let again = flat(&chat.render(60));
    assert_eq!(
        again.matches(&truncation_banner_flat()).count(),
        1,
        "{again}"
    );
}

// Verifies: gh #390 gap 1 - **the message names the remedy, not only the fact.**
// pi prints "Response was truncated before completion." and stops, which leaves a
// user with no next step; AGENTS.md's error-copy rule requires the actionable half.
#[test]
fn the_truncation_banner_names_a_remedy() {
    let notice = truncation_banner();
    assert!(
        notice.contains("max_tokens"),
        "it names the setting that gives the model room: {notice}"
    );
    assert!(
        notice.contains("/compact"),
        "and the command that leaves room for the rest: {notice}"
    );
    assert!(
        notice.contains("truncated"),
        "and what happened, in the user's words: {notice}"
    );
    // The jargon rule: it must not name the wire field a user cannot act on.
    assert!(
        !notice.contains("finish_reason") && !notice.contains("stop_reason"),
        "no wire vocabulary in user-facing copy: {notice}"
    );
}

// Verifies: gh #390 gap 1 - **a normal stop shows no banner.** The control for the
// whole feature: if `Length` were not distinguished, every turn would warn.
#[test]
fn a_normal_stop_shows_no_truncation_banner() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TextDelta("the whole answer".into()));
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Stop,
    });
    let text = flat(&chat.render(60));
    assert!(!text.contains(&truncation_banner_flat()), "{text}");
}

// Verifies: gh #390 gap 1 - **the status line says it too.** A user who has
// scrolled away from the banner still sees the turn's outcome named, and "done" for
// a cut answer would be the same silent-truncation defect one layer up.
#[test]
fn the_status_line_names_a_truncated_turn() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::TurnEnded {
        status: TurnStatus::Ok,
        stop_reason: StopReason::Length,
    });
    let line = chat.turn_status.as_ref().map(|line| line.text.clone());
    assert_eq!(
        line.as_deref(),
        Some("done: truncated"),
        "the status line does not say a plain `done`"
    );
}
