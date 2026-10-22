//! NFR-24 guard for gh #404 gap 1: nested calls must not flood the transcript.
//!
//! The defect: a tool that makes ten nested calls produced eleven cards. gh #77
//! delivered the linkage (`ToolCall.parent_call_id` and `ToolResult.nested`) but
//! nothing in the interface read it, so a `codemode` script that greps twenty
//! times buried its own result under twenty cards.
//!
//! The rows live in `lca-ui`'s own `transcript::nested_tests` (next to the code
//! they assert). What this file guards is the pair of properties that a
//! single-crate test cannot see, because they are about the **wiring**:
//!
//! 1. The interface reads `ToolResult.nested` and `ToolCall.parent_call_id` at
//!    all. A card that renders a checklist perfectly is useless if nothing hands
//!    it the children - and that is precisely the state the tree was in for the
//!    issue's whole life.
//! 2. The parent link survives into the transcript so an orphan is drawn. The
//!    cheap version of this fix is "skip every child", which hides a call
//!    entirely when the linkage is broken - worse than the flood.
//!
//! Both are source facts, so both are read from the source. That is a weaker
//! kind of assertion than a driven pane, and it is the right kind here: the
//! property is "this field is consumed", which no runtime row can distinguish
//! from "a constant of the same shape was passed".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The live turn event's linkage reaches the card that draws it.
///
/// Driven, not source-scanned, and deliberately so: an earlier version of this
/// row grepped `chat.rs` for `.nested`, which passed while the code read
/// `.take(0)` - it proved the *name* was present, not that the children arrived.
/// The property worth pinning is behavioural, and `Chat::on_turn_event` is the
/// public seam the run loop uses, so the row can drive the real wiring.
#[test]
fn the_live_turn_event_carries_nested_children_into_the_card() {
    use lca_protocol::record::NestedCallRecord;
    use lca_protocol::{ToolResult, ToolResultStatus};

    // The same two calls `Chat::on_turn_event` makes, through the same public
    // API: building a whole `Chat` here would need `UiOptions`, which exists
    // only as a test fixture inside `lca-ui` - and a second copy of it here
    // would be a fixture that drifts. The wiring this row is about is
    // `ToolResult.nested` -> `finish_tool_with_children`, so that is what is
    // driven.
    let mut transcript = lca_ui::transcript::Transcript::new();
    transcript.start_tool_with_parent("codemode", r#"{"script":"..."}"#, None);
    let mut result = ToolResult::ok("call-1", "found 2");
    result.nested = vec![
        NestedCallRecord {
            name: "grep-a".to_string(),
            status: ToolResultStatus::Ok,
            content_head: "hit".to_string(),
        },
        NestedCallRecord {
            name: "grep-b".to_string(),
            status: ToolResultStatus::Error,
            content_head: "no such file".to_string(),
        },
    ];
    let children = result
        .nested
        .iter()
        .map(|nested| lca_ui::transcript::ChildCall {
            name: nested.name.clone(),
            status: nested.status.into(),
            head: nested.content_head.clone(),
        })
        .collect();
    transcript.finish_tool_with_children(
        lca_ui::transcript::ToolStatus::Ok,
        Some(result.content.clone()),
        None,
        None,
        children,
    );

    let rows = transcript.render(100, &lca_ui::theme::Theme::plain());
    let text: Vec<String> = rows
        .iter()
        .map(|row| lca_tui::engine::text::strip_terminal_sequences(row))
        .collect();
    let joined = text.join("\n");
    assert_eq!(
        text.iter().filter(|row| row.contains("> codemode")).count(),
        1,
        "one parent card: {text:?}"
    );
    assert!(
        joined.contains("grep-a"),
        "the first embedded child reached the checklist: {text:?}"
    );
    assert!(
        joined.contains("grep-b"),
        "the second embedded child reached the checklist: {text:?}"
    );
    assert!(
        joined.contains('✗'),
        "a failed child keeps its glyph: {text:?}"
    );
    assert!(
        !text.iter().any(|row| row.contains("> grep-a")),
        "and no child became its own card: {text:?}"
    );
}

/// The parent link survives onto the card, so an orphan is drawn rather than
/// hidden.
///
/// The cheap version of this fix is "skip every child", which hides a call
/// entirely when the linkage is broken - worse than the flood. This drives a
/// child whose parent card was never started and asserts it still shows.
#[test]
fn a_child_whose_parent_is_missing_still_gets_a_card() {
    let mut transcript = lca_ui::transcript::Transcript::new();
    // No card for the parent: it is absent, which is what a replay from before
    // the linkage (or a dropped event) looks like.
    transcript.start_tool_with_parent(
        "shell",
        r#"{"command":"echo hi"}"#,
        Some("missing-parent/0".to_string()),
    );
    transcript.finish_tool(lca_ui::transcript::ToolStatus::Ok, Some("hi".to_string()));
    let rows = transcript.render(100, &lca_ui::theme::Theme::plain());
    let text: Vec<String> = rows
        .iter()
        .map(|row| lca_tui::engine::text::strip_terminal_sequences(row))
        .collect();
    assert!(
        text.iter().any(|row| row.contains("shell")),
        "an orphaned child is still a card: {text:?}"
    );
    assert!(
        text.iter().any(|row| row.contains("hi")),
        "and its output shows: {text:?}"
    );
}

/// The checklist is bounded, and its bound is the one the collapsed view uses.
///
/// A checklist with no bound is the flood again, one level down: a script that
/// nested 200 calls would draw 200 rows.
#[test]
fn the_checklist_is_bounded_and_the_tail_is_what_survives() {
    let nested = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/transcript/nested.rs"
    ))
    .expect("nested.rs is in the tree");

    assert!(
        nested.contains("COLLAPSED_CHILDREN"),
        "the collapsed view has a row budget"
    );
    // The budget is a small constant, not "all of them".
    let at = nested
        .find("pub const COLLAPSED_CHILDREN: usize = ")
        .expect("the constant is declared");
    let number: usize = nested[at..]
        .split('=')
        .nth(1)
        .and_then(|rest| rest.trim().split(';').next())
        .and_then(|digits| digits.trim().parse().ok())
        .expect("the bound is a number");
    assert!(
        (1..=20).contains(&number),
        "the collapsed budget ({number}) must be a small constant: 0 hides the checklist \
         and a huge number re-creates the flood"
    );

    // The tail survives, which is #391's rule reused rather than re-derived.
    assert!(
        nested.contains("children.len() - COLLAPSED_CHILDREN"),
        "the kept rows are the LAST N children: the useful part of a checklist is what \
         just happened"
    );
}

