//! gh #391 gaps 2-4: the live `Elapsed` timer, `!` vs `!!`, and the
//! truncation footer shown once.
//!
//! Split from `tests.rs` for the 1,200-line ceiling.

use super::render::format_duration_ms;
use super::*;
use lca_tui::engine::text::strip_terminal_sequences;

fn plain() -> Theme {
    Theme::plain()
}

fn strip(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| strip_terminal_sequences(l)).collect()
}

// --- gap 2: the timer's tiers ------------------------------------------

// Verifies: gh #391 gap 2 - pi's `formatToolDuration` tiers, verbatim from
// `render-utils.ts:100-110`: `4.2s` under a minute, `1m 30s` under an hour,
// `2h 3m 4s` above.
#[test]
fn durations_render_in_pis_three_tiers() {
    assert_eq!(format_duration_ms(0), "0.0s");
    assert_eq!(format_duration_ms(45), "0.0s");
    assert_eq!(format_duration_ms(4_200), "4.2s");
    assert_eq!(format_duration_ms(59_900), "59.9s");
    assert_eq!(format_duration_ms(60_000), "1m 0s");
    assert_eq!(format_duration_ms(90_000), "1m 30s");
    assert_eq!(format_duration_ms(3_599_000), "59m 59s");
    assert_eq!(format_duration_ms(3_600_000), "1h 0m 0s");
    assert_eq!(
        format_duration_ms(7_384_000),
        "2h 3m 4s",
        "hours, minutes, seconds"
    );
}

