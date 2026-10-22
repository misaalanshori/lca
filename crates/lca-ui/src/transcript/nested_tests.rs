//! Nested-call checklists (gh #404 gap 1): a parent's children render *inside*
//! its card, not as sibling cards.
//!
//! Kept in their own module because the defect and its rules are their own: the
//! flood is not a formatting bug in one card, it is the transcript's decision
//! about which entries are top-level.
//!
//! **The defect.** A tool that makes ten nested calls produced eleven cards: the
//! parent, then ten full cards for the children. gh #77 delivered the linkage
//! (`ToolCall.parent_call_id` and `ToolResult.nested`), but nothing in the
//! interface read it, so a `codemode` script that greps twenty times buried its
//! own result under twenty cards.

use super::*;
use lca_tui::engine::text::strip_terminal_sequences;

fn plain() -> Theme {
    Theme::plain()
}

fn strip(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| strip_terminal_sequences(l)).collect()
}

/// One nested child as the transcript holds it.
fn child(name: &str, status: lca_protocol::ToolResultStatus, head: &str) -> ChildCall {
    ChildCall {
        name: name.to_string(),
        status: status.into(),
        head: head.to_string(),
    }
}

/// Render one parent card with `children`, collapsed, at a wide width.
fn render_parent(children: Vec<ChildCall>, expanded: bool) -> Vec<String> {
    let mut transcript = Transcript::new();
    transcript.start_tool("codemode", r#"{"script":"..."}"#);
    transcript.finish_tool_with_children(
        ToolStatus::Ok,
        Some("Script completed\nWall time: 1.2s\nfound 3 matches".to_string()),
        None,
        None,
        children,
    );
    if expanded {
        transcript.toggle_tools_expanded();
    }
    strip(&transcript.render(100, &plain()))
}

// Verifies: gh #404 gap 1 - the flood. A parent with five children renders ONE
// top-level card; the children are checklist rows inside it. Before the fix this
// asserted two cards (gh #77's nested path did not exist in the interface), and
// the count is the whole defect.
#[test]
fn a_parent_with_many_children_renders_one_card_with_a_checklist() {
    let children = (0..5)
        .map(|n| {
            child(
                &format!("grep-{n}"),
                lca_protocol::ToolResultStatus::Ok,
                "hit",
            )
        })
        .collect::<Vec<_>>();
    let out = render_parent(children, false);
    // One header for the parent, and none for any child.
    let headers = out.iter().filter(|row| row.contains("> codemode")).count();
    assert_eq!(headers, 1, "exactly one top-level card: {out:?}");
    for n in 0..5 {
        assert!(
            !out.iter().any(|row| row.contains(&format!("> grep-{n}"))),
            "grep-{n} must not be its own card: {out:?}"
        );
    }
    // And the children are listed inside the card, by name.
    for n in 0..5 {
        assert!(
            out.iter().any(|row| row.contains(&format!("grep-{n}"))),
            "grep-{n} is listed in the parent's checklist: {out:?}"
        );
    }
}

// Verifies: gh #404 gap 1 - collapsed shows the LAST n children with the earlier
// count above, because the useful part of a checklist is what just happened
// (#391's tail-vs-head rule, reused rather than re-derived).
#[test]
fn a_collapsed_checklist_keeps_the_tail_and_counts_the_rest_above() {
    let children = (0..12)
        .map(|n| {
            child(
                &format!("step{n:02}"),
                lca_protocol::ToolResultStatus::Ok,
                "ok",
            )
        })
        .collect::<Vec<_>>();
    let out = render_parent(children.clone(), false);
    let joined = out.join("\n");
    // The last rows are present, the first are not.
    for n in 4..12 {
        assert!(
            joined.contains(&format!("step{n:02}")),
            "the tail keeps step{n:02}: {out:?}"
        );
    }
    for n in 0..4 {
        assert!(
            !joined.contains(&format!("step{n:02}")),
            "the tail drops step{n:02}: {out:?}"
        );
    }
    // The omission marker names the count, above the list.
    let marker = out
        .iter()
        .position(|row| row.contains("earlier"))
        .expect("an omission marker");
    let first_shown = out
        .iter()
        .position(|row| row.contains("step04"))
        .expect("the first kept row");
    assert!(
        marker < first_shown,
        "the marker sits above the tail it describes: {out:?}"
    );
    assert!(out[marker].contains('4'), "it names 4 earlier: {out:?}");
}

// Verifies: gh #404 gap 1 - expanding shows every child, and no marker.
#[test]
fn an_expanded_checklist_shows_every_child() {
    let children = (0..12)
        .map(|n| {
            child(
                &format!("step{n:02}"),
                lca_protocol::ToolResultStatus::Ok,
                "ok",
            )
        })
        .collect::<Vec<_>>();
    let out = render_parent(children, true);
    let joined = out.join("\n");
    for n in 0..12 {
        assert!(
            joined.contains(&format!("step{n:02}")),
            "step{n:02} shows: {out:?}"
        );
    }
    assert!(
        !joined.contains("earlier"),
        "nothing is omitted when expanded: {out:?}"
    );
}

// Verifies: gh #404 gap 1 - each state has its own glyph, and the glyph (not the
// colour) is the primary cue (NFR-28). A mixed set is the row that matters: one
// tick per state would pass with a single-glyph implementation.
#[test]
fn the_checklist_glyphs_distinguish_every_state() {
    use lca_protocol::ToolResultStatus as S;
    let children = vec![
        child("settled", S::Ok, "done"),
        child("failed", S::Error, "boom"),
        child("denied", S::Denied, "not allowed"),
        child("running", S::Timeout, "gave up"),
    ];
    let out = render_parent(children, false);
    let joined = out.join("\n");
    for glyph in ['✓', '✗', '⊘'] {
        assert!(
            joined.contains(glyph),
            "the {glyph} glyph appears for its state: {out:?}"
        );
    }
    // The four states must not collapse onto one glyph: each named child row
    // carries a glyph from a different class.
    let row_for = |name: &str| {
        out.iter()
            .find(|row| row.contains(name))
            .unwrap_or_else(|| panic!("{name} is listed: {out:?}"))
            .clone()
    };
    assert_ne!(row_for("settled"), row_for("failed"));
    assert_ne!(row_for("failed"), row_for("denied"));
}

// Verifies: gh #404 gap 1 - a failed child's error is visible when expanded,
// indented, and without its whole output. The checklist is for orientation; a
// child's full output belongs in its own view.
#[test]
fn an_expanded_checklist_shows_a_failed_childs_error_indented() {
    use lca_protocol::ToolResultStatus as S;
    let long_head = format!("KeyError: 'TOKEN'{}", " trailing detail".repeat(40));
    let children = vec![
        child("settled", S::Ok, "fine"),
        child("failed", S::Error, &long_head),
    ];
    let out = render_parent(children, true);
    let joined = out.join("\n");
    let error_row = out
        .iter()
        .position(|row| row.contains("KeyError"))
        .unwrap_or_else(|| panic!("the child's error shows: {out:?}"));
    // Indented under the child that produced it.
    assert!(
        out[error_row].starts_with("     "),
        "the error is indented under its child: {:?}",
        out[error_row]
    );
    // And not the whole output: it is bounded to a couple of rows plus a count,
    // so a 600-character head cannot spend the card's height.
    let error_rows = out
        .iter()
        .filter(|row| row.trim_start().starts_with("KeyError") || row.contains("more lines"))
        .count();
    assert!(
        error_rows <= 3,
        "the error is bounded to a few rows, got {error_rows}: {out:?}"
    );
    assert!(
        joined.contains("more lines"),
        "and it says how much was cut: {out:?}"
    );
}

// Verifies: gh #404 gap 1 - the orphan fallback. A child whose parent is absent
// still renders as a top-level card. The failure mode of "skip every child" is a
// broken linkage silently hiding a call entirely, which is worse than the flood.
#[test]
fn a_child_with_no_parent_still_renders_as_a_top_level_card() {
    let mut transcript = Transcript::new();
    // The parent's card was never started (a replay from before the linkage
    // existed, or a dropped event): the child must not vanish.
    transcript.start_tool_with_parent(
        "shell",
        r#"{"command":"echo hi"}"#,
        Some("missing-parent/1".to_string()),
    );
    transcript.finish_tool(ToolStatus::Ok, Some("its output".to_string()));
    let out = strip(&transcript.render(100, &plain()));
    assert!(
        out.iter().any(|row| row.contains("> shell")),
        "an orphaned child still shows: {out:?}"
    );
    assert!(
        out.iter().any(|row| row.contains("its output")),
        "and its output shows: {out:?}"
    );
}

// Verifies: gh #404 gap 1 - nesting depth. A child that itself nests renders
// without recursing the layout: the checklist is one level deep by design (the
// grandchild is summarised under its own parent, which is itself a row).
#[test]
fn a_deeply_nested_call_does_not_recurse_the_checklist() {
    use lca_protocol::ToolResultStatus as S;
    // The turn's own bound is eight levels (gh #77); the card shows one level,
    // so a child whose name carries a slash (a grandchild's id shape) renders as
    // a row without spawning its own sub-checklist.
    let children = vec![
        child("child-1/1", S::Ok, "grandchild ran"),
        child("child-2", S::Ok, "fine"),
    ];
    let out = render_parent(children, true);
    // One card, and both rows present at the same indent: no second level.
    assert_eq!(
        out.iter().filter(|row| row.contains("> codemode")).count(),
        1,
        "one card regardless of depth: {out:?}"
    );
    let depths: Vec<usize> = out
        .iter()
        .filter(|row| row.contains("child-1/1") || row.contains("child-2"))
        .map(|row| row.len() - row.trim_start().len())
        .collect();
    assert_eq!(depths.len(), 2, "both children are listed: {out:?}");
    assert_eq!(
        depths[0], depths[1],
        "a grandchild's row sits at the same depth as its sibling: {out:?}"
    );
}

// Verifies: gh #404 gap 1 - the wrapper-header strip. pi removes `Script
// completed` / `Wall time:` / `Output:` from a codemode parent's output; LCA's
// rule is narrower and safer: strip only boilerplate this tree provably
// produced, never author output.
#[test]
fn a_codemode_parent_strips_only_its_own_wrapper_boilerplate() {
    let mut transcript = Transcript::new();
    transcript.start_tool("codemode", r#"{"script":"..."}"#);
    transcript.finish_tool_with_children(
        ToolStatus::Ok,
        Some(
            "Script completed\nWall time: 1.2s\nOutput:\nfound 3 matches\nWall time: not a number"
                .to_string(),
        ),
        None,
        None,
        Vec::new(),
    );
    let out = strip(&transcript.render(100, &plain()));
    let joined = out.join("\n");
    assert!(
        !joined.contains("Script completed"),
        "the wrapper's own banner is dropped: {out:?}"
    );
    assert!(
        !joined.contains("Wall time: 1.2s"),
        "the wrapper's own timing line is dropped: {out:?}"
    );
    // What the script itself printed survives, including a line that happens to
    // start with a wrapper word but is not the wrapper's shape.
    assert!(
        joined.contains("found 3 matches"),
        "the real output: {out:?}"
    );
    assert!(
        joined.contains("Wall time: not a number"),
        "author output that merely mentions the phrase survives: {out:?}"
    );
}

// Verifies: gh #404 gap 1 / design point 5 - no invented micro-costs.
// `ToolResult` carries no usage, so there is nothing to show; a `$0.00000` would
// be a worse lie than silence.
#[test]
fn the_checklist_shows_no_cost_because_the_record_carries_none() {
    use lca_protocol::ToolResultStatus as S;
    let children = vec![child("a", S::Ok, "x"), child("b", S::Ok, "y")];
    let out = render_parent(children, false);
    let joined = out.join("\n");
    assert!(
        !joined.contains('$'),
        "no cost is shown because none was recorded: {out:?}"
    );
}

// Verifies: gh #404 gap 1 / design point 6 - a running parent keeps its live
// elapsed, exactly as a single card does (#391's rule, not a second one).
#[test]
fn a_running_parent_with_children_shows_a_live_elapsed() {
    use lca_protocol::ToolResultStatus as S;
    let mut transcript = Transcript::new();
    transcript.start_tool("codemode", "{}");
    transcript.push_children(vec![child("still-going", S::Ok, "x")]);
    let out = strip(&transcript.render(100, &plain()));
    assert!(
        out.iter().any(|row| row.contains("Elapsed")),
        "a running parent times itself: {out:?}"
    );
}

// Verifies: gh #404 gap 1 on the **replay** path. A resumed session reads the
// same `ToolResult.nested` the live turn wrote, so the checklist must appear
// there too - a fix that only worked live would show a flood the moment the user
// ran `/resume`.
#[test]
fn a_replayed_parent_renders_the_same_checklist() {
    use lca_protocol::Record;
    use lca_protocol::record::NestedCallRecord;
    let records = vec![
        Record::ToolCall {
            v: 1,
            ts: 0,
            id: "a-call0".to_string(),
            parent: None,
            call_id: "call-1".to_string(),
            name: "codemode".to_string(),
            arguments: r#"{"script":"..."}"#.to_string(),
            source: lca_protocol::ToolSource::Extension,
        },
        Record::ToolResult {
            v: 1,
            ts: 0,
            id: "a-result0".to_string(),
            parent: None,
            call_id: "call-1".to_string(),
            status: lca_protocol::ToolResultStatus::Ok,
            content: Some("found 2".to_string()),
            attachment: None,
            truncated: false,
            duration_ms: Some(1200),
            exit_code: None,
            full_output_path: None,
            nested: vec![
                NestedCallRecord {
                    name: "grep-a".to_string(),
                    status: lca_protocol::ToolResultStatus::Ok,
                    content_head: "hit".to_string(),
                },
                NestedCallRecord {
                    name: "grep-b".to_string(),
                    status: lca_protocol::ToolResultStatus::Error,
                    content_head: "no such file".to_string(),
                },
            ],
            images: Vec::new(),
        },
    ];
    let mut transcript = Transcript::new();
    transcript.replay_records(&records, None);
    let out = strip(&transcript.render(100, &plain()));
    assert_eq!(
        out.iter().filter(|row| row.contains("> codemode")).count(),
        1,
        "one card on replay too: {out:?}"
    );
    let joined = out.join("\n");
    assert!(joined.contains("grep-a"), "{out:?}");
    assert!(joined.contains("grep-b"), "{out:?}");
    assert!(joined.contains('✓') && joined.contains('✗'), "{out:?}");
}

// Verifies: gh #404 gap 1 - the **live event pair**. The turn emits a
// `ToolStarted` *and* a `ToolFinished` for a nested call (so a headless observer
// sees the pair), which means the child reaches the transcript as ordinary
// events. Found in a live pane: with only the card-side checklist wired, the
// child still got its own top-level card, because nothing suppressed its start.
//
// This is the row that would have caught that, and it asserts the whole pair:
// the start becomes a checklist row, and the finish settles *that row* rather
// than creating or amending a card.
#[test]
fn a_nested_start_and_finish_pair_becomes_one_checklist_row() {
    let mut transcript = Transcript::new();
    // The parent starts, as the model asked for it.
    transcript.start_tool_with_id("codemode", "{}", None, Some("call-1".to_string()));
    // A nested child's *own* events - this is what the turn really emits.
    transcript.start_tool_with_id(
        "grep",
        r#"{"pattern":"x"}"#,
        Some("call-1".to_string()),
        Some("call-1/1".to_string()),
    );
    transcript.finish_tool_with_id(
        ToolStatus::Ok,
        Some("hit".to_string()),
        None,
        Some(7),
        Vec::new(),
        Some("call-1/1".to_string()),
    );
    // The parent settles, carrying the persisted record too. It must not
    // duplicate the row the live pair already added.
    transcript.finish_tool_with_children(
        ToolStatus::Ok,
        Some("done".to_string()),
        None,
        Some(900),
        Vec::new(),
    );
    let out = strip(&transcript.render(100, &plain()));
    assert_eq!(
        out.iter().filter(|row| row.contains("> codemode")).count(),
        1,
        "one parent card: {out:?}"
    );
    assert!(
        !out.iter().any(|row| row.contains("> grep")),
        "the child never became a card: {out:?}"
    );
    let child_rows = out.iter().filter(|row| row.contains("grep")).count();
    assert_eq!(child_rows, 1, "the child is one checklist row: {out:?}");
    // And its live finish settled it: the row is a tick, not a spinner.
    let child_row = out
        .iter()
        .find(|row| row.contains("grep"))
        .expect("the row exists");
    assert!(
        child_row.contains('✓'),
        "the finish settled the row: {child_row:?}"
    );
}

// Verifies: gh #404 gap 1 - a **failed** live child settles to its failure glyph
// and keeps its head, so the expanded card can show the error.
#[test]
fn a_failed_live_child_settles_to_its_failure_state() {
    let mut transcript = Transcript::new();
    transcript.start_tool_with_id("codemode", "{}", None, Some("call-1".to_string()));
    transcript.start_tool_with_id(
        "grep",
        "{}",
        Some("call-1".to_string()),
        Some("call-1/1".to_string()),
    );
    transcript.finish_tool_with_id(
        ToolStatus::Error,
        Some("KeyError: 'TOKEN'".to_string()),
        None,
        None,
        Vec::new(),
        Some("call-1/1".to_string()),
    );
    transcript.toggle_tools_expanded();
    let out = strip(&transcript.render(100, &plain()));
    let joined = out.join("\n");
    assert!(
        out.iter().any(|row| row.contains('✗')),
        "the failed child shows its failure glyph: {out:?}"
    );
    assert!(
        joined.contains("KeyError"),
        "and its error is visible when expanded: {out:?}"
    );
}

// Verifies: gh #404 gap 1 - the **two-path dedup**. Found in a live pane: the
// turn emits a start/finish pair for each nested call *and* files a bounded
// record under the parent's result, so the same child reached the card twice and
// drew two identical rows. The live pair is richer (it carries the child's own
// output), so it wins; the persisted set only fills a card that has none.
#[test]
fn a_child_arriving_by_both_paths_is_one_row() {
    let mut transcript = Transcript::new();
    transcript.start_tool_with_id("codemode", "{}", None, Some("call-1".to_string()));
    // Path 1: the live pair.
    transcript.start_tool_with_id(
        "grep",
        "{}",
        Some("call-1".to_string()),
        Some("call-1/1".to_string()),
    );
    transcript.finish_tool_with_id(
        ToolStatus::Ok,
        Some("hit".to_string()),
        None,
        None,
        Vec::new(),
        Some("call-1/1".to_string()),
    );
    // Path 2: the parent's persisted record for the same child.
    transcript.finish_tool_with_children(
        ToolStatus::Ok,
        Some("done".to_string()),
        None,
        None,
        vec![ChildCall {
            name: "grep".to_string(),
            status: ChildStatus::Ok,
            head: "hit".to_string(),
        }],
    );
    let out = strip(&transcript.render(100, &plain()));
    assert_eq!(
        out.iter().filter(|row| row.contains("grep")).count(),
        1,
        "the child is one row, not two: {out:?}"
    );
}

// Verifies: gh #404 gap 1 - the persisted set still fills a card that got no live
// events, which is the replay path and the case where a sink dropped them.
#[test]
fn the_persisted_children_fill_a_card_with_no_live_rows() {
    let mut transcript = Transcript::new();
    transcript.start_tool_with_id("codemode", "{}", None, Some("call-1".to_string()));
    transcript.finish_tool_with_children(
        ToolStatus::Ok,
        Some("done".to_string()),
        None,
        None,
        vec![
            ChildCall {
                name: "from-record-a".to_string(),
                status: ChildStatus::Ok,
                head: "x".to_string(),
            },
            ChildCall {
                name: "from-record-b".to_string(),
                status: ChildStatus::Error,
                head: "y".to_string(),
            },
        ],
    );
    let out = strip(&transcript.render(100, &plain()));
    assert!(
        out.iter().any(|row| row.contains("from-record-a"))
            && out.iter().any(|row| row.contains("from-record-b")),
        "both persisted children show: {out:?}"
    );
}