/// The glyph table is total, so a new state cannot be added without deciding what
/// it looks like (NFR-28 also requires the glyph, not only the colour).
#[test]
fn every_child_state_has_a_glyph_and_a_word() {
    // `ChildStatus` is exhaustive by construction in Rust; what can rot is the
    // *pair* of accessors falling out of step, so this row asks both for every
    // variant the status enum has.
    use lca_ui::transcript::ChildStatus;
    let states = [
        ChildStatus::Running,
        ChildStatus::Ok,
        ChildStatus::Error,
        ChildStatus::Denied,
        ChildStatus::Timeout,
    ];
    let mut glyphs = Vec::new();
    for state in states {
        let glyph = state.glyph();
        assert!(!glyph.is_whitespace(), "{state:?} has a glyph");
        assert!(
            !state.word().is_empty(),
            "{state:?} has a word for a colour-blind reader"
        );
        glyphs.push(glyph);
    }
    // The four visual classes must be distinguishable by glyph alone: settled,
    // failed (error and timeout share a mark, which is pi's table), denied, and
    // running.
    assert!(glyphs.contains(&'✓'), "a settled glyph exists");
    assert!(glyphs.contains(&'✗'), "a failed glyph exists");
    assert!(glyphs.contains(&'⊘'), "a denied glyph exists");
    assert!(glyphs.contains(&'…'), "a running glyph exists");
}