// Verifies: gh #391 gap 2 - a **running** card shows `Elapsed`, and the text
// advances as the clock does. Driven off the card's own `started_at` rather
// than a second timer, so the row can move the clock instead of sleeping.
#[test]
fn a_running_card_shows_a_live_elapsed_line() {
    let mut t = Transcript::new();
    t.start_tool("shell", r#"{"command":"cargo test"}"#);
    let first = strip(&t.render(60, &plain())).join("\n");
    assert!(first.contains("Elapsed 0.0s"), "{first}");
    assert!(
        !first.contains("Took"),
        "a running card has no recorded duration yet: {first}"
    );

    // The clock moves. A real run waits; the row rewinds `started_at` so the
    // assertion is about the derivation, not about sleeping.
    t.set_started_at_for_test(
        0,
        std::time::Instant::now() - std::time::Duration::from_millis(4_200),
    );
    t.invalidate_running_tools();
    let later = strip(&t.render(60, &plain())).join("\n");
    assert!(
        later.contains("Elapsed 4.2s"),
        "the elapsed line follows started_at: {later}"
    );
}

// Verifies: gh #391 gap 2 - a settled card freezes to the recorded duration
// and never animates. This is the half a naive "always show now" gets wrong.
#[test]
fn a_settled_card_freezes_to_its_recorded_duration() {
    let mut t = Transcript::new();
    t.start_tool("shell", r#"{"command":"cargo test"}"#);
    t.set_started_at_for_test(
        0,
        std::time::Instant::now() - std::time::Duration::from_secs(5),
    );
    t.finish_tool_full(ToolStatus::Ok, Some("ok".into()), None, Some(4_200));

    let first = strip(&t.render(60, &plain())).join("\n");
    assert!(first.contains("Took 4.2s"), "{first}");
    assert!(
        !first.contains("Elapsed"),
        "a settled card does not tick: {first}"
    );

    // Even with the old `started_at` still very much in the past, and after
    // the tick's invalidation, the line does not move.
    t.invalidate_running_tools();
    let later = strip(&t.render(60, &plain())).join("\n");
    assert_eq!(
        later, first,
        "a completed card must not animate its duration"
    );
}

// Verifies: gh #391 gap 2 - a **replayed/historical** card shows its exact
// recorded duration and does not animate, because a replay carries no
// `started_at` at all. Without this, a resumed session would show `Elapsed`
// counting from now on a tool that finished yesterday.
#[test]
fn a_historical_card_shows_its_exact_duration_and_never_animates() {
    let mut t = Transcript::new();
    // A replay: the record's own duration, and no live clock.
    t.push_historical_tool_for_test("shell", "cargo test", "ok", 4_200);
    let first = strip(&t.render(60, &plain())).join("\n");
    assert!(first.contains("Took 4.2s"), "{first}");
    assert!(
        !first.contains("Elapsed"),
        "a historical card has no clock to show: {first}"
    );
    let later = strip(&t.render(60, &plain())).join("\n");
    assert_eq!(later, first, "and it never animates");
}

// Verifies: gh #391 gap 2 - an idle transcript reports no running tool, so
// the interface tick does not repaint once a second forever.
#[test]
fn only_a_running_card_asks_the_tick_to_repaint() {
    let mut t = Transcript::new();
    assert!(!t.has_running_tool(), "an empty transcript is idle");
    t.start_tool("shell", "true");
    assert!(t.has_running_tool(), "a live card wants the tick");
    t.finish_tool_full(ToolStatus::Ok, Some("".into()), None, Some(1));
    assert!(
        !t.has_running_tool(),
        "and stops wanting it when it settles"
    );
}

// --- gap 3: `!` vs `!!` ------------------------------------------------

// Verifies: gh #391 gap 3 / NFR-28 - `!` and `!!` differ in **style** (green
// `bashMode` vs quiet dim) and carry a **textual** distinction (the `!`/`!!`
// marker), so a terminal with no colour can still tell them apart.
#[test]
fn the_two_manual_modes_differ_in_colour_and_in_text() {
    let theme = Theme::colored();

    let mut context_included = Transcript::new();
    context_included.start_manual_tool_excluding("bash", r#"{"command":"ls"}"#, false);
    let included: String = context_included.render(60, &theme).concat();

    let mut eyes_only = Transcript::new();
    eyes_only.start_manual_tool_excluding("bash", r#"{"command":"ls"}"#, true);
    let excluded: String = eyes_only.render(60, &theme).concat();

    // Style: bashMode green (#b5bd68) vs the dim SGR.
    assert!(
        included.contains("38;2;181;189;104"),
        "`!` keeps pi's bashMode: {included:?}"
    );
    assert!(
        !excluded.contains("38;2;181;189;104"),
        "`!!` is not bashMode: {excluded:?}"
    );
    assert!(
        excluded.contains("\x1b[2m") || excluded.contains("\x1b[2;"),
        "`!!` draws dim: {excluded:?}"
    );

    // Text: the marker travels in the title, which is what survives no colour.
    let plain_included = strip(&context_included.render(60, &plain())).join("\n");
    let plain_excluded = strip(&eyes_only.render(60, &plain())).join("\n");
    assert!(
        plain_included.contains("!bash ls"),
        "the included form wears one bang: {plain_included}"
    );
    assert!(
        plain_excluded.contains("!!bash ls"),
        "the eyes-only form wears two: {plain_excluded}"
    );
    assert_ne!(
        plain_included, plain_excluded,
        "the plain theme still distinguishes the two modes (NFR-28)"
    );
}

// Verifies: gh #391 gap 3 - a **model-requested** shell call is unaffected:
// it keeps `toolTitle` and carries no `!` marker, so the editor's mode cannot
// be confused with the agent's own call.
#[test]
fn a_model_shell_call_carries_no_marker() {
    let mut t = Transcript::new();
    t.start_tool("shell", r#"{"command":"ls"}"#);
    let text = strip(&t.render(60, &plain())).join("\n");
    assert!(text.contains("shell ls"), "{text}");
    assert!(!text.contains("!shell"), "{text}");
}

// --- gap 4: the footer appears once ------------------------------------

// Verifies: gh #391 gap 4 - the LLM-facing spillover footer never reaches
// the preview, so it cannot eat two of five tail rows or repeat what the
// card's own warning says (the gh #328 family).
#[test]
fn the_truncation_footer_does_not_consume_a_tail_row() {
    let mut t = Transcript::new();
    t.start_tool("shell", r#"{"command":"build"}"#);
    let output = "line 1\nline 2\nline 3\nline 4\nline 5\nSUMMARY: 3 failed\n\n[Output truncated. Full output: /tmp/log]";
    t.finish_tool(ToolStatus::Ok, Some(output.to_string()));
    let rows = strip(&t.render(80, &plain()));
    let joined = rows.join("\n");
    assert!(
        joined.contains("SUMMARY: 3 failed"),
        "the summary survives at the tail: {joined}"
    );
    assert_eq!(
        joined.matches("Full output").count(),
        0,
        "the footer is not in the preview: {joined}"
    );
    assert_eq!(
        joined.matches("earlier lines").count(),
        1,
        "exactly one omission marker: {joined}"
    );
}
